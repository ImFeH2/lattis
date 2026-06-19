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
    let lan = Lan::new("underlay").await?;
    let host1 = Host::new("host1").await?;
    let host2 = Host::new("host2").await?;
    let host3 = Host::new("host3").await?;

    let iface1 = lan.connect(&host1).await?;
    let iface2 = lan.connect(&host2).await?;
    let iface3 = lan.connect(&host3).await?;

    iface1.add_address("10.20.0.1/24".parse()?).await?;
    iface2.add_address("10.20.0.2/24".parse()?).await?;
    iface3.add_address("10.20.0.3/24".parse()?).await?;

    run_echo_pair(&host1, "10.20.0.1:8000", &host2, "10.20.0.2:9000").await?;
    run_echo_pair(&host3, "10.20.0.3:8001", &host2, "10.20.0.2:9001").await?;

    Ok(())
}

async fn run_echo_pair(
    client_host: &Host,
    client_addr: &str,
    server_host: &Host,
    server_addr: &str,
) -> Result<()> {
    let client_addr: SocketAddr = client_addr.parse()?;
    let server_addr: SocketAddr = server_addr.parse()?;
    let (server_ready_tx, server_ready_rx) = oneshot::channel();
    let server = server_host
        .spawn(move || run_udp_echo_server(server_addr, client_addr, server_ready_tx))?;

    server_ready_rx.await?;

    let client = client_host.spawn(move || run_udp_echo_client(client_addr, server_addr))?;

    tokio::try_join!(server, client)?;

    Ok(())
}
