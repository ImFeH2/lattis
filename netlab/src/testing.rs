use std::net::SocketAddr;

use anyhow::{Result, ensure};
use tokio::{
    net::UdpSocket,
    sync::oneshot,
    time::{Duration, timeout},
};

const UDP_ECHO_TIMEOUT: Duration = Duration::from_secs(1);

pub async fn run_udp_echo_server(
    bind_addr: SocketAddr,
    expected_peer: SocketAddr,
    ready: oneshot::Sender<()>,
) -> Result<()> {
    let socket = UdpSocket::bind(bind_addr).await?;
    let _ = ready.send(());

    let mut buf = [0; 64];
    let (n, peer) = timeout(UDP_ECHO_TIMEOUT, socket.recv_from(&mut buf)).await??;

    ensure!(
        peer == expected_peer,
        "expected UDP peer {}, got {}",
        expected_peer,
        peer
    );
    ensure!(
        &buf[..n] == b"ping",
        "expected UDP payload ping, got {}",
        String::from_utf8_lossy(&buf[..n])
    );

    socket.send_to(b"pong", peer).await?;

    Ok(())
}

pub async fn run_udp_echo_client(bind_addr: SocketAddr, server_addr: SocketAddr) -> Result<()> {
    let socket = UdpSocket::bind(bind_addr).await?;

    socket.send_to(b"ping", server_addr).await?;

    let mut buf = [0; 64];
    let (n, peer) = timeout(UDP_ECHO_TIMEOUT, socket.recv_from(&mut buf)).await??;

    ensure!(
        peer == server_addr,
        "expected UDP peer {}, got {}",
        server_addr,
        peer
    );
    ensure!(
        &buf[..n] == b"pong",
        "expected UDP payload pong, got {}",
        String::from_utf8_lossy(&buf[..n])
    );

    Ok(())
}
