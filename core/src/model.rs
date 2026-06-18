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

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> PeerInfo {
        PeerInfo {
            device_id: DeviceID::random(),
            public_key: PublicKey::from([1; 32]),
            virtual_addresses: vec!["100.64.0.1/32".parse().unwrap()],
            endpoints: vec!["192.0.2.1:1001".parse().unwrap()],
        }
    }

    #[test]
    fn device_id_display_matches_json_string() -> anyhow::Result<()> {
        let device_id = DeviceID::random();
        let value = serde_json::to_value(&device_id)?;

        assert_eq!(value, serde_json::Value::String(device_id.to_string()));
        assert_eq!(serde_json::from_value::<DeviceID>(value)?, device_id);

        Ok(())
    }

    #[test]
    fn random_device_ids_are_unique() {
        assert_ne!(DeviceID::random(), DeviceID::random());
    }

    #[test]
    fn lattis_network_constants_describe_carrier_grade_nat_block() -> anyhow::Result<()> {
        let network = ipnet::Ipv4Net::new(LATTIS_NETWORK_PREFIX, LATTIS_NETWORK_PREFIX_LEN)?;

        assert_eq!(LATTIS_NETWORK_PREFIX, Ipv4Addr::new(100, 64, 0, 0));
        assert_eq!(LATTIS_NETWORK_PREFIX_LEN, 10);
        assert_eq!(LATTIS_NETWORK_HOST_BITS, 22);
        assert_eq!(LATTIS_NETWORK_ADDRESS_COUNT, 4_194_304);
        assert_eq!(network.to_string(), "100.64.0.0/10");

        Ok(())
    }

    #[test]
    fn peer_info_json_round_trips() -> anyhow::Result<()> {
        let peer = peer();
        let json = serde_json::to_string(&peer)?;

        assert_eq!(serde_json::from_str::<PeerInfo>(&json)?, peer);

        Ok(())
    }

    #[test]
    fn register_device_response_json_round_trips() -> anyhow::Result<()> {
        let device = peer();
        let response = RegisterDeviceResponse {
            device: device.clone(),
            peers: vec![peer()],
        };
        let json = serde_json::to_string(&response)?;

        assert_eq!(
            serde_json::from_str::<RegisterDeviceResponse>(&json)?,
            response
        );

        Ok(())
    }
}
