use anyhow::{Context, Result, anyhow};
use boringtun::{noise::Tunn, x25519::StaticSecret};
use std::{
    cmp::Ordering,
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, RwLock, RwLockReadGuard},
    time::{Duration, Instant},
};

use crate::model::{DeviceID, DeviceInfo, PublicKey};

pub(super) struct Peer {
    pub(super) index: u32,
    info: RwLock<DeviceInfo>,
    paths: RwLock<HashMap<SocketAddr, PeerPath>>,
    pub(super) tunnel: Mutex<Tunn>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PeerPath {
    endpoint: SocketAddr,
    reachable: bool,
    latency: Option<Duration>,
    last_seen: Option<Instant>,
    last_probe: Option<Instant>,
}

pub(super) struct PeerTable {
    private_key: StaticSecret,
    peers: RwLock<HashMap<PublicKey, Arc<Peer>>>,
}

impl Peer {
    fn from_info(index: u32, private_key: &StaticSecret, info: DeviceInfo) -> Result<Self> {
        info.endpoints
            .first()
            .context("Coordinator peer has no endpoint")?;
        let tunnel = Tunn::new(
            private_key.clone(),
            info.public_key,
            None,
            None,
            index,
            None,
        );

        Ok(Self {
            index,
            info: RwLock::new(info),
            paths: RwLock::new(HashMap::new()),
            tunnel: Mutex::new(tunnel),
        })
    }

    pub(super) fn selected_endpoint(&self) -> Result<SocketAddr> {
        let endpoints = self
            .info
            .read()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?
            .endpoints
            .clone();
        let paths = self
            .paths
            .read()
            .map_err(|_| anyhow!("WireGuard peer paths lock error"))?;

        if let Some(path) = paths
            .values()
            .filter(|path| path.reachable)
            .min_by(|a, b| compare_peer_paths(a, b))
        {
            return Ok(path.endpoint);
        }

        endpoints
            .first()
            .copied()
            .context("Coordinator peer has no endpoint")
    }

    pub(super) fn has_address(&self, address: IpAddr) -> Result<bool> {
        let info = self
            .info
            .read()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?;

        Ok(info.addresses.iter().any(|net| net.contains(&address)))
    }

    fn has_device_id(&self, device_id: &DeviceID) -> Result<bool> {
        Ok(&self
            .info
            .read()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?
            .device_id
            == device_id)
    }

    pub(super) fn has_endpoint(&self, endpoint: SocketAddr) -> Result<bool> {
        if self
            .paths
            .read()
            .map_err(|_| anyhow!("WireGuard peer paths lock error"))?
            .contains_key(&endpoint)
        {
            return Ok(true);
        }

        Ok(self
            .info
            .read()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?
            .endpoints
            .contains(&endpoint))
    }

    pub(super) fn info(&self) -> Result<DeviceInfo> {
        Ok(self
            .info
            .read()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?
            .clone())
    }

    pub(super) fn endpoints_to_probe(&self) -> Result<Vec<SocketAddr>> {
        let endpoints = self
            .info
            .read()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?
            .endpoints
            .clone();
        let paths = self
            .paths
            .read()
            .map_err(|_| anyhow!("WireGuard peer paths lock error"))?;

        Ok(endpoints
            .into_iter()
            .filter(|endpoint| !paths.contains_key(endpoint))
            .collect())
    }

    #[cfg(test)]
    fn path(&self, endpoint: SocketAddr) -> Result<Option<PeerPath>> {
        Ok(self
            .paths
            .read()
            .map_err(|_| anyhow!("WireGuard peer paths lock error"))?
            .get(&endpoint)
            .cloned())
    }

    pub(super) fn record_endpoint_probe(&self, endpoint: SocketAddr) -> Result<()> {
        self.update_path(endpoint, |path| {
            path.last_probe = Some(Instant::now());
        })
    }

    pub(super) fn confirm_endpoint(&self, endpoint: SocketAddr) -> Result<bool> {
        let mut paths = self
            .paths
            .write()
            .map_err(|_| anyhow!("WireGuard peer paths lock error"))?;
        let Some(path) = paths.get_mut(&endpoint) else {
            return Ok(false);
        };

        let now = Instant::now();
        let last_probe = path.last_probe.take();

        path.reachable = true;
        path.last_seen = Some(now);
        path.latency = last_probe
            .map(|last_probe| now - last_probe)
            .or(path.latency);

        Ok(true)
    }

    fn update(&self, info: DeviceInfo) -> Result<()> {
        info.endpoints
            .first()
            .context("Coordinator peer has no endpoint")?;

        self.remove_stale_paths(&info.endpoints)?;
        *self
            .info
            .write()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))? = info;

