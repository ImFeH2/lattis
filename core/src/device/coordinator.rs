use anyhow::{Result, bail, ensure};
use futures_util::StreamExt;
use if_addrs::{Interface, get_if_addrs};
use reqwest::Url;
use reqwest_eventsource::{Event, EventSource, ReadyState};
use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr, SocketAddrV6},
};

use crate::model::{DeviceInfo, RegisterDeviceRequest, RegisterDeviceResponse};

pub(super) struct CoordinatorClient {
    client: reqwest::Client,
    base_url: Url,
}

pub(super) enum PeerEvent {
    Peer(DeviceInfo),
    Peers(Vec<DeviceInfo>),
}

pub(super) struct PeerEventStream {
    source: EventSource,
}

impl CoordinatorClient {
    pub(super) fn new(base_url: Url) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    pub(super) async fn register(
        &self,
        request: RegisterDeviceRequest,
    ) -> Result<RegisterDeviceResponse> {
        let response = self
            .client
            .post(self.url("/devices/register")?)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(response)
    }

    pub(super) fn peer_events(&self, device_id: &str) -> Result<PeerEventStream> {
        let url = self.url(&format!("/devices/{device_id}/peers/events"))?;

        Ok(PeerEventStream {
            source: EventSource::get(url),
        })
    }

    pub(super) fn local_endpoints(&self, listen_port: u16) -> Result<Vec<SocketAddr>> {
        let mut endpoints = Vec::new();
        let mut seen = HashSet::new();

        for interface in get_if_addrs()? {
            if !is_endpoint_interface(&interface) {
                continue;
            }

            let Some(endpoint) = endpoint_from_interface(&interface, listen_port) else {
                continue;
            };

            if seen.insert(endpoint) {
                endpoints.push(endpoint);
            }
        }

        ensure!(
            !endpoints.is_empty(),
            "Device has no local endpoint address to register"
        );

        Ok(endpoints)
    }

    fn url(&self, path: &str) -> Result<Url> {
        Ok(self.base_url.join(path)?)
    }
}

impl PeerEventStream {
    pub(super) async fn next(&mut self) -> Result<Option<PeerEvent>> {
        loop {
            match self.source.next().await {
                Some(Ok(Event::Open)) => {}
                Some(Ok(Event::Message(message))) => {
                    if let Some(event) = parse_peer_event(&message.event, &message.data)? {
                        return Ok(Some(event));
                    }
                }
                Some(Err(error)) => {
                    if self.source.ready_state() == ReadyState::Closed {
                        bail!(error);
                    }
                    eprintln!("Coordinator peer event stream error: {error}");
                }
                None => return Ok(None),
            }
        }
    }
}

fn parse_peer_event(event: &str, data: &str) -> Result<Option<PeerEvent>> {
    match event {
        "peer" => Ok(Some(PeerEvent::Peer(serde_json::from_str(data)?))),
        "peers" => Ok(Some(PeerEvent::Peers(serde_json::from_str(data)?))),
        "error" => bail!("{data}"),
        _ => Ok(None),
    }
}

fn endpoint_from_interface(interface: &Interface, listen_port: u16) -> Option<SocketAddr> {
    let ip = interface.ip();

    if !is_endpoint_ip(ip) {
        return None;
    }

    match ip {
        IpAddr::V4(address) => Some(SocketAddr::new(IpAddr::V4(address), listen_port)),
        IpAddr::V6(address) => Some(SocketAddr::V6(SocketAddrV6::new(
            address,
            listen_port,
            0,
            ipv6_scope_id(interface),
        ))),
    }
}

fn ipv6_scope_id(interface: &Interface) -> u32 {
    if interface.is_link_local() {
        interface.index.unwrap_or_default()
    } else {
        0
    }
}

fn is_endpoint_interface(interface: &Interface) -> bool {
    interface.is_oper_up() && !interface.is_loopback() && !interface.is_p2p()
}

