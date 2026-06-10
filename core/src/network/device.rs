use anyhow::{Result, anyhow, bail, ensure};
use boringtun::noise::{Packet, Tunn, TunnResult};
use etherparse::IpSlice;
use ipnet::IpNet;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU32, Ordering},
    },
};
use tokio::{
    net::UdpSocket,
    task::JoinHandle,
    time::{Duration, interval},
};

use super::{DeviceConfig, Peer, PrivateKey, PublicKey};

const MTU: usize = 1500;
const WIREGUARD_OVERHEAD: usize = 32;
const WIREGUARD_PACKET_BUFFER_SIZE: usize = MTU + WIREGUARD_OVERHEAD;
const WIREGUARD_TIMER_INTERVAL: Duration = Duration::from_millis(250);
const WIREGUARD_HANDSHAKE_RESPONSE: u32 = 2;
const WIREGUARD_HANDSHAKE_RESPONSE_SIZE: usize = 92;

pub struct Device {
    state: Arc<DeviceState>,
    outbound: JoinHandle<Result<()>>,
    inbound: JoinHandle<Result<()>>,
    timer: JoinHandle<Result<()>>,
}

impl Device {
    pub(crate) async fn start(
        interface_name: String,
        listen_port: u16,
        config: DeviceConfig,
    ) -> Result<Self> {
        if config.addresses.is_empty() {
            bail!("At least one virtual address must be configured");
        }

        let builder = tun_rs::DeviceBuilder::new().name(interface_name);
        let tun = builder.build_async()?;
        for addr in &config.addresses {
            match addr {
                IpNet::V4(address) => tun.add_address_v4(address.addr(), address.prefix_len())?,
                IpNet::V6(address) => tun.add_address_v6(address.addr(), address.prefix_len())?,
            };
        }

        let socket_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen_port);
        let socket = UdpSocket::bind(&socket_address).await?;

        let state = Arc::new(DeviceState::new(config.private_key));
        let tun = Arc::new(tun);
        let socket = Arc::new(socket);

        let outbound = {
            let tun = tun.clone();
            let socket = socket.clone();
            let state = state.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; MTU];
                loop {
                    let len = tun.recv(&mut buf).await?;
                    let raw_packet = &buf[..len];
                    let ip_packet = IpSlice::from_slice(raw_packet)?;
                    let dst = ip_packet.destination_addr();

                    let peer = state.find_peer_by_destination(dst)?;

                    if let Some(peer) = peer {
                        let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];

                        let result = {
                            let mut tunnel = peer
                                .tunnel
                                .lock()
                                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

                            tunnel.encapsulate(raw_packet, &mut out_buf)
                        };

                        match result {
                            TunnResult::WriteToNetwork(packet) => {
                                let endpoint = peer.endpoint()?;
                                socket.send_to(packet, endpoint).await?;
                            }
                            TunnResult::Done => {}
                            TunnResult::Err(err) => {
                                eprintln!("WireGuard outbound error: {:?}", err);
                            }
                            TunnResult::WriteToTunnelV4(_, _)
                            | TunnResult::WriteToTunnelV6(_, _) => {
                                eprintln!("Unexpected tunnel output type");
                            }
                        }
                    }
                }
            })
        };

        let inbound = {
            let tun = tun.clone();
            let socket = socket.clone();
            let state = state.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];
                loop {
                    let (len, src) = socket.recv_from(&mut buf).await?;
                    let raw_packet = &buf[..len];

                    if let Some(peer) = state.find_peer_by_endpoint(src)? {
                        handle_peer_datagram(
                            peer,
                            raw_packet,
                            src,
                            &tun,
                            &socket,
                            EndpointUpdate::None,
                            true,
                        )
                        .await?;
                        continue;
                    }

                    if let Some(peer) = state.find_peer_by_packet(raw_packet)? {
                        handle_peer_datagram(
                            peer,
                            raw_packet,
                            src,
                            &tun,
                            &socket,
                            EndpointUpdate::VerifiedPacket,
                            true,
                        )
                        .await?;
                        continue;
                    }

                    let mut handled = false;
                    for peer in state.peers()? {
                        if handle_peer_datagram(
                            peer,
                            raw_packet,
                            src,
                            &tun,
                            &socket,
                            EndpointUpdate::HandshakeResponse,
                            false,
                        )
                        .await?
                        {
                            handled = true;
                            break;
                        }
                    }

                    if !handled {
                        eprintln!("Received packet from unknown peer: {}", src);
                    }
                }
            })
        };

        let timer = {
            let socket = socket.clone();
            let state = state.clone();

            tokio::spawn(async move {
                let mut interval = interval(WIREGUARD_TIMER_INTERVAL);

                loop {
                    interval.tick().await;

                    for peer in state.peers()? {
                        let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];

                        let result = {
                            let mut tunnel = peer
                                .tunnel
                                .lock()
                                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

                            tunnel.update_timers(&mut out_buf)
                        };

                        match result {
                            TunnResult::WriteToNetwork(packet) => {
                                let endpoint = peer.endpoint()?;
                                socket.send_to(packet, endpoint).await?;
                            }
                            TunnResult::Done => {}
                            TunnResult::Err(err) => {
                                eprintln!("WireGuard timer error: {:?}", err);
                            }
                            TunnResult::WriteToTunnelV4(_, _)
                            | TunnResult::WriteToTunnelV6(_, _) => {
                                eprintln!("Unexpected timer output type");
                            }
                        }
                    }
                }
            })
        };

        Ok(Self {
            state,
            outbound,
            inbound,
            timer,
        })
    }

    pub fn add_peer(&self, peer: Peer) -> Result<()> {
        self.state.add_peer(peer)
    }

    pub fn remove_peer(&self, public_key: &PublicKey) -> Result<()> {
        self.state.remove_peer(public_key)
    }

    pub fn update_peer_endpoint(&self, public_key: &PublicKey, endpoint: SocketAddr) -> Result<()> {
        self.state.update_peer_endpoint(public_key, endpoint)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.outbound.abort();
        self.inbound.abort();
        self.timer.abort();
    }
}

