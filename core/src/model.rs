use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::{fmt, net::Ipv4Addr, net::SocketAddr};
use uuid::Uuid;

pub(crate) use boringtun::x25519::PublicKey;

pub(crate) const LATTIS_NETWORK_PREFIX: Ipv4Addr = Ipv4Addr::new(100, 64, 0, 0);
pub(crate) const LATTIS_NETWORK_PREFIX_LEN: u8 = 10;
pub(crate) const LATTIS_NETWORK_HOST_BITS: u32 = 32 - LATTIS_NETWORK_PREFIX_LEN as u32;
pub(crate) const LATTIS_NETWORK_ADDRESS_COUNT: u32 = 1 << LATTIS_NETWORK_HOST_BITS;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct DeviceID(Uuid);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PeerInfo {
    pub(crate) device_id: DeviceID,
    pub(crate) public_key: PublicKey,
    pub(crate) virtual_addresses: Vec<IpNet>,
    pub(crate) endpoints: Vec<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegisterDeviceRequest {
    pub(crate) device_id: DeviceID,
    pub(crate) public_key: PublicKey,
    pub(crate) endpoints: Vec<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegisterDeviceResponse {
    pub(crate) device: PeerInfo,
    pub(crate) peers: Vec<PeerInfo>,
}

impl fmt::Display for DeviceID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl DeviceID {
    pub(crate) fn random() -> Self {
        Self(Uuid::new_v4())
    }
}
