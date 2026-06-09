use anyhow::{Result, ensure};
use ipnet::IpNet;
use lattis_core::{Device, Peer, PrivateKey, PublicKey};
use net_topo::{
    DirectLink, Host,
    testing::{run_udp_echo_client, run_udp_echo_server},
};
use rand_core::OsRng;
use std::net::SocketAddr;
use tokio::{
    net::UdpSocket,
    sync::oneshot,
    time::{Duration, timeout},
};

const HOST1_IP: &str = "10.10.0.1";
const HOST2_IP: &str = "10.10.0.2";
const DEVICE1_VIRTUAL_IP: &str = "100.100.100.1";
const DEVICE2_VIRTUAL_IP: &str = "100.100.100.2";
const UNALLOWED_VIRTUAL_IP: &str = "100.100.100.3";
const DEVICE1_LISTEN_PORT: u16 = 8000;
const DEVICE2_LISTEN_PORT: u16 = 9000;
const DEVICE1_MSG_PORT: u16 = 8111;
const DEVICE2_MSG_PORT: u16 = 9111;
const WRONG_MSG_PORT: u16 = 9112;
const PREFIX_LEN: u8 = 24;
const WIREGUARD_HANDSHAKE_INIT: u32 = 1;
const WIREGUARD_HANDSHAKE_INIT_SIZE: usize = 148;
const TEST_TIMEOUT: Duration = Duration::from_secs(1);

#[tokio::test]
async fn connects_virtual_addresses() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, device2_public_key) = key_pair();

    let (_device1, _device2) = start_devices(
        &host1,
        &host2,
        device1_private_key,
        device2_public_key,
        device2_private_key,
        device1_public_key,
    )
    .await?;

    assert_udp_echo_succeeds(
        &host1,
        &host2,
        socket_addr(DEVICE1_VIRTUAL_IP, DEVICE1_MSG_PORT)?,
        socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?,
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn sends_wireguard_handshake_on_underlay() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, _) = key_pair();
    let (_, device2_public_key) = key_pair();

    let _device1 = start_device(
        &host1,
        DEVICE1_LISTEN_PORT,
        device1_private_key,
        DEVICE1_VIRTUAL_IP,
        device2_public_key,
        DEVICE2_VIRTUAL_IP,
        HOST2_IP,
        DEVICE2_LISTEN_PORT,
    )
    .await?;

    let outer_endpoint = socket_addr(HOST2_IP, DEVICE2_LISTEN_PORT)?;
    let (ready_tx, ready_rx) = oneshot::channel();
    let capture = host2.spawn(move || capture_outer_udp_packet(outer_endpoint, ready_tx))?;
    ready_rx.await?;

    let client_addr = socket_addr(DEVICE1_VIRTUAL_IP, DEVICE1_MSG_PORT)?;
    let target_addr = socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?;
    let sender = host1.spawn(move || send_udp_datagram(client_addr, target_addr))?;

    sender.await?;
    let packet = capture.await?;

    ensure!(
        packet.len() == WIREGUARD_HANDSHAKE_INIT_SIZE,
        "expected WireGuard handshake packet size {}, got {}",
        WIREGUARD_HANDSHAKE_INIT_SIZE,
        packet.len()
    );

    let message_type = u32::from_le_bytes(packet[..4].try_into()?);
    ensure!(
        message_type == WIREGUARD_HANDSHAKE_INIT,
        "expected WireGuard handshake message type {}, got {}",
        WIREGUARD_HANDSHAKE_INIT,
        message_type
    );

    Ok(())
}

#[tokio::test]
async fn rejects_wrong_peer_key() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, _) = key_pair();
    let (_, wrong_device2_public_key) = key_pair();

    let (_device1, _device2) = start_devices(
        &host1,
        &host2,
        device1_private_key,
        wrong_device2_public_key,
        device2_private_key,
        device1_public_key,
    )
    .await?;

    assert_udp_echo_fails(
        &host1,
        &host2,
        socket_addr(DEVICE1_VIRTUAL_IP, DEVICE1_MSG_PORT)?,
        socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?,
        socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?,
        "UDP echo unexpectedly succeeded with a wrong WireGuard peer key",
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn drops_unallowed_virtual_address() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, device2_public_key) = key_pair();

    let (_device1, _device2) = start_devices(
        &host1,
        &host2,
        device1_private_key,
        device2_public_key,
        device2_private_key,
        device1_public_key,
    )
    .await?;

    assert_udp_echo_fails(
        &host1,
        &host2,
        socket_addr(DEVICE1_VIRTUAL_IP, DEVICE1_MSG_PORT)?,
        socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?,
        socket_addr(UNALLOWED_VIRTUAL_IP, DEVICE2_MSG_PORT)?,
        "UDP echo unexpectedly succeeded for an unallowed virtual address",
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn does_not_echo_to_wrong_udp_port() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, device2_public_key) = key_pair();

    let (_device1, _device2) = start_devices(
        &host1,
        &host2,
        device1_private_key,
        device2_public_key,
        device2_private_key,
        device1_public_key,
    )
    .await?;

    assert_udp_echo_fails(
        &host1,
        &host2,
        socket_addr(DEVICE1_VIRTUAL_IP, DEVICE1_MSG_PORT)?,
        socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?,
        socket_addr(DEVICE2_VIRTUAL_IP, WRONG_MSG_PORT)?,
        "UDP echo unexpectedly succeeded on the wrong UDP port",
    )
    .await?;

    Ok(())
}

