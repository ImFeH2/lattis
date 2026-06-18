use anyhow::{Result, bail, ensure};
use ipnet::{IpNet, Ipv4Net};
use rand_core::{OsRng, RngCore};
use std::{collections::HashMap, sync::Arc};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, broadcast};

use super::api;
use crate::model::{
    DeviceID, LATTIS_NETWORK_ADDRESS_COUNT, LATTIS_NETWORK_PREFIX, PeerInfo, PublicKey,
    RegisterDeviceRequest, RegisterDeviceResponse,
};

#[derive(Clone)]
pub struct Coordinator {
    peers: Arc<RwLock<HashMap<DeviceID, PeerInfo>>>,
    peer_events: broadcast::Sender<PeerInfo>,
}

impl Coordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn register(
        &self,
        mut request: RegisterDeviceRequest,
    ) -> Result<RegisterDeviceResponse> {
        validate_registration(&request)?;
        dedup_endpoints(&mut request.endpoints);

        let (registered_peer, response) = {
            let mut peers = self.peers.write().await;
            ensure_public_key_available(&peers, &request.device_id, &request.public_key)?;

            let virtual_addresses = peer_virtual_addresses(&peers, &request.device_id)?;
            let registered_peer = PeerInfo {
                device_id: request.device_id,
                public_key: request.public_key,
                virtual_addresses,
                endpoints: request.endpoints,
            };

            peers.insert(registered_peer.device_id.clone(), registered_peer.clone());
            let response = RegisterDeviceResponse {
                peers: collect_peers_for(&peers, &registered_peer.device_id)?,
                device: registered_peer.clone(),
            };

            (registered_peer, response)
        };

        self.publish_peer_update(registered_peer);
        Ok(response)
    }

    pub(crate) async fn peers_for(&self, device_id: &DeviceID) -> Result<Vec<PeerInfo>> {
        let peers = self.peers.read().await;
        collect_peers_for(&peers, device_id)
    }

    pub(crate) fn subscribe_peer_events(&self) -> broadcast::Receiver<PeerInfo> {
        self.peer_events.subscribe()
    }

    pub(crate) fn router(&self) -> axum::Router {
        api::router(self.clone())
    }

    pub async fn serve(&self, listener: TcpListener) -> Result<()> {
        axum::serve(listener, self.router()).await?;
        Ok(())
    }

    fn publish_peer_update(&self, peer: PeerInfo) {
        let _ = self.peer_events.send(peer);
    }
}

impl Default for Coordinator {
    fn default() -> Self {
        let (peer_events, _) = broadcast::channel(128);

        Self {
            peers: Arc::new(RwLock::new(HashMap::new())),
            peer_events,
        }
    }
}

fn collect_peers_for(
    peers: &HashMap<DeviceID, PeerInfo>,
    device_id: &DeviceID,
) -> Result<Vec<PeerInfo>> {
    ensure!(
        peers.contains_key(device_id),
        "Coordinator device is not registered"
    );

    Ok(peers
        .values()
        .filter(|peer| &peer.device_id != device_id)
        .cloned()
        .collect())
}

fn peer_virtual_addresses(
    peers: &HashMap<DeviceID, PeerInfo>,
    device_id: &DeviceID,
) -> Result<Vec<IpNet>> {
    if let Some(peer) = peers.get(device_id) {
        return Ok(peer.virtual_addresses.clone());
    }

    Ok(vec![IpNet::V4(allocate_virtual_address(peers)?)])
}

fn allocate_virtual_address(peers: &HashMap<DeviceID, PeerInfo>) -> Result<Ipv4Net> {
    for _ in 0..LATTIS_NETWORK_ADDRESS_COUNT {
        let host = OsRng.next_u32() % LATTIS_NETWORK_ADDRESS_COUNT;
        let candidate = u32::from(LATTIS_NETWORK_PREFIX) | host;
        let candidate = candidate.into();

        if !uses_virtual_address(peers, candidate) {
            return Ok(Ipv4Net::new(candidate, 32)?);
        }
    }

    bail!("Coordinator virtual address pool is exhausted")
}

fn uses_virtual_address(peers: &HashMap<DeviceID, PeerInfo>, address: std::net::Ipv4Addr) -> bool {
    peers.values().any(|peer| {
        peer.virtual_addresses.iter().any(
            |virtual_address| matches!(virtual_address, IpNet::V4(net) if net.addr() == address),
        )
    })
}

fn ensure_public_key_available(
    peers: &HashMap<DeviceID, PeerInfo>,
    device_id: &DeviceID,
    public_key: &PublicKey,
) -> Result<()> {
    for peer in peers.values() {
        if &peer.device_id != device_id && peer.public_key == *public_key {
            bail!("A different device already uses this public key");
        }
    }

    Ok(())
}

fn validate_registration(request: &RegisterDeviceRequest) -> Result<()> {
    ensure!(
        !request.endpoints.is_empty(),
        "Coordinator device must have at least one endpoint"
    );
    Ok(())
}

