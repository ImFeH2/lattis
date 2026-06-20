#![cfg(target_os = "linux")]

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use netlab::{Host, NatType, Net};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};

const REACHABILITY_TIMEOUT: Duration = Duration::from_secs(1);

#[tokio::test]
async fn full_cone_allows_inbound_connections() -> Result<()> {
    let net = Net::new();
    let router = net.router().await?;
    let private_lan = net.lan("10.35.1.0/24".parse()?).await?;
    let external_lan = net.lan("10.35.2.0/24".parse()?).await?;
    let private_host = net.host().await?;
    let external_host = net.host().await?;

    private_lan.set_gateway(&router).await?;
    router.enable_nat(&private_lan, NatType::FullCone).await?;
    router.attach(&external_lan).await?;
    external_lan.set_gateway(&router).await?;

    let private_addr = private_host.join(&private_lan).await?;
    external_host.join(&external_lan).await?;

    external_host
        .assert_can_reach(&private_host, private_addr.addr())
        .await?;

    Ok(())
}

#[tokio::test]
async fn symmetric_masks_private_source_and_blocks_inbound_connections() -> Result<()> {
    let net = Net::new();
    let router = net.router().await?;
    let private_lan = net.lan("10.34.1.0/24".parse()?).await?;
    let external_lan = net.lan("10.34.2.0/24".parse()?).await?;
    let private_host = net.host().await?;
    let external_host = net.host().await?;

    private_lan.set_gateway(&router).await?;
    router.enable_nat(&private_lan, NatType::Symmetric).await?;
    router.enable_nat(&private_lan, NatType::Symmetric).await?;
    let router_external_addr = router.attach(&external_lan).await?;
    external_lan.set_gateway(&router).await?;

    let private_addr = private_host.join(&private_lan).await?;
    let external_addr = external_host.join(&external_lan).await?;

    let observed_peer =
        connect_and_observe_peer(&private_host, &external_host, external_addr.addr()).await?;

    assert_eq!(observed_peer.ip(), IpAddr::V4(router_external_addr.addr()));

    let inbound_result = external_host
        .assert_can_reach(&private_host, private_addr.addr())
        .await;
    assert!(inbound_result.is_err());

    Ok(())
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
