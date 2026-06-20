#![cfg(target_os = "linux")]

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use netlab::{Host, Lan, Net, Router};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};

const REACHABILITY_TIMEOUT: Duration = Duration::from_secs(1);

struct NatFixture {
    external_host: Host,
    external_lan: Lan,
    private_host: Host,
    private_lan: Lan,
    router: Router,
}

#[tokio::test]
async fn masquerade_lans_block_direct_inbound_connections() -> Result<()> {
    let fixture = NatFixture::new("10.35.1.0/24", "10.35.2.0/24").await?;

    fixture.private_lan.set_gateway(&fixture.router).await?;
    fixture
        .router
        .enable_masquerade(&fixture.private_lan)
        .await?;
    fixture.router.attach(&fixture.external_lan).await?;
    fixture.external_lan.set_gateway(&fixture.router).await?;

    let private_addr = fixture.private_host.join(&fixture.private_lan).await?;
    fixture.external_host.join(&fixture.external_lan).await?;

    let inbound_result = fixture
        .external_host
        .assert_can_reach(&fixture.private_host, private_addr.addr())
        .await;
    assert!(inbound_result.is_err());

    Ok(())
}

#[tokio::test]
async fn repeated_masquerade_setup_keeps_outbound_connections_working() -> Result<()> {
    let fixture = NatFixture::new("10.39.1.0/24", "10.39.2.0/24").await?;

    fixture.private_lan.set_gateway(&fixture.router).await?;
    fixture
        .router
        .enable_masquerade(&fixture.private_lan)
        .await?;
    fixture
        .router
        .enable_masquerade(&fixture.private_lan)
        .await?;
    let router_external_addr = fixture.router.attach(&fixture.external_lan).await?;
    fixture.external_lan.set_gateway(&fixture.router).await?;

    fixture.private_host.join(&fixture.private_lan).await?;
    let external_addr = fixture.external_host.join(&fixture.external_lan).await?;

    let observed_peer = connect_and_observe_peer(
        &fixture.private_host,
        &fixture.external_host,
        external_addr.addr(),
    )
    .await?;

    assert_eq!(observed_peer.ip(), IpAddr::V4(router_external_addr.addr()));

    Ok(())
}

#[tokio::test]
async fn masquerade_setup_attaches_router_to_private_lan() -> Result<()> {
    let net = Net::new();
    let router = net.router().await?;
    let private_lan = net.lan("10.40.1.0/30".parse()?).await?;
    let host = net.host().await?;

    let router_addr = router.enable_masquerade(&private_lan).await?;
    let router_addr_again = router.attach(&private_lan).await?;
    let host_addr = host.join(&private_lan).await?;

    assert_eq!(router_addr, "10.40.1.1/30".parse()?);
    assert_eq!(router_addr_again, router_addr);
    assert_eq!(host_addr, "10.40.1.2/30".parse()?);

    Ok(())
}

#[tokio::test]
async fn repeated_masquerade_setup_does_not_enable_direct_inbound() -> Result<()> {
    let fixture = NatFixture::new("10.41.1.0/24", "10.41.2.0/24").await?;

    fixture.private_lan.set_gateway(&fixture.router).await?;
    fixture
        .router
        .enable_masquerade(&fixture.private_lan)
        .await?;
    let router_external_addr = fixture.router.attach(&fixture.external_lan).await?;
    fixture.external_lan.set_gateway(&fixture.router).await?;

    let private_addr = fixture.private_host.join(&fixture.private_lan).await?;
    let external_addr = fixture.external_host.join(&fixture.external_lan).await?;

    let first_inbound_result = fixture
        .external_host
        .assert_can_reach(&fixture.private_host, private_addr.addr())
        .await;
    assert!(first_inbound_result.is_err());

    fixture
        .router
        .enable_masquerade(&fixture.private_lan)
        .await?;

    let inbound_result = fixture
        .external_host
        .assert_can_reach(&fixture.private_host, private_addr.addr())
        .await;
    assert!(inbound_result.is_err());

    let observed_peer = connect_and_observe_peer(
        &fixture.private_host,
        &fixture.external_host,
        external_addr.addr(),
    )
    .await?;

    assert_eq!(observed_peer.ip(), IpAddr::V4(router_external_addr.addr()));

    Ok(())
}

impl NatFixture {
    async fn new(private_network: &str, external_network: &str) -> Result<Self> {
        let net = Net::new();

        Ok(Self {
            external_host: net.host().await?,
            external_lan: net.lan(external_network.parse()?).await?,
            private_host: net.host().await?,
            private_lan: net.lan(private_network.parse()?).await?,
            router: net.router().await?,
        })
    }
}

async fn connect_and_observe_peer(
    client: &Host,
    server: &Host,
    server_addr: impl Into<IpAddr>,
) -> Result<SocketAddr> {
    let server_addr = server_addr.into();
    let (port_tx, port_rx) = oneshot::channel();
    let (peer_tx, peer_rx) = oneshot::channel();

    let server_task = server.spawn(move || async move {
        let listener = TcpListener::bind(SocketAddr::new(server_addr, 0)).await?;
        let port = listener.local_addr()?.port();
        let _ = port_tx.send(port);

        let (_, peer_addr) = timeout(REACHABILITY_TIMEOUT, listener.accept()).await??;
        let _ = peer_tx.send(peer_addr);

        Ok(())
    })?;

    let port = match port_rx.await {
        Ok(port) => port,
        Err(_) => {
            server_task.await.context("peer listener failed to start")?;
            return Err(anyhow!("peer listener stopped before reporting port"));
        }
    };

    let server_socket = SocketAddr::new(server_addr, port);
    let client_task = client.spawn(move || async move {
        timeout(REACHABILITY_TIMEOUT, TcpStream::connect(server_socket)).await??;

        Ok(())
    })?;

    client_task.await?;
    let observed_peer = peer_rx
        .await
        .context("peer listener stopped before reporting peer address")?;
    server_task.await?;

    Ok(observed_peer)
}
