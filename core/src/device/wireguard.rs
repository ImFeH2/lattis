use anyhow::{Result, anyhow};
use boringtun::noise::{Packet, Tunn, TunnResult};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio::net::UdpSocket;

use super::{packet::PacketDevice, peer::Peer};

pub(super) const MTU: usize = 1500;
const WIREGUARD_OVERHEAD: usize = 32;
pub(super) const WIREGUARD_PACKET_BUFFER_SIZE: usize = MTU + WIREGUARD_OVERHEAD;
const WIREGUARD_HANDSHAKE_RESPONSE: u32 = 2;
const WIREGUARD_HANDSHAKE_RESPONSE_SIZE: usize = 92;

pub(super) enum EndpointUpdate {
    None,
    VerifiedPacket,
    HandshakeResponse,
}

pub(super) async fn handle_peer_datagram(
    peer: Arc<Peer>,
    raw_packet: &[u8],
    src: SocketAddr,
    packet_device: &dyn PacketDevice,
    socket: &UdpSocket,
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

    let update_endpoint = match endpoint_update {
        EndpointUpdate::None => false,
        EndpointUpdate::VerifiedPacket => true,
        EndpointUpdate::HandshakeResponse => is_wireguard_handshake_response(&result),
    };

    if update_endpoint {
        peer.update_endpoint(src)?;
    }

    handle_tunn_result(&peer, result, packet_device, socket, src, "inbound").await?;
    drain_peer(peer, packet_device, socket, src).await?;

    Ok(true)
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

async fn drain_peer(
    peer: Arc<Peer>,
    packet_device: &dyn PacketDevice,
    socket: &UdpSocket,
    endpoint: SocketAddr,
) -> Result<()> {
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
        handle_tunn_result(&peer, result, packet_device, socket, endpoint, "drain").await?;

        if done {
            break;
        }
    }

    Ok(())
}

async fn handle_tunn_result(
    peer: &Peer,
    result: TunnResult<'_>,
    packet_device: &dyn PacketDevice,
    socket: &UdpSocket,
    endpoint: SocketAddr,
    context: &str,
) -> Result<()> {
    match result {
        TunnResult::WriteToNetwork(packet) => {
            socket.send_to(packet, endpoint).await?;
        }
        TunnResult::WriteToTunnelV4(packet, source) => {
            let source = IpAddr::V4(source);
            if peer.has_address(source)? {
                packet_device.send(packet).await?;
            } else {
                eprintln!(
                    "Dropped WireGuard packet from unauthorized source {}",
                    source
                );
            }
        }
        TunnResult::WriteToTunnelV6(packet, source) => {
            let source = IpAddr::V6(source);
            if peer.has_address(source)? {
                packet_device.send(packet).await?;
            } else {
                eprintln!(
                    "Dropped WireGuard packet from unauthorized source {}",
                    source
                );
            }
        }
        TunnResult::Done => {}
        TunnResult::Err(err) => {
            eprintln!("WireGuard {} error from {}: {:?}", context, endpoint, err);
        }
    }

    Ok(())
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
