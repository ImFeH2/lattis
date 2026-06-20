#![cfg(target_os = "linux")]

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, ensure};
use netlab::{Host, HostTask, Lan, NatType, Net, Router};
use tokio::{
    net::UdpSocket,
    sync::{Mutex, mpsc, oneshot},
    time::timeout,
};

const PACKET_TIMEOUT: Duration = Duration::from_secs(1);
const FIRST_NAT_PORT: u16 = 6000;
const PRIVATE_PORT: u16 = 5000;
const SECOND_NAT_PORT: u16 = 6001;

struct UdpNatFixture {
    external_host: Host,
    external_lan: Lan,
    other_external_host: Host,
    private_host: Host,
    private_lan: Lan,
    router: Router,
}

struct PrivatePeer {
    inbound: Mutex<mpsc::UnboundedReceiver<String>>,
    sender: mpsc::UnboundedSender<PrivatePeerCommand>,
    _task: HostTask<()>,
}

struct PrivatePeerCommand {
    destination: SocketAddrV4,
    payload: &'static str,
    result: oneshot::Sender<Result<(), String>>,
}

#[tokio::test]
async fn full_cone_allows_any_remote_after_mapping_exists() -> Result<()> {
    let scenario = UdpNatScenario::new(NatType::FullCone, 42).await?;

    let public_addr = scenario.send_from_private_to_first_remote().await?;
    let allowed = scenario
        .send_from_second_remote_to_private(public_addr, "from-second")
        .await?;

    assert_eq!(allowed.as_deref(), Some("from-second"));

    Ok(())
}

#[tokio::test]
async fn full_cone_rejects_unknown_public_ports() -> Result<()> {
    let scenario = UdpNatScenario::new(NatType::FullCone, 43).await?;

    let public_addr = scenario.send_from_private_to_first_remote().await?;
    let unknown_addr = SocketAddrV4::new(public_addr.ip().to_owned(), public_addr.port() + 10);
    let blocked = scenario
        .send_from_second_remote_to_private(unknown_addr, "unknown-port")
        .await?;

    assert!(blocked.is_none());

    Ok(())
}

#[tokio::test]
async fn restricted_cone_allows_known_address_from_different_port() -> Result<()> {
    let scenario = UdpNatScenario::new(NatType::RestrictedCone, 44).await?;

    let public_addr = scenario.send_from_private_to_first_remote().await?;
    let allowed = scenario
        .send_from_first_remote_alt_port_to_private(public_addr, "from-known-address")
        .await?;

    assert_eq!(allowed.as_deref(), Some("from-known-address"));

    Ok(())
}

#[tokio::test]
async fn restricted_cone_rejects_unknown_addresses() -> Result<()> {
    let scenario = UdpNatScenario::new(NatType::RestrictedCone, 45).await?;

    let public_addr = scenario.send_from_private_to_first_remote().await?;
    let blocked = scenario
        .send_from_second_remote_to_private(public_addr, "from-unknown-address")
        .await?;

    assert!(blocked.is_none());

    Ok(())
}

#[tokio::test]
async fn port_restricted_cone_allows_known_endpoint() -> Result<()> {
    let scenario = UdpNatScenario::new(NatType::PortRestrictedCone, 46).await?;

    let public_addr = scenario.send_from_private_to_first_remote().await?;
    let allowed = scenario
        .send_from_first_remote_to_private(public_addr, "from-known-endpoint")
        .await?;

    assert_eq!(allowed.as_deref(), Some("from-known-endpoint"));

    Ok(())
}

#[tokio::test]
async fn port_restricted_cone_rejects_known_address_with_different_port() -> Result<()> {
    let scenario = UdpNatScenario::new(NatType::PortRestrictedCone, 47).await?;

    let public_addr = scenario.send_from_private_to_first_remote().await?;
    let blocked = scenario
        .send_from_first_remote_alt_port_to_private(public_addr, "from-different-port")
        .await?;

    assert!(blocked.is_none());

    Ok(())
}

#[tokio::test]
async fn symmetric_nat_uses_destination_specific_mappings() -> Result<()> {
    let scenario = UdpNatScenario::new(NatType::Symmetric, 48).await?;

    let first_public_addr = scenario.send_from_private_to_first_remote().await?;
    let second_public_addr = scenario.send_from_private_to_second_remote().await?;

    assert_ne!(first_public_addr, second_public_addr);

    Ok(())
}

#[tokio::test]
async fn symmetric_nat_allows_matching_destination_endpoint() -> Result<()> {
    let scenario = UdpNatScenario::new(NatType::Symmetric, 49).await?;

    let public_addr = scenario.send_from_private_to_first_remote().await?;
    let allowed = scenario
        .send_from_first_remote_to_private(public_addr, "from-matching-endpoint")
        .await?;

    assert_eq!(allowed.as_deref(), Some("from-matching-endpoint"));

    Ok(())
}