struct WireGuardPeer {
    index: u32,
    public_key: PublicKey,
    allowed_ips: Vec<IpNet>,
    endpoint: RwLock<SocketAddr>,
    tunnel: Mutex<Tunn>,
}

struct DeviceState {
    private_key: PrivateKey,
    peers: RwLock<PeerTable>,
    next_peer_index: AtomicU32,
}

#[derive(Default)]
struct PeerTable {
    peers: Vec<Arc<WireGuardPeer>>,
}

impl WireGuardPeer {
    fn new(
        private_key: PrivateKey,
        public_key: PublicKey,
        allowed_ips: Vec<IpNet>,
        endpoint: SocketAddr,
        index: u32,
    ) -> Self {
        let tunnel = Tunn::new(private_key, public_key, None, None, index, None);

        Self {
            index,
            public_key,
            allowed_ips,
            endpoint: RwLock::new(endpoint),
            tunnel: Mutex::new(tunnel),
        }
    }

    fn endpoint(&self) -> Result<SocketAddr> {
        Ok(*self
            .endpoint
            .read()
            .map_err(|_| anyhow!("WireGuard peer endpoint lock error"))?)
    }

    fn update_endpoint(&self, endpoint: SocketAddr) -> Result<()> {
        *self
            .endpoint
            .write()
            .map_err(|_| anyhow!("WireGuard peer endpoint lock error"))? = endpoint;
        Ok(())
    }

    fn public_key_matches(&self, public_key: &PublicKey) -> bool {
        self.public_key.to_bytes() == public_key.to_bytes()
    }
}

impl DeviceState {
    fn new(private_key: PrivateKey) -> Self {
        Self {
            private_key,
            peers: RwLock::new(PeerTable::default()),
            next_peer_index: AtomicU32::new(1),
        }
    }

    fn add_peer(&self, peer: Peer) -> Result<()> {
        let mut peers = self
            .peers
            .write()
            .map_err(|_| anyhow!("WireGuard peer table lock error"))?;

        ensure!(
            !peers.contains_public_key(&peer.public_key),
            "WireGuard peer already exists"
        );

        let index = self.next_peer_index.fetch_add(1, Ordering::Relaxed);
        peers.peers.push(Arc::new(WireGuardPeer::new(
            self.private_key.clone(),
            peer.public_key,
            peer.allowed_ips,
            peer.endpoint,
            index,
        )));

        Ok(())
    }

    fn remove_peer(&self, public_key: &PublicKey) -> Result<()> {
        let mut peers = self
            .peers
            .write()
            .map_err(|_| anyhow!("WireGuard peer table lock error"))?;

        let original_len = peers.peers.len();
        peers
            .peers
            .retain(|peer| !peer.public_key_matches(public_key));

        ensure!(
            peers.peers.len() != original_len,
            "WireGuard peer does not exist"
        );

        Ok(())
    }

