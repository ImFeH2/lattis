use anyhow::{Ok, Result, anyhow, bail};
use boringtun::noise::{Tunn, TunnResult};
pub use boringtun::x25519::{PublicKey, StaticSecret as PrivateKey};
use etherparse::IpSlice;
use ipnet::IpNet;
use rand_core::OsRng;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};
use tokio::{net::UdpSocket, task::JoinHandle};

const DEFAULT_INTERFACE_NAME: &str = "lattis0";
const DEFAULT_LISTEN_PORT: u16 = 52171;
const MTU: usize = 1500;
const WIREGUARD_OVERHEAD: usize = 32;
const WIREGUARD_PACKET_BUFFER_SIZE: usize = MTU + WIREGUARD_OVERHEAD;

pub struct Device {
    outbound: JoinHandle<Result<()>>,
    inbound: JoinHandle<Result<()>>,
}

impl Device {
    pub fn builder() -> DeviceBuilder {
        DeviceBuilder {
            interface_name: DEFAULT_INTERFACE_NAME.to_string(),
            listen_port: DEFAULT_LISTEN_PORT,
            config: DeviceConfig {
                private_key: PrivateKey::random_from_rng(OsRng),
                addresses: vec![],
            },
            peers: vec![],
        }
    }

    async fn start(
        interface_name: String,
        listen_port: u16,
        config: DeviceConfig,
        peers: Vec<Peer>,
    ) -> Result<Self> {
        if peers.is_empty() {
            bail!("At least one peer must be configured");
        }

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

        let peers: Vec<WireGuardPeer> = peers
            .into_iter()
            .enumerate()
            .map(|(index, p)| {
                WireGuardPeer::new(
                    config.private_key.clone(),
                    p.public_key,
                    p.allowed_ips,
                    p.endpoint,
                    index as u32 + 1,
                )
            })
            .collect();

        let tun = Arc::new(tun);
        let socket = Arc::new(socket);
        let peers = Arc::new(peers);

        let outbound = {
            let tun = tun.clone();
            let socket = socket.clone();
            let peers = peers.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; MTU];
                loop {
                    let len = tun.recv(&mut buf).await?;
                    let raw_packet = &buf[..len];
                    let ip_packet = IpSlice::from_slice(raw_packet)?;
                    let dst = ip_packet.destination_addr();

                    let peer = peers
                        .iter()
                        .find(|p| p.allowed_ips.iter().any(|net| net.contains(&dst)));

                    if let Some(peer) = peer {
                        let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];
                        let endpoint = peer.endpoint;

                        let result = {
                            let mut tunnel = peer
                                .tunnel
                                .lock()
                                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

                            tunnel.encapsulate(raw_packet, &mut out_buf)
                        };

                        match result {
                            TunnResult::WriteToNetwork(packet) => {
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
            let peers = peers.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];
                loop {
                    let (len, src) = socket.recv_from(&mut buf).await?;
                    let raw_packet = &buf[..len];

                    let Some(peer) = peers.iter().find(|p| p.endpoint == src) else {
                        eprintln!("Received packet from unknown peer: {}", src);
                        continue;
                    };

                    let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];
                    let endpoint = peer.endpoint;

                    let result = {
                        let mut tunnel = peer
                            .tunnel
                            .lock()
                            .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

                        tunnel.decapsulate(Some(src.ip()), raw_packet, &mut out_buf)
                    };

                    match result {
                        TunnResult::WriteToNetwork(packet) => {
                            socket.send_to(packet, endpoint).await?;
                        }
                        TunnResult::WriteToTunnelV4(packet, _)
                        | TunnResult::WriteToTunnelV6(packet, _) => {
                            tun.send(packet).await?;
                        }
                        TunnResult::Done => {}
                        TunnResult::Err(err) => {
                            eprintln!("WireGuard inbound error from {}: {:?}", src, err);
                        }
                    }

                    loop {
                        let result = {
                            let mut tunnel = peer
                                .tunnel
                                .lock()
                                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

                            tunnel.decapsulate(None, &[], &mut out_buf)
                        };

                        match result {
                            TunnResult::WriteToNetwork(packet) => {
                                socket.send_to(packet, endpoint).await?;
                            }
                            TunnResult::WriteToTunnelV4(packet, _)
                            | TunnResult::WriteToTunnelV6(packet, _) => {
                                tun.send(packet).await?;
                            }
                            TunnResult::Done => break,
                            TunnResult::Err(err) => {
                                eprintln!("WireGuard drain error from {}: {:?}", src, err);
                                break;
                            }
                        }
                    }
                }
            })
        };

        Ok(Self { outbound, inbound })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.outbound.abort();
        self.inbound.abort();
    }
}

pub struct Peer {
    public_key: PublicKey,
    allowed_ips: Vec<IpNet>,
    endpoint: SocketAddr,
}

struct WireGuardPeer {
    allowed_ips: Vec<IpNet>,
    endpoint: SocketAddr,
    tunnel: Mutex<Tunn>,
}

impl Peer {
    pub fn new(public_key: PublicKey, allowed_ips: Vec<IpNet>, endpoint: SocketAddr) -> Self {
        Self {
            public_key,
            allowed_ips,
            endpoint,
        }
    }
}

impl WireGuardPeer {
    pub fn new(
        private_key: PrivateKey,
        public_key: PublicKey,
        allowed_ips: Vec<IpNet>,
        endpoint: SocketAddr,
        index: u32,
    ) -> Self {
        let tunnel = Tunn::new(private_key, public_key, None, None, index, None);

        Self {
            allowed_ips,
            endpoint,
            tunnel: Mutex::new(tunnel),
        }
    }
}

pub struct DeviceConfig {
    pub private_key: PrivateKey,
    pub addresses: Vec<IpNet>,
}

pub struct DeviceBuilder {
    interface_name: String,
    listen_port: u16,
    config: DeviceConfig,
    peers: Vec<Peer>,
}

impl DeviceBuilder {
    pub fn interface_name(mut self, name: &str) -> Self {
        self.interface_name = name.to_string();
        self
    }

    pub fn listen_port(mut self, port: u16) -> Self {
        self.listen_port = port;
        self
    }

    pub fn private_key(mut self, key: PrivateKey) -> Self {
        self.config.private_key = key;
        self
    }

    pub fn add_virtual_address(mut self, address: IpNet) -> Self {
        self.config.addresses.push(address);
        self
    }

    pub fn add_peer(mut self, peer: Peer) -> Self {
        self.peers.push(peer);
        self
    }

    pub async fn build(self) -> Result<Device> {
        Device::start(
            self.interface_name,
            self.listen_port,
            self.config,
            self.peers,
        )
        .await
    }
}
