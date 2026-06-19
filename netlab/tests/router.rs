#![cfg(target_os = "linux")]

use std::net::SocketAddr;

use anyhow::Result;
use netlab::{
    Host, Lan, Router,
    testing::{run_udp_echo_client, run_udp_echo_server},
};
use tokio::sync::oneshot;

#[tokio::test]
async fn router_forwards_between_served_lans() -> Result<()> {
    let router = Router::new().await?;
    let lan1 = Lan::new("10.30.1.0/24".parse()?).await?;
    let lan2 = Lan::new("10.30.2.0/24".parse()?).await?;
    let host1 = Host::new().await?;
    let host2 = Host::new().await?;

    router.serve(&lan1).await?;
    router.serve(&lan2).await?;

    let (_iface1, host1_addr) = lan1.attach(&host1).await?;
    let (_iface2, host2_addr) = lan2.attach(&host2).await?;

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
