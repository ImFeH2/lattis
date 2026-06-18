use anyhow::{Result, bail, ensure};
use futures_util::StreamExt;
use if_addrs::{Interface, get_if_addrs};
use reqwest::Url;
use reqwest_eventsource::{Event, EventSource, ReadyState};
use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr, SocketAddrV6},
};

use crate::model::{PeerInfo, RegisterDeviceRequest, RegisterDeviceResponse};

pub(super) struct CoordinatorClient {
    client: reqwest::Client,
    base_url: Url,
}

pub(super) enum PeerEvent {
    Peer(PeerInfo),
    Peers(Vec<PeerInfo>),
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