#[tokio::test]
async fn symmetric_nat_rejects_different_remote_on_an_existing_mapping() -> Result<()> {
    let scenario = UdpNatScenario::new(NatType::Symmetric, 50).await?;

    let first_public_addr = scenario.send_from_private_to_first_remote().await?;
    let second_public_addr = scenario.send_from_private_to_second_remote().await?;
    let first_blocked = scenario
        .send_from_second_remote_to_private(first_public_addr, "wrong-second")
        .await?;
    let second_blocked = scenario
        .send_from_first_remote_to_private(second_public_addr, "wrong-first")
        .await?;

    assert!(first_blocked.is_none());
    assert!(second_blocked.is_none());

    Ok(())
}

impl UdpNatScenario {
    async fn new(nat_type: NatType, network_id: u8) -> Result<Self> {
        let fixture = UdpNatFixture::new(network_id).await?;
        let private_addr = fixture.private_host.join(&fixture.private_lan).await?;
        let external_addr = fixture.external_host.join(&fixture.external_lan).await?;
        let other_external_addr = fixture
            .other_external_host
            .join(&fixture.external_lan)
            .await?;
        let first_remote = SocketAddrV4::new(external_addr.addr(), 7000);
        let second_remote = SocketAddrV4::new(other_external_addr.addr(), 7000);

        let router_private_addr = fixture.router.attach(&fixture.private_lan).await?.addr();
        let router_public_addr = fixture.router.attach(&fixture.external_lan).await?.addr();

        fixture.private_lan.set_gateway(&fixture.router).await?;

        let private_peer = SocketAddrV4::new(private_addr.addr(), PRIVATE_PORT);
        fixture
            .router
            .enable_udp_nat(
                &fixture.private_lan,
                &fixture.external_lan,
                private_peer,
                nat_type,
                vec![
                    (FIRST_NAT_PORT, first_remote),
                    (SECOND_NAT_PORT, second_remote),
                ],
            )
            .await?;

        Ok(Self {
            external_host: fixture.external_host,
            first_remote,
            first_private_nat_addr: SocketAddrV4::new(router_private_addr, FIRST_NAT_PORT),
            other_external_host: fixture.other_external_host,
            private_peer_task: start_private_peer(&fixture.private_host, private_peer).await?,
            router_public_addr,
            second_remote,
            second_private_nat_addr: SocketAddrV4::new(router_private_addr, SECOND_NAT_PORT),
        })
    }

    async fn send_from_private_to_first_remote(&self) -> Result<SocketAddrV4> {
        self.send_from_private(self.first_remote, "to-first").await
    }

    async fn send_from_private_to_second_remote(&self) -> Result<SocketAddrV4> {
        self.send_from_private(self.second_remote, "to-second")
            .await
    }

    async fn send_from_private(
        &self,
        remote: SocketAddrV4,
        payload: &'static str,
    ) -> Result<SocketAddrV4> {
        let private_nat_addr = if remote == self.second_remote {
            self.second_private_nat_addr
        } else {
            self.first_private_nat_addr
        };
        let receiver = if remote == self.second_remote {
            &self.other_external_host
        } else {
            &self.external_host
        };
        let observed_source = receive_external_datagram_from_private(
            receiver,
            remote,
            &self.private_peer_task,
            private_nat_addr,
            payload,
        )
        .await?;

        ensure!(
            observed_source.ip() == &self.router_public_addr,
            "external host observed {}, expected router public address {}",
            observed_source,
            self.router_public_addr
        );

        Ok(observed_source)
    }

    async fn send_from_first_remote_to_private(
        &self,
        public_addr: SocketAddrV4,
        payload: &'static str,
    ) -> Result<Option<String>> {
        send_to_private_through_nat(
            &self.private_peer_task,
            &self.external_host,
            self.first_remote,
            public_addr,
            payload,
        )
        .await
    }

    async fn send_from_first_remote_alt_port_to_private(
        &self,
        public_addr: SocketAddrV4,
        payload: &'static str,
    ) -> Result<Option<String>> {
        let alternate_remote = SocketAddrV4::new(self.first_remote.ip().to_owned(), 7001);

        send_to_private_through_nat(
            &self.private_peer_task,
            &self.external_host,
            alternate_remote,
            public_addr,
            payload,
        )
        .await
    }

    async fn send_from_second_remote_to_private(
        &self,
        public_addr: SocketAddrV4,
        payload: &'static str,
    ) -> Result<Option<String>> {
        send_to_private_through_nat(
            &self.private_peer_task,
            &self.other_external_host,
            self.second_remote,
            public_addr,
            payload,
        )
        .await
    }
}

