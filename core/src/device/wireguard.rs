use anyhow::{Result, anyhow};
use boringtun::noise::{Packet, Tunn, TunnResult};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::net::UdpSocket;

use super::{packet::PacketDevice, peer::Peer};

pub(super) const MTU: usize = 1500;
const WIREGUARD_OVERHEAD: usize = 32;
pub(super) const WIREGUARD_PACKET_BUFFER_SIZE: usize = MTU + WIREGUARD_OVERHEAD;
const WIREGUARD_HANDSHAKE_RESPONSE: u32 = 2;
const WIREGUARD_HANDSHAKE_RESPONSE_SIZE: usize = 92;

pub(super) struct WireGuardIo {
    packet_device: Arc<dyn PacketDevice>,
    socket: Arc<UdpSocket>,
}

pub(super) enum EndpointUpdate {
    VerifiedPacket,
    GeneratedHandshakeResponse,
}

impl WireGuardIo {
    pub(super) async fn bind(
        listen_port: u16,
        packet_device: Arc<dyn PacketDevice>,
    ) -> Result<Self> {
        let socket_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen_port);
        let socket = UdpSocket::bind(&socket_address).await?;

        Ok(Self {
            packet_device,
            socket: Arc::new(socket),
        })
    }

    pub(super) async fn recv_datagram(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        Ok(self.socket.recv_from(buf).await?)
    }

    pub(super) async fn recv_packet(&self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.packet_device.recv(buf).await?)
    }

    pub(super) async fn encapsulate_packet(
        &self,
        peer: Arc<Peer>,
        raw_packet: &[u8],
    ) -> Result<()> {
        let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];

        let result = {
            let mut tunnel = peer
                .tunnel
                .lock()
                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

            tunnel.encapsulate(raw_packet, &mut out_buf)
        };
        let endpoint = peer.selected_endpoint()?;

        self.handle_tunn_result(&peer, result, endpoint, "outbound")
            .await
    }

    pub(super) async fn decapsulate_datagram(
        &self,
        peer: Arc<Peer>,
        raw_packet: &[u8],
        src: SocketAddr,
        endpoint_update: EndpointUpdate,
        log_errors: bool,
    ) -> Result<bool> {
        let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];

        let result = {
            let mut tunnel = peer
                .tunnel
                .lock()
                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

            tunnel.decapsulate(Some(src.ip()), raw_packet, &mut out_buf)
        };

        if let TunnResult::Err(err) = &result {
            if log_errors {
                eprintln!("WireGuard inbound error from {}: {:?}", src, err);
            }
            return Ok(false);
        }

        let should_confirm_endpoint = match endpoint_update {
            EndpointUpdate::VerifiedPacket => true,
            EndpointUpdate::GeneratedHandshakeResponse => is_wireguard_handshake_response(&result),
        };
        let endpoint_confirmed = should_confirm_endpoint && peer.confirm_endpoint(src)?;

        self.handle_tunn_result(&peer, result, src, "inbound")
            .await?;
        self.drain_peer(peer.clone(), src).await?;

        if should_confirm_endpoint && !endpoint_confirmed {
            self.probe_endpoint(&peer, src).await?;
        }

        Ok(true)
    }

    pub(super) async fn probe_endpoint(&self, peer: &Peer, endpoint: SocketAddr) -> Result<()> {
        let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];

        let result = {
            let mut tunnel = peer
                .tunnel
                .lock()
                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

            tunnel.format_handshake_initiation(&mut out_buf, true)
        };

        match result {
            TunnResult::WriteToNetwork(packet) => {
                self.socket.send_to(packet, endpoint).await?;
                peer.record_endpoint_probe(endpoint)?;
            }
            TunnResult::Done => {}
            TunnResult::Err(err) => {
                return Err(anyhow!("WireGuard handshake probe error: {:?}", err));
            }
            TunnResult::WriteToTunnelV4(_, _) | TunnResult::WriteToTunnelV6(_, _) => {
                return Err(anyhow!("Unexpected handshake probe output type"));
            }
        }

        Ok(())
    }

    pub(super) async fn update_timers(&self, peer: Arc<Peer>) -> Result<()> {
        let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];

        let result = {
            let mut tunnel = peer
                .tunnel
                .lock()
                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

            tunnel.update_timers(&mut out_buf)
        };
        let endpoint = peer.selected_endpoint()?;

        self.handle_tunn_result(&peer, result, endpoint, "timer")
            .await
    }

    async fn drain_peer(&self, peer: Arc<Peer>, endpoint: SocketAddr) -> Result<()> {
        loop {
            let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];
            let result = {
                let mut tunnel = peer
                    .tunnel
                    .lock()
                    .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

                tunnel.decapsulate(None, &[], &mut out_buf)
            };

            let done = matches!(result, TunnResult::Done | TunnResult::Err(_));
            self.handle_tunn_result(&peer, result, endpoint, "drain")
                .await?;

            if done {
                break;
            }
        }

        Ok(())
    }

    async fn handle_tunn_result(
        &self,
        peer: &Peer,
        result: TunnResult<'_>,
        endpoint: SocketAddr,
        context: &str,
    ) -> Result<()> {
        match result {
            TunnResult::WriteToNetwork(packet) => {
                self.socket.send_to(packet, endpoint).await?;
            }
            TunnResult::WriteToTunnelV4(packet, source) => {
                self.send_tunnel_packet(peer, packet, IpAddr::V4(source))
                    .await?;
            }
            TunnResult::WriteToTunnelV6(packet, source) => {
                self.send_tunnel_packet(peer, packet, IpAddr::V6(source))
                    .await?;
            }
            TunnResult::Done => {}
            TunnResult::Err(err) => {
                eprintln!("WireGuard {} error from {}: {:?}", context, endpoint, err);
            }
        }

        Ok(())
    }

    async fn send_tunnel_packet(&self, peer: &Peer, packet: &[u8], source: IpAddr) -> Result<()> {
        if peer.has_address(source)? {
            self.packet_device.send(packet).await?;
        } else {
            eprintln!(
                "Dropped WireGuard packet from unauthorized source {}",
                source
            );
        }

        Ok(())
    }
}

