use anyhow::{Context, Result};
use boringtun::x25519::StaticSecret;
use etherparse::IpSlice;
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    task::JoinHandle,
    time::{Duration, interval},
};

use super::{
    coordinator::{CoordinatorClient, PeerEvent},
    packet::PacketDevice,
    peer::PeerTable,
    route::{RouteGuard, add_lattis_network_route},
    tun::open_tun_device,
    wireguard::{
        EndpointUpdate, MTU, WIREGUARD_PACKET_BUFFER_SIZE, WireGuardIo, packet_receiver_index,
    },
};
use crate::model::{DeviceID, DeviceInfo, RegisterDeviceRequest};

const WIREGUARD_TIMER_INTERVAL: Duration = Duration::from_millis(250);

pub struct Device {
    _route: RouteGuard,
    info: DeviceInfo,
    peers: Arc<PeerTable>,
    wireguard: Arc<WireGuardIo>,
    peer_events: JoinHandle<Result<()>>,
    outbound: JoinHandle<Result<()>>,
    inbound: JoinHandle<Result<()>>,
    timer: JoinHandle<Result<()>>,
}

struct DeviceTasks {
    wireguard: Arc<WireGuardIo>,
    peer_events: JoinHandle<Result<()>>,
    outbound: JoinHandle<Result<()>>,
    inbound: JoinHandle<Result<()>>,
    timer: JoinHandle<Result<()>>,
}

impl Device {
    pub(super) async fn start(
        interface_name: String,
        listen_port: u16,
        coordinator: CoordinatorClient,
    ) -> Result<Self> {
        let private_key = StaticSecret::random_from_rng(rand_core::OsRng);
        let device_id = crate::model::DeviceID::random();
        let endpoints = coordinator.local_endpoints(listen_port)?;

        let registration = coordinator
            .register(RegisterDeviceRequest {
                device_id: device_id.clone(),
                public_key: crate::model::PublicKey::from(&private_key),
                endpoints,
            })
            .await?;
        let peer_events = coordinator.peer_events(&device_id.to_string())?;
        let packet_device =
            open_tun_device(&interface_name, registration.device.addresses.clone())?;
        let route = add_lattis_network_route(&packet_device).await?;

        let info = registration.device;
        let peers = Arc::new(PeerTable::new(private_key));
        peers.replace(registration.peers)?;

        let tasks = Self::spawn_runtime(
            listen_port,
            peers.clone(),
            peer_events,
            Arc::new(packet_device),
        )
        .await?;

        Ok(Self::from_runtime(info, peers, route, tasks))
    }

    pub fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    pub fn peers(&self) -> Result<Vec<DeviceInfo>> {
        self.peers.all_infos()
    }

    pub async fn probe_peer(&self, device_id: &DeviceID) -> Result<()> {
        let peer = self
            .peers
            .find_by_device_id(device_id)?
            .context("Peer not found")?;

        self.wireguard
            .probe_endpoint(&peer, peer.selected_endpoint()?)
            .await
    }

    pub async fn probe_peer_endpoint(
        &self,
        device_id: &DeviceID,
        endpoint: SocketAddr,
    ) -> Result<()> {
        let peer = self
            .peers
            .find_by_device_id(device_id)?
            .context("Peer not found")?;

        self.wireguard.probe_endpoint(&peer, endpoint).await
    }

    fn from_runtime(
        info: DeviceInfo,
        peers: Arc<PeerTable>,
        route: RouteGuard,
        tasks: DeviceTasks,
    ) -> Self {
        Self {
            _route: route,
            info,
            peers,
            wireguard: tasks.wireguard,
            peer_events: tasks.peer_events,
            outbound: tasks.outbound,
            inbound: tasks.inbound,
            timer: tasks.timer,
        }
    }

    async fn spawn_runtime(
        listen_port: u16,
        peers: Arc<PeerTable>,
        mut peer_events: super::coordinator::PeerEventStream,
        packet_device: Arc<dyn PacketDevice>,
    ) -> Result<DeviceTasks> {
        let wireguard = Arc::new(WireGuardIo::bind(listen_port, packet_device).await?);
        probe_peer_endpoints(&peers, &wireguard).await?;

        let peer_events = {
            let peers = peers.clone();
            let wireguard = wireguard.clone();

            tokio::spawn(async move {
                while let Some(event) = peer_events.next().await? {
                    match event {
                        PeerEvent::Peer(peer) => peers.upsert(peer)?,
                        PeerEvent::Peers(peer_list) => peers.replace(peer_list)?,
                    }
                    probe_peer_endpoints(&peers, &wireguard).await?;
                }

                Ok(())
            })
        };

        let outbound = {
            let wireguard = wireguard.clone();
            let peers = peers.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; MTU];
                loop {
                    let len = wireguard.recv_packet(&mut buf).await?;
                    let raw_packet = &buf[..len];
                    let ip_packet = IpSlice::from_slice(raw_packet)?;
                    let dst = ip_packet.destination_addr();

                    if let Some(peer) = peers.find_by_destination(dst)? {
                        wireguard.encapsulate_packet(peer, raw_packet).await?;
                    }
                }
            })
        };

        let inbound = {
            let wireguard = wireguard.clone();
            let peers = peers.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];
                loop {
                    let (len, src) = wireguard.recv_datagram(&mut buf).await?;
                    let raw_packet = &buf[..len];

                    if let Some(peer) = peers.find_by_endpoint(src)? {
                        wireguard
                            .decapsulate_datagram(
                                peer,
                                raw_packet,
                                src,
                                EndpointUpdate::VerifiedPacket,
                                true,
                            )
                            .await?;
                        continue;
                    }

                    if let Some(index) = packet_receiver_index(raw_packet)
                        && let Some(peer) = peers.find_by_index(index >> 8)?
                    {
                        wireguard
                            .decapsulate_datagram(
                                peer,
                                raw_packet,
                                src,
                                EndpointUpdate::VerifiedPacket,
                                true,
                            )
                            .await?;
                        continue;
                    }

                    let mut handled = false;
                    for peer in peers.all()? {
                        if wireguard
                            .decapsulate_datagram(
                                peer,
                                raw_packet,
                                src,
                                EndpointUpdate::GeneratedHandshakeResponse,
                                false,
                            )
                            .await?
                        {
                            handled = true;
                            break;
                        }
                    }

                    if !handled {
                        eprintln!("Received packet from unknown peer: {}", src);
                    }
                }
            })
        };

        let timer = {
            let wireguard = wireguard.clone();
            let peers = peers.clone();

            tokio::spawn(async move {
                let mut interval = interval(WIREGUARD_TIMER_INTERVAL);

                loop {
                    interval.tick().await;

                    for peer in peers.all()? {
                        wireguard.update_timers(peer).await?;
                    }
                }
            })
        };

        Ok(DeviceTasks {
            wireguard,
            peer_events,
            outbound,
            inbound,
            timer,
        })
    }
}

async fn probe_peer_endpoints(peers: &PeerTable, wireguard: &WireGuardIo) -> Result<()> {
    for peer in peers.all()? {
        for endpoint in peer.endpoints_to_probe()? {
            wireguard.probe_endpoint(&peer, endpoint).await?;
        }
    }

    Ok(())
}

impl Drop for Device {
    fn drop(&mut self) {
        self.peer_events.abort();
        self.outbound.abort();
        self.inbound.abort();
        self.timer.abort();
    }
}
