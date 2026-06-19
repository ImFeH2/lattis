use anyhow::{Context, Result, anyhow};
use boringtun::{noise::Tunn, x25519::StaticSecret};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, RwLock, RwLockReadGuard},
};

use crate::model::{DeviceInfo, PublicKey};

pub(super) struct Peer {
    pub(super) index: u32,
    info: RwLock<DeviceInfo>,
    pub(super) tunnel: Mutex<Tunn>,
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
            tunnel: Mutex::new(tunnel),
        })
    }

    pub(super) fn endpoint(&self) -> Result<SocketAddr> {
        self.info
            .read()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?
            .endpoints
            .first()
            .copied()
            .context("Coordinator peer has no endpoint")
    }

    pub(super) fn update_endpoint(&self, endpoint: SocketAddr) -> Result<()> {
        let mut info = self
            .info
            .write()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?;

        match info.endpoints.first_mut() {
            Some(current) => *current = endpoint,
            None => info.endpoints.push(endpoint),
        }

        Ok(())
    }

    pub(super) fn allows_ip(&self, address: IpAddr) -> Result<bool> {
        let info = self
            .info
            .read()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?;

        Ok(info.addresses.iter().any(|net| net.contains(&address)))
    }

    pub(super) fn info(&self) -> Result<DeviceInfo> {
        Ok(self
            .info
            .read()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))?
            .clone())
    }

    fn update(&self, info: DeviceInfo) -> Result<()> {
        info.endpoints
            .first()
            .context("Coordinator peer has no endpoint")?;

        *self
            .info
            .write()
            .map_err(|_| anyhow!("WireGuard peer info lock error"))? = info;

        Ok(())
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
            if peer.allows_ip(dst)? {
                return Ok(Some(peer.clone()));
            }
        }

        Ok(None)
    }

    pub(super) fn find_by_endpoint(&self, endpoint: SocketAddr) -> Result<Option<Arc<Peer>>> {
        let peers = self.read()?;

        for peer in peers.values() {
            if peer.endpoint()? == endpoint {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceID;

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

    #[test]
    fn upsert_adds_peer_and_finds_it_by_destination_endpoint_and_index() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;

        let by_destination = table.find_by_destination("100.64.0.1".parse()?)?;
        let by_endpoint = table.find_by_endpoint(endpoint(1001))?;
        let by_index = table.find_by_index(1)?;

        assert!(by_destination.is_some());
        assert!(by_endpoint.is_some());
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
    fn endpoint_update_is_reflected_in_device_info() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(device_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        let peer = table
            .find_by_endpoint(endpoint(1001))?
            .expect("peer exists");

        peer.update_endpoint(endpoint(2001))?;

        let info = peer.info()?;
        assert_eq!(info.endpoints[0], endpoint(2001));
        assert_eq!(table.all_infos()?[0].endpoints[0], endpoint(2001));

        Ok(())
    }

    #[test]
    fn replace_removes_missing_peers_and_keeps_updated_peers() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.replace(vec![
            device_info(public_key(1), "100.64.0.1/32", endpoint(1001)),
            device_info(public_key(2), "100.64.0.2/32", endpoint(1002)),
        ])?;

        let first = table
            .find_by_endpoint(endpoint(1001))?
            .expect("peer exists");
        table.replace(vec![
            device_info(public_key(1), "100.64.0.10/32", endpoint(1010)),
            device_info(public_key(3), "100.64.0.3/32", endpoint(1003)),
        ])?;

        let updated_first = table
            .find_by_endpoint(endpoint(1010))?
            .expect("updated peer exists");
        assert!(Arc::ptr_eq(&first, &updated_first));
        assert_eq!(table.all()?.len(), 2);
        assert!(table.find_by_endpoint(endpoint(1002))?.is_none());
        assert!(table.find_by_destination("100.64.0.10".parse()?)?.is_some());
        assert!(table.find_by_destination("100.64.0.3".parse()?)?.is_some());

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