pub(super) fn packet_receiver_index(raw_packet: &[u8]) -> Option<u32> {
    let packet = Tunn::parse_incoming_packet(raw_packet).ok()?;

    match packet {
        Packet::HandshakeInit(_) => None,
        Packet::HandshakeResponse(packet) => Some(packet.receiver_idx),
        Packet::PacketCookieReply(packet) => Some(packet.receiver_idx),
        Packet::PacketData(packet) => Some(packet.receiver_idx),
    }
}

fn is_wireguard_handshake_response(result: &TunnResult<'_>) -> bool {
    let TunnResult::WriteToNetwork(packet) = result else {
        return false;
    };

    if packet.len() != WIREGUARD_HANDSHAKE_RESPONSE_SIZE {
        return false;
    }

    let Ok(message_type) = packet[..4].try_into().map(u32::from_le_bytes) else {
        return false;
    };

    message_type == WIREGUARD_HANDSHAKE_RESPONSE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(message_type: u32, receiver_index: Option<u32>, len: usize) -> Vec<u8> {
        let mut packet = vec![0; len];
        packet[..4].copy_from_slice(&message_type.to_le_bytes());

        if let Some(receiver_index) = receiver_index {
            let offset = if message_type == WIREGUARD_HANDSHAKE_RESPONSE {
                8
            } else {
                4
            };
            packet[offset..offset + 4].copy_from_slice(&receiver_index.to_le_bytes());
        }

        packet
    }

    #[test]
    fn packet_receiver_index_ignores_handshake_init() {
        let packet = packet(1, None, 148);

        assert_eq!(packet_receiver_index(&packet), None);
    }

    #[test]
    fn packet_receiver_index_reads_handshake_response() {
        let packet = packet(WIREGUARD_HANDSHAKE_RESPONSE, Some(0x0102_0304), 92);

        assert_eq!(packet_receiver_index(&packet), Some(0x0102_0304));
    }

    #[test]
    fn packet_receiver_index_reads_cookie_reply() {
        let packet = packet(3, Some(0x0102_0304), 64);

        assert_eq!(packet_receiver_index(&packet), Some(0x0102_0304));
    }

    #[test]
    fn packet_receiver_index_reads_data_packet() {
        let packet = packet(4, Some(0x0102_0304), 32);

        assert_eq!(packet_receiver_index(&packet), Some(0x0102_0304));
    }

    #[test]
    fn packet_receiver_index_rejects_short_or_unknown_packets() {
        assert_eq!(packet_receiver_index(&[1, 2, 3]), None);
        assert_eq!(packet_receiver_index(&packet(99, None, 16)), None);
    }
}