        Ok(())
    }

    fn remove_stale_paths(&self, endpoints: &[SocketAddr]) -> Result<()> {
        self.paths
            .write()
            .map_err(|_| anyhow!("WireGuard peer paths lock error"))?
            .retain(|endpoint, path| endpoints.contains(endpoint) || path.has_runtime_state());

        Ok(())
    }

    fn update_path(&self, endpoint: SocketAddr, update: impl FnOnce(&mut PeerPath)) -> Result<()> {
        let mut paths = self
            .paths
            .write()
            .map_err(|_| anyhow!("WireGuard peer paths lock error"))?;
        let path = paths
            .entry(endpoint)
            .or_insert_with(|| PeerPath::new(endpoint));
        update(path);

        Ok(())
    }
}

impl PeerPath {
    fn new(endpoint: SocketAddr) -> Self {
        Self {
            endpoint,
            reachable: false,
            latency: None,
            last_seen: None,
            last_probe: None,
        }
    }

    fn has_runtime_state(&self) -> bool {
        self.reachable
            || self.latency.is_some()
            || self.last_seen.is_some()
            || self.last_probe.is_some()
    }
}

impl PeerTable {
    pub(super) fn new(private_key: StaticSecret) -> Self {
        Self {
            private_key,
            peers: RwLock::new(HashMap::new()),
        }
    }

    fn read(&self) -> Result<RwLockReadGuard<'_, HashMap<PublicKey, Arc<Peer>>>> {
        self.peers
            .read()
            .map_err(|_| anyhow!("WireGuard peer table lock error"))
    }

    pub(super) fn find_by_destination(&self, dst: IpAddr) -> Result<Option<Arc<Peer>>> {
        let peers = self.read()?;

        for peer in peers.values() {
            if peer.has_address(dst)? {
                return Ok(Some(peer.clone()));
            }
        }

        Ok(None)
    }

    pub(super) fn find_by_endpoint(&self, endpoint: SocketAddr) -> Result<Option<Arc<Peer>>> {
        let peers = self.read()?;

        for peer in peers.values() {
            if peer.has_endpoint(endpoint)? {
                return Ok(Some(peer.clone()));
            }
        }

        Ok(None)
    }

    pub(super) fn find_by_device_id(&self, device_id: &DeviceID) -> Result<Option<Arc<Peer>>> {
        let peers = self.read()?;

        for peer in peers.values() {
            if peer.has_device_id(device_id)? {
                return Ok(Some(peer.clone()));
            }
        }

        Ok(None)
    }

    pub(super) fn find_by_index(&self, index: u32) -> Result<Option<Arc<Peer>>> {
        Ok(self
            .read()?
            .values()
            .find(|peer| peer.index == index)
            .cloned())
    }

    pub(super) fn all(&self) -> Result<Vec<Arc<Peer>>> {
        Ok(self.read()?.values().cloned().collect())
    }

    pub(super) fn all_infos(&self) -> Result<Vec<DeviceInfo>> {
        self.read()?
            .values()
            .map(|peer| peer.info())
            .collect::<Result<Vec<_>>>()
    }

    pub(super) fn replace(&self, peer_infos: Vec<DeviceInfo>) -> Result<()> {
        let mut peers = self
            .peers
            .write()
            .map_err(|_| anyhow!("WireGuard peer table lock error"))?;
        let mut updated_peers = HashMap::with_capacity(peer_infos.len());

        for info in peer_infos {
            let public_key = info.public_key;

            let peer = if let Some(peer) = peers.get(&public_key) {
                peer.update(info)?;
                peer.clone()
            } else {
                Arc::new(Peer::from_info(
                    next_peer_index(&updated_peers)?,
                    &self.private_key,
                    info,
                )?)
            };

            updated_peers.insert(public_key, peer);
        }

        *peers = updated_peers;
        Ok(())
    }

    pub(super) fn upsert(&self, info: DeviceInfo) -> Result<()> {
        let mut peers = self
            .peers
            .write()
            .map_err(|_| anyhow!("WireGuard peer table lock error"))?;
        let public_key = info.public_key;

        if let Some(peer) = peers.get(&public_key) {
            peer.update(info)?;
            return Ok(());
        }

        let index = next_peer_index(&peers)?;
        peers.insert(
            public_key,
            Arc::new(Peer::from_info(index, &self.private_key, info)?),
        );
        Ok(())
    }
}

fn next_peer_index(peers: &HashMap<PublicKey, Arc<Peer>>) -> Result<u32> {
    let max_index = peers.values().map(|peer| peer.index).max().unwrap_or(0);

    max_index
        .checked_add(1)
        .context("WireGuard peer index is exhausted")
}

