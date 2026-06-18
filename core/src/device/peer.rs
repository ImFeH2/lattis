use anyhow::{Context, Result, anyhow};
use boringtun::{noise::Tunn, x25519::StaticSecret};
use ipnet::IpNet;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, RwLock, RwLockReadGuard},
};

use crate::model::{PeerInfo, PublicKey};

pub(super) struct Peer {
    pub(super) index: u32,
    allowed_ips: RwLock<Vec<IpNet>>,
    endpoint: RwLock<SocketAddr>,
    pub(super) tunnel: Mutex<Tunn>,
}

pub(super) struct PeerTable {
    private_key: StaticSecret,
    peers: RwLock<HashMap<PublicKey, Arc<Peer>>>,
}

impl Peer {
    fn from_info(index: u32, private_key: &StaticSecret, info: PeerInfo) -> Result<Self> {
        let endpoint = *info
            .endpoints
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
            allowed_ips: RwLock::new(info.virtual_addresses),
            endpoint: RwLock::new(endpoint),
            tunnel: Mutex::new(tunnel),
        })
    }

    pub(super) fn endpoint(&self) -> Result<SocketAddr> {
        Ok(*self
            .endpoint
            .read()
            .map_err(|_| anyhow!("WireGuard peer endpoint lock error"))?)
    }

    pub(super) fn update_endpoint(&self, endpoint: SocketAddr) -> Result<()> {
        *self
            .endpoint
            .write()
            .map_err(|_| anyhow!("WireGuard peer endpoint lock error"))? = endpoint;
        Ok(())
    }

    pub(super) fn allows_ip(&self, address: IpAddr) -> Result<bool> {
        let allowed_ips = self
            .allowed_ips
            .read()
            .map_err(|_| anyhow!("WireGuard peer allowed IPs lock error"))?;

        Ok(allowed_ips.iter().any(|net| net.contains(&address)))
    }

    fn update(&self, info: PeerInfo) -> Result<()> {
        let endpoint = *info
            .endpoints
            .first()
            .context("Coordinator peer has no endpoint")?;

        *self
            .allowed_ips
            .write()
            .map_err(|_| anyhow!("WireGuard peer allowed IPs lock error"))? =
            info.virtual_addresses;
        *self
            .endpoint
            .write()
            .map_err(|_| anyhow!("WireGuard peer endpoint lock error"))? = endpoint;

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

    pub(super) fn replace(&self, peer_infos: Vec<PeerInfo>) -> Result<()> {
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

    pub(super) fn upsert(&self, info: PeerInfo) -> Result<()> {
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

    fn peer_info(public_key: PublicKey, address: &str, endpoint: SocketAddr) -> PeerInfo {
        PeerInfo {
            device_id: DeviceID::random(),
            public_key,
            virtual_addresses: vec![address.parse().unwrap()],
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
        table.upsert(peer_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;

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
        table.upsert(peer_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;

        assert!(table.find_by_destination("100.64.0.2".parse()?)?.is_none());

        Ok(())
    }

    #[test]
    fn upsert_updates_existing_peer_in_place() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.upsert(peer_info(public_key(1), "100.64.0.1/32", endpoint(1001)))?;
        table.upsert(peer_info(public_key(1), "100.64.0.2/32", endpoint(1002)))?;

        assert_eq!(table.all()?.len(), 1);
        assert!(table.find_by_destination("100.64.0.1".parse()?)?.is_none());
        assert!(table.find_by_destination("100.64.0.2".parse()?)?.is_some());
        assert!(table.find_by_endpoint(endpoint(1001))?.is_none());
        assert!(table.find_by_endpoint(endpoint(1002))?.is_some());
        assert!(table.find_by_index(1)?.is_some());

        Ok(())
    }

    #[test]
    fn replace_removes_missing_peers_and_keeps_updated_peers() -> Result<()> {
        let table = PeerTable::new(private_key());
        table.replace(vec![
            peer_info(public_key(1), "100.64.0.1/32", endpoint(1001)),
            peer_info(public_key(2), "100.64.0.2/32", endpoint(1002)),
        ])?;

        let first = table
            .find_by_endpoint(endpoint(1001))?
            .expect("peer exists");
        table.replace(vec![
            peer_info(public_key(1), "100.64.0.10/32", endpoint(1010)),
            peer_info(public_key(3), "100.64.0.3/32", endpoint(1003)),
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
            peer_info(public_key(1), "100.64.0.1/32", endpoint(1001)),
            peer_info(public_key(2), "100.64.0.2/32", endpoint(1002)),
        ])?;

        assert!(table.find_by_index(1)?.is_some());
        assert!(table.find_by_index(2)?.is_some());
        assert!(table.find_by_index(3)?.is_none());

        Ok(())
    }

    #[test]
    fn upsert_rejects_peer_without_endpoint() {
        let table = PeerTable::new(private_key());
        let result = table.upsert(PeerInfo {
            device_id: DeviceID::random(),
            public_key: public_key(1),
            virtual_addresses: vec!["100.64.0.1/32".parse().unwrap()],
            endpoints: Vec::new(),
        });

        assert!(result.is_err());
    }
}