async fn connected_hosts() -> Result<(Host, Host)> {
    let host1 = Host::new("host1").await?;
    let host2 = Host::new("host2").await?;

    let (iface1, iface2) = DirectLink::connect(&host1, &host2).await?;

    iface1
        .configure()
        .add_address(HOST1_IP.parse()?, 24)
        .up()
        .apply()
        .await?;

    iface2
        .configure()
        .add_address(HOST2_IP.parse()?, 24)
        .up()
        .apply()
        .await?;

    Ok((host1, host2))
}

async fn start_devices(
    host1: &Host,
    host2: &Host,
    device1_private_key: PrivateKey,
    device1_peer_public_key: PublicKey,
    device2_private_key: PrivateKey,
    device2_peer_public_key: PublicKey,
) -> Result<(Device, Device)> {
    let device1 = start_device(
        host1,
        DEVICE1_LISTEN_PORT,
        device1_private_key,
        DEVICE1_VIRTUAL_IP,
        device1_peer_public_key,
        DEVICE2_VIRTUAL_IP,
        HOST2_IP,
        DEVICE2_LISTEN_PORT,
    )
    .await?;

    let device2 = start_device(
        host2,
        DEVICE2_LISTEN_PORT,
        device2_private_key,
        DEVICE2_VIRTUAL_IP,
        device2_peer_public_key,
        DEVICE1_VIRTUAL_IP,
        HOST1_IP,
        DEVICE1_LISTEN_PORT,
    )
    .await?;

    Ok((device1, device2))
}

async fn start_device(
    host: &Host,
    listen_port: u16,
    private_key: PrivateKey,
    virtual_ip: &'static str,
    peer_public_key: PublicKey,
    peer_virtual_ip: &'static str,
    peer_host_ip: &'static str,
    peer_listen_port: u16,
) -> Result<Device> {
    let peer_endpoint = socket_addr(peer_host_ip, peer_listen_port)?;

    host.run(move || async move {
        let peer = Peer::new(
            peer_public_key,
            vec![IpNet::new(peer_virtual_ip.parse()?, 32)?],
            peer_endpoint,
        );
        let virtual_address = IpNet::new(virtual_ip.parse()?, PREFIX_LEN)?;

        Device::builder()
            .listen_port(listen_port)
            .private_key(private_key)
            .add_virtual_address(virtual_address)
            .add_peer(peer)
            .build()
            .await
    })
    .await
}

async fn assert_udp_echo_succeeds(
    host1: &Host,
    host2: &Host,
    client_addr: SocketAddr,
    server_addr: SocketAddr,
) -> Result<()> {
    let (server_ready_tx, server_ready_rx) = oneshot::channel();
    let server =
        host2.spawn(move || run_udp_echo_server(server_addr, client_addr, server_ready_tx))?;

    server_ready_rx.await?;

    let client = host1.spawn(move || run_udp_echo_client(client_addr, server_addr))?;

    tokio::try_join!(server, client)?;

    Ok(())
}

async fn assert_udp_echo_fails(
    host1: &Host,
    host2: &Host,
    client_addr: SocketAddr,
    server_addr: SocketAddr,
    client_target_addr: SocketAddr,
    message: &str,
) -> Result<()> {
    let (server_ready_tx, server_ready_rx) = oneshot::channel();
    let server =
        host2.spawn(move || run_udp_echo_server(server_addr, client_addr, server_ready_tx))?;

    server_ready_rx.await?;

    let client = host1.spawn(move || run_udp_echo_client(client_addr, client_target_addr))?;
    let (server_result, client_result) = tokio::join!(server, client);

    ensure!(
        server_result.is_err(),
        "{}: server unexpectedly received the request",
        message
    );
    ensure!(
        client_result.is_err(),
        "{}: client unexpectedly received the response",
        message
    );

    Ok(())
}

async fn capture_outer_udp_packet(
    bind_addr: SocketAddr,
    ready: oneshot::Sender<()>,
) -> Result<Vec<u8>> {
    let socket = UdpSocket::bind(bind_addr).await?;
    let _ = ready.send(());

    let mut buf = [0u8; 2048];
    let (len, _) = timeout(TEST_TIMEOUT, socket.recv_from(&mut buf)).await??;

    Ok(buf[..len].to_vec())
}

async fn send_udp_datagram(bind_addr: SocketAddr, target_addr: SocketAddr) -> Result<()> {
    let socket = UdpSocket::bind(bind_addr).await?;
    socket.send_to(b"ping", target_addr).await?;
    Ok(())
}

fn key_pair() -> (PrivateKey, PublicKey) {
    let private_key = PrivateKey::random_from_rng(OsRng);
    let public_key = PublicKey::from(&private_key);
    (private_key, public_key)
}

fn socket_addr(ip: &str, port: u16) -> Result<SocketAddr> {
    Ok(format!("{}:{}", ip, port).parse()?)
}