fn compare_peer_paths(a: &PeerPath, b: &PeerPath) -> Ordering {
    compare_path_latency(a.latency, b.latency)
        .then_with(|| b.last_seen.cmp(&a.last_seen))
        .then_with(|| a.endpoint.cmp(&b.endpoint))
}

fn compare_path_latency(a: Option<Duration>, b: Option<Duration>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceID;
    use anyhow::Context;

    fn endpoint(port: u16) -> SocketAddr {
        SocketAddr::from(([192, 0, 2, 1], port))
    }

    fn device_info(public_key: PublicKey, address: &str, endpoint: SocketAddr) -> DeviceInfo {
        DeviceInfo {
            device_id: DeviceID::random(),
            public_key,
            addresses: vec![address.parse().unwrap()],
            endpoints: vec![endpoint],
        }
    }

    fn private_key() -> StaticSecret {
        StaticSecret::from([7; 32])
    }

    fn public_key(value: u8) -> PublicKey {
        PublicKey::from([value; 32])
    }

    fn peer(table: &PeerTable, endpoint: SocketAddr) -> Result<Arc<Peer>> {
        table
            .find_by_endpoint(endpoint)?
            .context("peer should exist")
    }

    fn set_reachable_path(peer: &Peer, endpoint: SocketAddr, latency: Duration) -> Result<()> {
        peer.update_path(endpoint, |path| {
            path.reachable = true;
            path.latency = Some(latency);
            path.last_seen = Some(Instant::now());
        })
    }

    #[test]
    fn upsert_adds_peer_and_finds_it_by_destination_endpoint_and_index() -> Result<()> {
        let table = PeerTable::new(private_key());
        let info = device_info(public_key(1), "100.64.0.1/32", endpoint(1001));
        let device_id = info.device_id.clone();
        table.upsert(info)?;

        let by_destination = table.find_by_destination("100.64.0.1".parse()?)?;
        let by_endpoint = table.find_by_endpoint(endpoint(1001))?;
        let by_device_id = table.find_by_device_id(&device_id)?;
        let by_index = table.find_by_index(1)?;

        assert!(by_destination.is_some());
        assert!(by_endpoint.is_some());
        assert!(by_device_id.is_some());
        assert!(by_index.is_some());
        assert_eq!(table.all()?.len(), 1);

        Ok(())
    }

    #[test]
    fn find_by_destination_returns_none_for_unmatched_address() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;

        assert!(table.find_by_destination("100.64.0.2".parse()?)?.is_none());

        Ok(())
    }

    #[test]
    fn upsert_updates_existing_peer_in_place() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        table.upsert(device_info(public_key(1), "100.64.0.2/32", endpoint(1002)))?;

        assert_eq!(table.all()?.len(), 1);
        assert!(table.find_by_destination("100.64.0.1".parse()?)?.is_none());
        assert!(table.find_by_destination("100.64.0.2".parse()?)?.is_some());
        assert!(table.find_by_endpoint(endpoint(1001))?.is_none());
        assert!(table.find_by_endpoint(endpoint(1002))?.is_some());
        assert!(table.find_by_index(1)?.is_some());

        Ok(())
    }

    #[test]
    fn all_infos_returns_current_peer_infos() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        table.upsert(device_info(public_key(2), "100.64.0.2/32", endpoint(1002)))?;

        let infos = table.all_infos()?;

        assert_eq!(infos.len(), 2);
        assert!(infos.iter().any(|info| info.public_key == public_key(1)));
        assert!(infos.iter().any(|info| info.public_key == public_key(2)));

        Ok(())
    }

    #[test]
    fn coordinator_endpoints_are_probed_before_becoming_peer_paths() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        let peer = peer(&table, endpoint(1001))?;

        assert_eq!(peer.endpoints_to_probe()?, vec![endpoint(1001)]);
        assert!(peer.path(endpoint(1001))?.is_none());

        peer.record_endpoint_probe(endpoint(1001))?;

        assert!(peer.endpoints_to_probe()?.is_empty());
        assert!(peer.path(endpoint(1001))?.is_some());

        table.upsert(device_info(public_key(1), "100.64.0.2/32", endpoint(1002)))?;

        assert_eq!(peer.endpoints_to_probe()?, vec![endpoint(1002)]);

        Ok(())
    }

    #[test]
    fn endpoint_selects_reachable_path_with_lowest_latency() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        let peer = peer(&table, endpoint(1001))?;

        set_reachable_path(&peer, endpoint(2001), Duration::from_millis(50))?;
        set_reachable_path(&peer, endpoint(2002), Duration::from_millis(10))?;

        assert_eq!(peer.selected_endpoint()?, endpoint(2002));

        Ok(())
    }

    #[test]
    fn endpoint_confirmation_requires_existing_probe_path() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        let peer = peer(&table, endpoint(1001))?;

        assert!(!peer.confirm_endpoint(endpoint(2001))?);
        assert!(peer.path(endpoint(2001))?.is_none());
        assert_eq!(peer.selected_endpoint()?, endpoint(1001));

        Ok(())
    }

    #[test]
    fn probed_endpoint_can_be_confirmed_without_changing_device_info() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        let peer = peer(&table, endpoint(1001))?;

        peer.record_endpoint_probe(endpoint(2001))?;
        assert!(peer.confirm_endpoint(endpoint(2001))?);

        let info = peer.info()?;
        let path = peer
            .path(endpoint(2001))?
            .context("verified path should exist")?;

        assert!(path.reachable);
        assert!(path.last_seen.is_some());
        assert_eq!(peer.selected_endpoint()?, endpoint(2001));
        assert!(table.find_by_endpoint(endpoint(2001))?.is_some());
        assert_eq!(info.endpoints[0], endpoint(1001));
        assert_eq!(table.all_infos()?[0].endpoints[0], endpoint(1001));

        Ok(())
    }

    #[test]
    fn probe_records_runtime_path_without_selecting_it_before_verification() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        let peer = peer(&table, endpoint(1001))?;

        peer.record_endpoint_probe(endpoint(2001))?;

        let path = peer
            .path(endpoint(2001))?
            .context("probe path should exist")?;

        assert!(!path.reachable);
        assert!(path.last_probe.is_some());
        assert_eq!(peer.selected_endpoint()?, endpoint(1001));
        assert_eq!(peer.info()?.endpoints[0], endpoint(1001));

        Ok(())
    }

    #[test]
    fn verified_endpoint_records_latency_after_probe() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        let peer = peer(&table, endpoint(1001))?;

        peer.record_endpoint_probe(endpoint(2001))?;
        assert!(peer.confirm_endpoint(endpoint(2001))?);

        let path = peer
            .path(endpoint(2001))?
            .context("verified path should exist")?;

        assert!(path.latency.is_some());
        assert!(path.last_probe.is_none());

        Ok(())
    }

    #[test]
    fn replace_removes_missing_peers_and_keeps_updated_peers() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.replace(vec![
            device_info(public_key(1), "100.64.0.1/32", endpoint(1001)),
            device_info(public_key(2), "100.64.0.2/32", endpoint(1002)),
        ])?;

        let first = peer(&table, endpoint(1001))?;
        table.replace(vec![
            device_info(public_key(1), "100.64.0.10/32", endpoint(1010)),
            device_info(public_key(3), "100.64.0.3/32", endpoint(1003)),
        ])?;

        let updated_first = peer(&table, endpoint(1010))?;
        assert!(Arc::ptr_eq(&first, &updated_first));
        assert_eq!(table.all()?.len(), 2);
        assert!(table.find_by_endpoint(endpoint(1002))?.is_none());
        assert!(table.find_by_destination("100.64.0.10".parse()?)?.is_some());
        assert!(table.find_by_destination("100.64.0.3".parse()?)?.is_some());

        Ok(())
    }

    #[test]
    fn replace_preserves_verified_runtime_path_for_updated_peer() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        let peer = peer(&table, endpoint(1001))?;

        peer.record_endpoint_probe(endpoint(2001))?;
        assert!(peer.confirm_endpoint(endpoint(2001))?);
        table.replace(vec![device_info(
            public_key(1),
            "100.64.0.10/32",
            endpoint(1010),
        )])?;

        assert!(table.find_by_endpoint(endpoint(1001))?.is_none());
        assert!(table.find_by_endpoint(endpoint(1010))?.is_some());
        assert!(table.find_by_endpoint(endpoint(2001))?.is_some());
        assert_eq!(peer.selected_endpoint()?, endpoint(2001));

        Ok(())
    }

    #[test]
    fn replace_assigns_incrementing_indexes_to_new_peers() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.replace(vec![
            device_info(public_key(1), "100.64.0.1/32", endpoint(1001)),
            device_info(public_key(2), "100.64.0.2/32", endpoint(1002)),
        ])?;

        assert!(table.find_by_index(1)?.is_some());
        assert!(table.find_by_index(2)?.is_some());
        assert!(table.find_by_index(3)?.is_none());

        Ok(())
    }

    #[test]
    fn upsert_rejects_peer_without_endpoint() {
        let table = PeerTable::new(private_key());
        let result = table.upsert(DeviceInfo {
            device_id: DeviceID::random(),
            public_key: public_key(1),
            addresses: vec!["100.64.0.1/32".parse().unwrap()],
            endpoints: Vec::new(),
        });

        assert!(result.is_err());
    }
}
