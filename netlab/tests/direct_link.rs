#![cfg(target_os = "linux")]

use anyhow::Result;
use netlab::{
    DirectLink, Host,
    testing::{run_udp_echo_client, run_udp_echo_server},
};
use std::net::SocketAddr;
use tokio::sync::oneshot;

#[tokio::test]
async fn direct_link_connects_host_addresses() -> Result<()> {
    let host1 = Host::new("host1").await?;
    let host2 = Host::new("host2").await?;

    let host1_ip = "10.10.0.1";
    let host2_ip = "10.10.0.2";

    let (iface1, iface2) = DirectLink::connect(&host1, &host2).await?;

    iface1
        .configure()
        .add_address("10.10.0.1/24".parse()?)
        .up()
        .apply()
        .await?;

    iface2
        .configure()
        .add_address("10.10.0.2/24".parse()?)
        .up()
        .apply()
        .await?;

    let host1_socket: SocketAddr = format!("{host1_ip}:8000").parse()?;
    let host2_socket: SocketAddr = format!("{host2_ip}:9000").parse()?;

    let (server_ready_tx, server_ready_rx) = oneshot::channel();
    let server =
        host2.spawn(move || run_udp_echo_server(host2_socket, host1_socket, server_ready_tx))?;

    server_ready_rx.await?;

    let client = host1.spawn(move || run_udp_echo_client(host1_socket, host2_socket))?;

    tokio::try_join!(server, client)?;

    Ok(())
}