struct UdpNatScenario {
    external_host: Host,
    first_remote: SocketAddrV4,
    first_private_nat_addr: SocketAddrV4,
    other_external_host: Host,
    private_peer_task: PrivatePeer,
    router_public_addr: Ipv4Addr,
    second_remote: SocketAddrV4,
    second_private_nat_addr: SocketAddrV4,
}

impl UdpNatFixture {
    async fn new(network_id: u8) -> Result<Self> {
        let net = Net::new();

        Ok(Self {
            external_host: net.host().await?,
            external_lan: net.lan(ipv4_net(10, network_id, 2, 0, 24)).await?,
            other_external_host: net.host().await?,
            private_host: net.host().await?,
            private_lan: net.lan(ipv4_net(10, network_id, 1, 0, 24)).await?,
            router: net.router().await?,
        })
    }
}

async fn receive_external_datagram_from_private(
    receiver: &Host,
    receiver_addr: SocketAddrV4,
    sender: &PrivatePeer,
    destination: SocketAddrV4,
    payload: &'static str,
) -> Result<SocketAddrV4> {
    let (ready_tx, ready_rx) = oneshot::channel();
    let receiver_task = receiver.spawn(move || async move {
        let socket = UdpSocket::bind(receiver_addr).await?;
        let _ = ready_tx.send(());

        let mut buffer = [0; 128];
        let (len, source) = timeout(PACKET_TIMEOUT, socket.recv_from(&mut buffer)).await??;
        ensure!(
            &buffer[..len] == payload.as_bytes(),
            "unexpected payload: {}",
            String::from_utf8_lossy(&buffer[..len])
        );

        match source {
            std::net::SocketAddr::V4(source) => Ok(source),
            std::net::SocketAddr::V6(_) => Err(anyhow!("received IPv6 UDP packet")),
        }
    })?;

    ready_rx
        .await
        .context("receiver failed to bind UDP socket")?;

    sender.send(destination, payload).await?;
    receiver_task.await
}

async fn send_to_private_through_nat(
    private_peer: &PrivatePeer,
    external_host: &Host,
    external_addr: SocketAddrV4,
    public_addr: SocketAddrV4,
    payload: &'static str,
) -> Result<Option<String>> {
    let external_task = external_host.spawn(move || async move {
        let socket = UdpSocket::bind(external_addr).await?;
        socket.send_to(payload.as_bytes(), public_addr).await?;
        Ok(())
    })?;

    external_task.await?;
    private_peer.receive().await
}

async fn start_private_peer(host: &Host, addr: SocketAddrV4) -> Result<PrivatePeer> {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<PrivatePeerCommand>();
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<String>();
    let (ready_tx, ready_rx) = oneshot::channel();

    let task = host.spawn(move || async move {
        let socket = UdpSocket::bind(addr).await?;
        let _ = ready_tx.send(());
        let mut buffer = [0; 128];

        loop {
            tokio::select! {
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        break;
                    };
                    let result = socket
                        .send_to(command.payload.as_bytes(), command.destination)
                        .await
                        .map(|_| ())
                        .map_err(|err| err.to_string());
                    let _ = command.result.send(result);
                }
                result = socket.recv_from(&mut buffer) => {
                    let (len, _) = result?;
                    let payload = String::from_utf8(buffer[..len].to_vec())?;
                    let _ = inbound_tx.send(payload);
                }
            }
        }

        Ok(())
    })?;

    ready_rx
        .await
        .context("private peer failed to bind UDP socket")?;

    Ok(PrivatePeer {
        inbound: Mutex::new(inbound_rx),
        sender: command_tx,
        _task: task,
    })
}

impl PrivatePeer {
    async fn receive(&self) -> Result<Option<String>> {
        let mut inbound = self.inbound.lock().await;

        match timeout(PACKET_TIMEOUT, inbound.recv()).await {
            Ok(Some(payload)) => Ok(Some(payload)),
            Ok(None) => Err(anyhow!("private peer stopped before receiving UDP packet")),
            Err(_) => Ok(None),
        }
    }

    async fn send(&self, destination: SocketAddrV4, payload: &'static str) -> Result<()> {
        let (result_tx, result_rx) = oneshot::channel();

        self.sender
            .send(PrivatePeerCommand {
                destination,
                payload,
                result: result_tx,
            })
            .map_err(|_| anyhow!("private peer sender stopped"))?;

        result_rx
            .await
            .context("private peer stopped before reporting send result")?
            .map_err(|err| anyhow!(err))
    }
}

fn ipv4_net(a: u8, b: u8, c: u8, d: u8, prefix: u8) -> ipnet::Ipv4Net {
    ipnet::Ipv4Net::new(Ipv4Addr::new(a, b, c, d), prefix).expect("valid IPv4 network")
}