    fn update_peer_endpoint(&self, public_key: &PublicKey, endpoint: SocketAddr) -> Result<()> {
        let peer = {
            let peers = self
                .peers
                .read()
                .map_err(|_| anyhow!("WireGuard peer table lock error"))?;

            peers.find_by_public_key(public_key)
        };

        let Some(peer) = peer else {
            bail!("WireGuard peer does not exist");
        };

        peer.update_endpoint(endpoint)
    }

    fn find_peer_by_destination(&self, dst: IpAddr) -> Result<Option<Arc<WireGuardPeer>>> {
        let peers = self
            .peers
            .read()
            .map_err(|_| anyhow!("WireGuard peer table lock error"))?;

        Ok(peers.find_by_destination(dst))
    }

    fn find_peer_by_endpoint(&self, endpoint: SocketAddr) -> Result<Option<Arc<WireGuardPeer>>> {
        let peers = self
            .peers
            .read()
            .map_err(|_| anyhow!("WireGuard peer table lock error"))?;

        peers.find_by_endpoint(endpoint)
    }

    fn find_peer_by_packet(&self, raw_packet: &[u8]) -> Result<Option<Arc<WireGuardPeer>>> {
        let packet = match Tunn::parse_incoming_packet(raw_packet) {
            Ok(packet) => packet,
            Err(_) => return Ok(None),
        };

        let Some(index) = packet_receiver_index(&packet) else {
            return Ok(None);
        };

        let peers = self
            .peers
            .read()
            .map_err(|_| anyhow!("WireGuard peer table lock error"))?;

        Ok(peers.find_by_index(index >> 8))
    }

    fn peers(&self) -> Result<Vec<Arc<WireGuardPeer>>> {
        let peers = self
            .peers
            .read()
            .map_err(|_| anyhow!("WireGuard peer table lock error"))?;

        Ok(peers.all())
    }
}

impl PeerTable {
    fn contains_public_key(&self, public_key: &PublicKey) -> bool {
        self.peers
            .iter()
            .any(|peer| peer.public_key_matches(public_key))
    }

    fn find_by_public_key(&self, public_key: &PublicKey) -> Option<Arc<WireGuardPeer>> {
        self.peers
            .iter()
            .find(|peer| peer.public_key_matches(public_key))
            .cloned()
    }

    fn find_by_destination(&self, dst: IpAddr) -> Option<Arc<WireGuardPeer>> {
        self.peers
            .iter()
            .find(|peer| peer.allowed_ips.iter().any(|net| net.contains(&dst)))
            .cloned()
    }

    fn find_by_endpoint(&self, endpoint: SocketAddr) -> Result<Option<Arc<WireGuardPeer>>> {
        for peer in &self.peers {
            if peer.endpoint()? == endpoint {
                return Ok(Some(peer.clone()));
            }
        }

        Ok(None)
    }

    fn find_by_index(&self, index: u32) -> Option<Arc<WireGuardPeer>> {
        self.peers.iter().find(|peer| peer.index == index).cloned()
    }

    fn all(&self) -> Vec<Arc<WireGuardPeer>> {
        self.peers.clone()
    }
}

enum EndpointUpdate {
    None,
    VerifiedPacket,
    HandshakeResponse,
}

async fn handle_peer_datagram(
    peer: Arc<WireGuardPeer>,
    raw_packet: &[u8],
    src: SocketAddr,
    tun: &tun_rs::AsyncDevice,
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

    handle_tunn_result(result, tun, socket, src, "inbound").await?;
    drain_peer(peer, tun, socket, src).await?;

    Ok(true)
}

async fn drain_peer(
    peer: Arc<WireGuardPeer>,
    tun: &tun_rs::AsyncDevice,
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
        handle_tunn_result(result, tun, socket, endpoint, "drain").await?;

        if done {
            break;
        }
    }

    Ok(())
}

async fn handle_tunn_result(
    result: TunnResult<'_>,
    tun: &tun_rs::AsyncDevice,
    socket: &UdpSocket,
    endpoint: SocketAddr,
    context: &str,
) -> Result<()> {
    match result {
        TunnResult::WriteToNetwork(packet) => {
            socket.send_to(packet, endpoint).await?;
        }
        TunnResult::WriteToTunnelV4(packet, _) | TunnResult::WriteToTunnelV6(packet, _) => {
            tun.send(packet).await?;
        }
        TunnResult::Done => {}
        TunnResult::Err(err) => {
            eprintln!("WireGuard {} error from {}: {:?}", context, endpoint, err);
        }
    }

    Ok(())
}

fn packet_receiver_index(packet: &Packet<'_>) -> Option<u32> {
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
