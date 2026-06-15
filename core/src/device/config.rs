use anyhow::Result;
use ipnet::IpNet;
use rand_core::OsRng;
use std::{net::SocketAddr, sync::Arc};

use super::{Device, PacketDevice, PrivateKey, PublicKey};

const DEFAULT_INTERFACE_NAME: &str = "lattis0";
pub const DEFAULT_DEVICE_LISTEN_PORT: u16 = 52171;

pub struct PeerConfig {
    pub(super) public_key: PublicKey,
    pub(super) allowed_ips: Vec<IpNet>,
    pub(super) endpoint: SocketAddr,
}

impl PeerConfig {
    pub fn new(public_key: PublicKey, allowed_ips: Vec<IpNet>, endpoint: SocketAddr) -> Self {
        Self {
            public_key,
            allowed_ips,
            endpoint,
        }
    }

    pub fn from_identity(identity: &DeviceIdentity, endpoint: SocketAddr) -> Self {
        Self {
            public_key: identity.public_key,
            allowed_ips: identity.virtual_addresses.clone(),
            endpoint,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub public_key: PublicKey,
    pub virtual_addresses: Vec<IpNet>,
}

pub struct DeviceConfig {
    pub private_key: PrivateKey,
    pub virtual_addresses: Vec<IpNet>,
}

pub struct DeviceBuilder {
    interface_name: String,
    listen_port: u16,
    config: DeviceConfig,
    packet_device: Option<Arc<dyn PacketDevice>>,
}

impl Device {
    pub fn builder() -> DeviceBuilder {
        DeviceBuilder {
            interface_name: DEFAULT_INTERFACE_NAME.to_string(),
            listen_port: DEFAULT_DEVICE_LISTEN_PORT,
            config: DeviceConfig {
                private_key: PrivateKey::random_from_rng(OsRng),
                virtual_addresses: vec![],
            },
            packet_device: None,
        }
    }
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
        self.config.virtual_addresses.push(address);
        self
    }

    pub fn packet_device(mut self, device: impl PacketDevice) -> Self {
        self.packet_device = Some(Arc::new(device));
        self
    }

    pub async fn build(self) -> Result<Device> {
        match self.packet_device {
            Some(packet_device) => {
                Device::start_with_packet_device(self.listen_port, self.config, packet_device).await
            }
            None => Device::start(self.interface_name, self.listen_port, self.config).await,
        }
    }
}
