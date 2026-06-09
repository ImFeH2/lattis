use anyhow::{Ok, Result, bail};
use etherparse::IpSlice;
use ipnet::IpNet;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{net::UdpSocket, task::JoinHandle};

pub struct Device {
    outbound: JoinHandle<Result<()>>,
    inbound: JoinHandle<Result<()>>,
}

impl Device {
    pub fn builder() -> DeviceBuilder {
        DeviceBuilder {
            interface_name: "lattis0".to_string(),
            listen_port: 8000,
            local: LocalConfig { addresses: vec![] },
            peers: vec![],
        }
    }

    async fn start(config: DeviceConfig) -> Result<Self> {
        if config.peers.is_empty() {
            bail!("At least one peer must be configured");
        }

        if config.local.addresses.is_empty() {
            bail!("At least one local address must be configured");
        }

        let builder = tun_rs::DeviceBuilder::new().name(config.interface_name);
        let tun = builder.build_async()?;
        for addr in &config.local.addresses {
            match addr {
                IpNet::V4(address) => tun.add_address_v4(address.addr(), address.prefix_len())?,
                IpNet::V6(address) => tun.add_address_v6(address.addr(), address.prefix_len())?,
            };
        }

        let socket_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), config.listen_port);
        let socket = UdpSocket::bind(&socket_address).await?;

        let tun = Arc::new(tun);
        let socket = Arc::new(socket);
        let peers = Arc::new(config.peers);

        let outbound = {
            let tun = tun.clone();
            let socket = socket.clone();
            let peers = peers.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 1500];
                loop {
                    let len = tun.recv(&mut buf).await?;
                    let raw_packet = &buf[..len];
                    let ip_packet = IpSlice::from_slice(raw_packet)?;
                    let dst = ip_packet.destination_addr();

                    let peer = peers
                        .iter()
                        .find(|p| p.allowed_ips.iter().any(|net| net.contains(&dst)));

                    if let Some(peer) = peer {
                        socket.send_to(raw_packet, peer.endpoint).await?;
                    }
                }
            })
        };

        let inbound = {
            let tun = tun.clone();
            let socket = socket.clone();
            let peers = peers.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 1500];
                loop {
                    let (len, src) = socket.recv_from(&mut buf).await?;
                    let packet = &buf[..len];

                    if peers.iter().any(|p| p.endpoint == src) {
                        tun.send(packet).await?;
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

#[derive(Debug)]
pub struct Peer {
    pub allowed_ips: Vec<IpNet>,
    pub endpoint: SocketAddr,
}

#[derive(Debug)]
pub struct DeviceConfig {
    interface_name: String,
    listen_port: u16,
    local: LocalConfig,
    peers: Vec<Peer>,
}

#[derive(Debug)]
pub struct LocalConfig {
    pub addresses: Vec<IpNet>,
}

pub struct DeviceBuilder {
    interface_name: String,
    listen_port: u16,
    local: LocalConfig,
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

    pub fn add_local_address(mut self, address: IpNet) -> Self {
        self.local.addresses.push(address);
        self
    }

    pub fn add_peer(mut self, peer: Peer) -> Self {
        self.peers.push(peer);
        self
    }

    pub async fn build(self) -> Result<Device> {
        Device::start(DeviceConfig {
            interface_name: self.interface_name,
            listen_port: self.listen_port,
            local: self.local,
            peers: self.peers,
        })
        .await
    }
}
