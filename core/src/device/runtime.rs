use anyhow::{Result, anyhow};
use boringtun::{noise::TunnResult, x25519::StaticSecret};
use etherparse::IpSlice;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    net::UdpSocket,
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
        EndpointUpdate, MTU, WIREGUARD_PACKET_BUFFER_SIZE, handle_peer_datagram,
        packet_receiver_index,
    },
};
use crate::model::{PeerInfo, RegisterDeviceRequest};

const WIREGUARD_TIMER_INTERVAL: Duration = Duration::from_millis(250);

pub struct Device {
    _route: RouteGuard,
    peer_events: JoinHandle<Result<()>>,
    outbound: JoinHandle<Result<()>>,
    inbound: JoinHandle<Result<()>>,
    timer: JoinHandle<Result<()>>,
}

struct DeviceTasks {
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
            open_tun_device(&interface_name, registration.device.virtual_addresses)?;
        let route = add_lattis_network_route(&packet_device).await?;

        let tasks = Self::spawn_runtime(
            listen_port,
            private_key,
            registration.peers,
            peer_events,
            Arc::new(packet_device),
        )
        .await?;

        Ok(Self::from_runtime(route, tasks))
    }

    fn from_runtime(route: RouteGuard, tasks: DeviceTasks) -> Self {
        Self {
            _route: route,
            peer_events: tasks.peer_events,
            outbound: tasks.outbound,
            inbound: tasks.inbound,
            timer: tasks.timer,
        }
    }

    async fn spawn_runtime(
        listen_port: u16,
        private_key: StaticSecret,
        initial_peers: Vec<PeerInfo>,
        mut peer_events: super::coordinator::PeerEventStream,
        packet_device: Arc<dyn PacketDevice>,
    ) -> Result<DeviceTasks> {
        let socket_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen_port);
        let socket = UdpSocket::bind(&socket_address).await?;

        let peers = Arc::new(PeerTable::new(private_key));
        peers.replace(initial_peers)?;
        let socket = Arc::new(socket);

        let peer_events = {
            let peers = peers.clone();

            tokio::spawn(async move {
                while let Some(event) = peer_events.next().await? {
                    match event {
                        PeerEvent::Peer(peer) => peers.upsert(peer)?,
                        PeerEvent::Peers(peer_list) => peers.replace(peer_list)?,
                    }
                }

                Ok(())
            })
        };

        let outbound = {
            let packet_device = packet_device.clone();
            let socket = socket.clone();
            let peers = peers.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; MTU];
                loop {
                    let len = packet_device.recv(&mut buf).await?;
                    let raw_packet = &buf[..len];
                    let ip_packet = IpSlice::from_slice(raw_packet)?;
                    let dst = ip_packet.destination_addr();

                    let peer = peers.find_by_destination(dst)?;

                    if let Some(peer) = peer {
                        let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];

                        let result = {
                            let mut tunnel = peer
                                .tunnel
                                .lock()
                                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

                            tunnel.encapsulate(raw_packet, &mut out_buf)
                        };

                        match result {
                            TunnResult::WriteToNetwork(packet) => {
                                let endpoint = peer.endpoint()?;
                                socket.send_to(packet, endpoint).await?;
                            }
                            TunnResult::Done => {}
                            TunnResult::Err(err) => {
                                eprintln!("WireGuard outbound error: {:?}", err);
                            }
                            TunnResult::WriteToTunnelV4(_, _)
                            | TunnResult::WriteToTunnelV6(_, _) => {
                                eprintln!("Unexpected tunnel output type");
                            }
                        }
                    }
                }
            })
        };

        let inbound = {
            let packet_device = packet_device.clone();
            let socket = socket.clone();
            let peers = peers.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];
                loop {
                    let (len, src) = socket.recv_from(&mut buf).await?;
                    let raw_packet = &buf[..len];

                    if let Some(peer) = peers.find_by_endpoint(src)? {
                        handle_peer_datagram(
                            peer,
                            raw_packet,
                            src,
                            packet_device.as_ref(),
                            &socket,
                            EndpointUpdate::None,
                            true,
                        )
                        .await?;
                        continue;
                    }

                    if let Some(index) = packet_receiver_index(raw_packet)
                        && let Some(peer) = peers.find_by_index(index >> 8)?
                    {
                        handle_peer_datagram(
                            peer,
                            raw_packet,
                            src,
                            packet_device.as_ref(),
                            &socket,
                            EndpointUpdate::VerifiedPacket,
                            true,
                        )
                        .await?;
                        continue;
                    }

                    let mut handled = false;
                    for peer in peers.all()? {
                        if handle_peer_datagram(
                            peer,
                            raw_packet,
                            src,
                            packet_device.as_ref(),
                            &socket,
                            EndpointUpdate::HandshakeResponse,
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
            let socket = socket.clone();
            let peers = peers.clone();

            tokio::spawn(async move {
                let mut interval = interval(WIREGUARD_TIMER_INTERVAL);

                loop {
                    interval.tick().await;

                    for peer in peers.all()? {
                        let mut out_buf = [0u8; WIREGUARD_PACKET_BUFFER_SIZE];

                        let result = {
                            let mut tunnel = peer
                                .tunnel
                                .lock()
                                .map_err(|_| anyhow!("WireGuard tunnel mutex error"))?;

                            tunnel.update_timers(&mut out_buf)
                        };

                        match result {
                            TunnResult::WriteToNetwork(packet) => {
                                let endpoint = peer.endpoint()?;
                                socket.send_to(packet, endpoint).await?;
                            }
                            TunnResult::Done => {}
                            TunnResult::Err(err) => {
                                eprintln!("WireGuard timer error: {:?}", err);
                            }
                            TunnResult::WriteToTunnelV4(_, _)
                            | TunnResult::WriteToTunnelV6(_, _) => {
                                eprintln!("Unexpected timer output type");
                            }
                        }
                    }
                }
            })
        };

        Ok(DeviceTasks {
            peer_events,
            outbound,
            inbound,
            timer,
        })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.peer_events.abort();
        self.outbound.abort();
        self.inbound.abort();
        self.timer.abort();
    }
}
