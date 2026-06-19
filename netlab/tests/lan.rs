#![cfg(target_os = "linux")]

use anyhow::Result;
use netlab::{
    Host, Lan,
    testing::{run_udp_echo_client, run_udp_echo_server},
};
use std::net::SocketAddr;
use tokio::sync::oneshot;

#[tokio::test]
async fn lan_connects_multiple_host_addresses() -> Result<()> {
    let lan = Lan::new("10.20.0.0/24".parse()?).await?;
    let host1 = Host::new().await?;
    let host2 = Host::new().await?;
    let host3 = Host::new().await?;

    let (_iface1, host1_addr) = lan.attach(&host1).await?;
    let (_iface2, host2_addr) = lan.attach(&host2).await?;
    let (_iface3, host3_addr) = lan.attach(&host3).await?;

    run_echo_pair(
        &host1,
        SocketAddr::new(host1_addr.addr().into(), 8000),
        &host2,
        SocketAddr::new(host2_addr.addr().into(), 9000),
    )
    .await?;
    run_echo_pair(
        &host3,
        SocketAddr::new(host3_addr.addr().into(), 8001),
        &host2,
        SocketAddr::new(host2_addr.addr().into(), 9001),
    )
    .await?;

    Ok(())
}

async fn run_echo_pair(
    client_host: &Host,
    client_addr: SocketAddr,
    server_host: &Host,
    server_addr: SocketAddr,
) -> Result<()> {
    let (server_ready_tx, server_ready_rx) = oneshot::channel();
    let server = server_host
        .spawn(move || run_udp_echo_server(server_addr, client_addr, server_ready_tx))?;

    server_ready_rx.await?;

    let client = client_host.spawn(move || run_udp_echo_client(client_addr, server_addr))?;

    tokio::try_join!(server, client)?;

    Ok(())
}
