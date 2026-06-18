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