fn is_endpoint_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(address) => {
            !address.is_unspecified() && !address.is_loopback() && !address.is_multicast()
        }
        IpAddr::V6(address) => {
            !address.is_unspecified() && !address.is_loopback() && !address.is_multicast()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;
    use crate::model::{DeviceID, RegisterDeviceResponse};
    use if_addrs::{IfAddr, IfOperStatus, Ifv4Addr, Ifv6Addr};

    fn interface(ip: IpAddr, oper_status: IfOperStatus, is_p2p: bool) -> Interface {
        Interface {
            name: "eth0".to_string(),
            addr: match ip {
                IpAddr::V4(ip) => IfAddr::V4(Ifv4Addr {
                    ip,
                    netmask: Ipv4Addr::new(255, 255, 255, 0),
                    prefixlen: 24,
                    broadcast: None,
                }),
                IpAddr::V6(ip) => IfAddr::V6(Ifv6Addr {
                    ip,
                    netmask: Ipv6Addr::from(u128::MAX),
                    prefixlen: 128,
                    broadcast: None,
                }),
            },
            index: Some(42),
            oper_status,
            is_p2p,
        }
    }

    fn device_info() -> DeviceInfo {
        DeviceInfo {
            device_id: DeviceID::random(),
            public_key: crate::model::PublicKey::from([1; 32]),
            addresses: vec!["100.64.0.1/32".parse().unwrap()],
            endpoints: vec![SocketAddr::from(([192, 0, 2, 1], 1001))],
        }
    }

    #[test]
    fn coordinator_client_joins_paths_against_base_url() -> Result<()> {
        let client = CoordinatorClient::new(Url::parse("http://127.0.0.1:52170/base")?);

        assert_eq!(
            client.url("/devices/register")?.as_str(),
            "http://127.0.0.1:52170/devices/register"
        );
        assert_eq!(
            client.url("devices/register")?.as_str(),
            "http://127.0.0.1:52170/devices/register"
        );

        Ok(())
    }

    #[test]
    fn parse_peer_event_returns_single_peer() -> Result<()> {
        let peer = device_info();
        let data = serde_json::to_string(&peer)?;
        let event = parse_peer_event("peer", &data)?;

        let Some(PeerEvent::Peer(parsed)) = event else {
            panic!("expected peer event");
        };
        assert_eq!(parsed, peer);

        Ok(())
    }

    #[test]
    fn parse_peer_event_returns_peer_list() -> Result<()> {
        let peer = device_info();
        let data = serde_json::to_string(&vec![peer.clone()])?;
        let event = parse_peer_event("peers", &data)?;

        let Some(PeerEvent::Peers(parsed)) = event else {
            panic!("expected peers event");
        };
        assert_eq!(parsed, vec![peer]);

        Ok(())
    }

    #[test]
    fn parse_peer_event_ignores_unknown_event() -> Result<()> {
        assert!(parse_peer_event("keep-alive", "")?.is_none());
        Ok(())
    }

    #[test]
    fn parse_peer_event_returns_error_event_as_error() {
        assert!(parse_peer_event("error", "registration failed").is_err());
    }

    #[test]
    fn parse_peer_event_rejects_invalid_json() {
        assert!(parse_peer_event("peer", "{").is_err());
    }

    #[test]
    fn endpoint_ip_filter_rejects_non_endpoint_addresses() {
        assert!(!is_endpoint_ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        assert!(!is_endpoint_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_endpoint_ip(IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1))));
        assert!(!is_endpoint_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        assert!(!is_endpoint_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_endpoint_ip(IpAddr::V6(Ipv6Addr::from(
            0xff02_0000_0000_0000_0000_0000_0000_0001_u128,
        ))));
    }

    #[test]
    fn endpoint_ip_filter_accepts_regular_addresses() {
        assert!(is_endpoint_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        assert!(is_endpoint_ip(IpAddr::V6(Ipv6Addr::from(
            0x2001_0db8_0000_0000_0000_0000_0000_0001_u128,
        ))));
    }

    #[test]
    fn endpoint_interface_filter_requires_usable_interface() {
        assert!(is_endpoint_interface(&interface(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            IfOperStatus::Up,
            false,
        )));
        assert!(!is_endpoint_interface(&interface(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            IfOperStatus::Down,
            false,
        )));
        assert!(!is_endpoint_interface(&interface(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IfOperStatus::Up,
            false,
        )));
        assert!(!is_endpoint_interface(&interface(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            IfOperStatus::Up,
            true,
        )));
    }

    #[test]
    fn endpoint_from_interface_builds_ipv4_endpoint() {
        let interface = interface(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            IfOperStatus::Up,
            false,
        );

        assert_eq!(
            endpoint_from_interface(&interface, 52171),
            Some(SocketAddr::from(([192, 0, 2, 1], 52171)))
        );
    }

    #[test]
    fn endpoint_from_interface_adds_scope_for_ipv6_link_local_address() {
        let interface = interface(
            IpAddr::V6(Ipv6Addr::from(
                0xfe80_0000_0000_0000_0000_0000_0000_0001_u128,
            )),
            IfOperStatus::Up,
            false,
        );

        assert_eq!(
            endpoint_from_interface(&interface, 52171),
            Some(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(0xfe80_0000_0000_0000_0000_0000_0000_0001_u128),
                52171,
                0,
                42,
            )))
        );
    }

    #[test]
    fn endpoint_from_interface_skips_unusable_ip() {
        let interface = interface(IpAddr::V4(Ipv4Addr::LOCALHOST), IfOperStatus::Up, false);

        assert!(endpoint_from_interface(&interface, 52171).is_none());
    }

    #[test]
    fn register_device_response_json_is_valid_peer_event_data() -> Result<()> {
        let peer = device_info();
        let response = RegisterDeviceResponse {
            device: peer.clone(),
            peers: vec![peer.clone()],
        };

        let device_event = parse_peer_event("peer", &serde_json::to_string(&response.device)?)?;
        let peers_event = parse_peer_event("peers", &serde_json::to_string(&response.peers)?)?;

        assert!(matches!(device_event, Some(PeerEvent::Peer(_))));
        assert!(matches!(peers_event, Some(PeerEvent::Peers(_))));

        Ok(())
    }
}