fn dedup_endpoints(endpoints: &mut Vec<std::net::SocketAddr>) {
    let mut seen = std::collections::HashSet::with_capacity(endpoints.len());
    endpoints.retain(|endpoint| seen.insert(*endpoint));
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;
    use crate::model::LATTIS_NETWORK_PREFIX_LEN;

    fn endpoint(port: u16) -> SocketAddr {
        SocketAddr::from(([192, 0, 2, 1], port))
    }

    fn public_key(value: u8) -> PublicKey {
        PublicKey::from([value; 32])
    }

    fn request(
        device_id: DeviceID,
        public_key: PublicKey,
        endpoints: Vec<SocketAddr>,
    ) -> RegisterDeviceRequest {
        RegisterDeviceRequest {
            device_id,
            public_key,
            endpoints,
        }
    }

    #[tokio::test]
    async fn register_returns_device_and_no_peers_for_first_device() -> Result<()> {
        let coordinator = Coordinator::new();
        let response = coordinator
            .register(request(
                DeviceID::random(),
                public_key(1),
                vec![endpoint(1001)],
            ))
            .await?;

        assert!(response.peers.is_empty());
        assert_eq!(response.device.public_key, public_key(1));
        assert_eq!(response.device.endpoints, vec![endpoint(1001)]);

        let address = response.device.virtual_addresses[0];
        let IpNet::V4(address) = address else {
            panic!("expected IPv4 virtual address");
        };
        let network = Ipv4Net::new(LATTIS_NETWORK_PREFIX, LATTIS_NETWORK_PREFIX_LEN)?;
        assert_eq!(address.prefix_len(), 32);
        assert!(network.contains(&address.addr()));

        Ok(())
    }

    #[tokio::test]
    async fn register_returns_existing_peers_and_excludes_self() -> Result<()> {
        let coordinator = Coordinator::new();
        let first = coordinator
            .register(request(
                DeviceID::random(),
                public_key(1),
                vec![endpoint(1001)],
            ))
            .await?;
        let second = coordinator
            .register(request(
                DeviceID::random(),
                public_key(2),
                vec![endpoint(1002)],
            ))
            .await?;

        assert_eq!(second.peers, vec![first.device.clone()]);
        assert!(
            !second
                .peers
                .iter()
                .any(|peer| peer.device_id == second.device.device_id)
        );

        let first_peers = coordinator.peers_for(&first.device.device_id).await?;
        assert_eq!(first_peers, vec![second.device]);

        Ok(())
    }

    #[tokio::test]
    async fn register_reuses_virtual_addresses_for_existing_device() -> Result<()> {
        let coordinator = Coordinator::new();
        let device_id = DeviceID::random();
        let first = coordinator
            .register(request(
                device_id.clone(),
                public_key(1),
                vec![endpoint(1001)],
            ))
            .await?;
        let second = coordinator
            .register(request(device_id, public_key(2), vec![endpoint(1002)]))
            .await?;

        assert_eq!(
            second.device.virtual_addresses,
            first.device.virtual_addresses
        );
        assert_eq!(second.device.public_key, public_key(2));
        assert_eq!(second.device.endpoints, vec![endpoint(1002)]);

        Ok(())
    }

    #[tokio::test]
    async fn register_rejects_public_key_used_by_another_device() -> Result<()> {
        let coordinator = Coordinator::new();
        coordinator
            .register(request(
                DeviceID::random(),
                public_key(1),
                vec![endpoint(1001)],
            ))
            .await?;

        let result = coordinator
            .register(request(
                DeviceID::random(),
                public_key(1),
                vec![endpoint(1002)],
            ))
            .await;

        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn register_rejects_empty_endpoints() {
        let coordinator = Coordinator::new();
        let result = coordinator
            .register(request(DeviceID::random(), public_key(1), Vec::new()))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn register_deduplicates_endpoints() -> Result<()> {
        let coordinator = Coordinator::new();
        let response = coordinator
            .register(request(
                DeviceID::random(),
                public_key(1),
                vec![endpoint(1001), endpoint(1001), endpoint(1002)],
            ))
            .await?;

        assert_eq!(
            response.device.endpoints,
            vec![endpoint(1001), endpoint(1002)]
        );

        Ok(())
    }

    #[tokio::test]
    async fn peers_for_rejects_unknown_device() {
        let coordinator = Coordinator::new();
        let result = coordinator.peers_for(&DeviceID::random()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn register_publishes_peer_update() -> Result<()> {
        let coordinator = Coordinator::new();
        let mut events = coordinator.subscribe_peer_events();
        let response = coordinator
            .register(request(
                DeviceID::random(),
                public_key(1),
                vec![endpoint(1001)],
            ))
            .await?;

        assert_eq!(events.recv().await?, response.device);

        Ok(())
    }

    #[test]
    fn uses_virtual_address_matches_ipv4_host_address_only() {
        let device_id = DeviceID::random();
        let peers = HashMap::from([(
            device_id.clone(),
            PeerInfo {
                device_id,
                public_key: public_key(1),
                virtual_addresses: vec![
                    "100.64.0.1/32".parse().unwrap(),
                    "fd00::1/128".parse().unwrap(),
                ],
                endpoints: vec![endpoint(1001)],
            },
        )]);

        assert!(uses_virtual_address(
            &peers,
            std::net::Ipv4Addr::new(100, 64, 0, 1)
        ));
        assert!(!uses_virtual_address(
            &peers,
            std::net::Ipv4Addr::new(100, 64, 0, 2)
        ));
    }
}
