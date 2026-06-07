use anyhow::Result;
use net_topo::{DirectLink, Host};
use std::net::SocketAddr;
use tokio::{
    net::UdpSocket,
    sync::oneshot,
    time::{Duration, timeout},
};

async fn run_udp_echo_server(
    bind_addr: SocketAddr,
    expected_peer: SocketAddr,
    ready: oneshot::Sender<()>,
) -> Result<()> {
    let socket = UdpSocket::bind(bind_addr).await?;
    let _ = ready.send(());

    let mut buf = [0; 64];
    let (n, peer) = timeout(Duration::from_secs(1), socket.recv_from(&mut buf)).await??;

    assert_eq!(peer, expected_peer);
    assert_eq!(&buf[..n], b"ping");

    socket.send_to(b"pong", peer).await?;

    Ok(())
}

async fn run_udp_echo_client(bind_addr: SocketAddr, server_addr: SocketAddr) -> Result<()> {
    let socket = UdpSocket::bind(bind_addr).await?;

    socket.send_to(b"ping", server_addr).await?;

    let mut buf = [0; 64];
    let (n, peer) = timeout(Duration::from_secs(1), socket.recv_from(&mut buf)).await??;

    assert_eq!(peer, server_addr);
    assert_eq!(&buf[..n], b"pong");

    Ok(())
}

#[cfg(target_os = "linux")]
#[ignore = "requires Linux network namespaces"]
#[tokio::test]
async fn two_device_direct_link() -> Result<()> {
    let host1 = Host::new("host1").await?;
    let host2 = Host::new("host2").await?;

    let host1_ip = "10.10.0.1";
    let host2_ip = "10.10.0.2";

    let (iface1, iface2) = DirectLink::connect(&host1, &host2).await?;

    iface1
        .configure()
        .add_address(host1_ip.parse()?, 24)
        .up()
        .apply()
        .await?;

    iface2
        .configure()
        .add_address(host2_ip.parse()?, 24)
        .up()
        .apply()
        .await?;

    host1
        .run_blocking(|| {
            lattis_core::print_info()?;
            Ok(())
        })
        .await?;

    host2
        .run_blocking(|| {
            lattis_core::print_info()?;
            Ok(())
        })
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
