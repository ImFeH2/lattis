#![cfg(target_os = "linux")]

use anyhow::Result;
use netlab::{
    Host, Lan,
    testing::{run_udp_echo_client, run_udp_echo_server},
};
use std::net::SocketAddr;
use tokio::sync::oneshot;

#[tokio::test]
async fn host_connects_to_peer_addresses() -> Result<()> {
    let lan = Lan::new("10.10.0.0/24".parse()?).await?;
    let host1 = Host::new().await?;
    let host2 = Host::new().await?;

    let (_iface1, host1_addr) = lan.attach(&host1).await?;
    let (_iface2, host2_addr) = lan.attach(&host2).await?;
    let host1_socket = SocketAddr::new(host1_addr.addr().into(), 8000);
    let host2_socket = SocketAddr::new(host2_addr.addr().into(), 9000);

    let (server_ready_tx, server_ready_rx) = oneshot::channel();
    let server =
        host2.spawn(move || run_udp_echo_server(host2_socket, host1_socket, server_ready_tx))?;

    server_ready_rx.await?;

    let client = host1.spawn(move || run_udp_echo_client(host1_socket, host2_socket))?;

    tokio::try_join!(server, client)?;

    Ok(())
}

#[tokio::test]
async fn connected_interface_can_be_modified_while_up() -> Result<()> {
    let lan = Lan::new("10.11.0.0/24".parse()?).await?;
    let host1 = Host::new().await?;
    let host2 = Host::new().await?;

    let (mut iface1, host1_addr) = lan.attach(&host1).await?;
    let (_iface2, host2_addr) = lan.attach(&host2).await?;

    iface1.rename("uplink0").await?;

    let host1_socket = SocketAddr::new(host1_addr.addr().into(), 8000);
    let host2_socket = SocketAddr::new(host2_addr.addr().into(), 9000);

    let (server_ready_tx, server_ready_rx) = oneshot::channel();
    let server =
        host2.spawn(move || run_udp_echo_server(host2_socket, host1_socket, server_ready_tx))?;

    server_ready_rx.await?;

    let client = host1.spawn(move || run_udp_echo_client(host1_socket, host2_socket))?;

    tokio::try_join!(server, client)?;

    Ok(())
}
