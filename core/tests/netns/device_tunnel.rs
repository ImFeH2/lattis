use anyhow::{Result, ensure};
use ipnet::IpNet;
use lattis_core::{DEFAULT_DEVICE_LISTEN_PORT, Device, Peer, PrivateKey, PublicKey};
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
const WRONG_DEVICE_LISTEN_PORT: u16 = DEFAULT_DEVICE_LISTEN_PORT + 1;
const DEVICE1_MSG_PORT: u16 = 8111;
const DEVICE2_MSG_PORT: u16 = 9111;
const WRONG_MSG_PORT: u16 = 9112;
const PREFIX_LEN: u8 = 24;
const WIREGUARD_HANDSHAKE_INIT: u32 = 1;
const WIREGUARD_HANDSHAKE_INIT_SIZE: usize = 148;
const TEST_TIMEOUT: Duration = Duration::from_secs(3);
const HANDSHAKE_RETRY_TEST_TIMEOUT: Duration = Duration::from_secs(7);

#[tokio::test]
async fn connects_virtual_addresses() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, device2_public_key) = key_pair();

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    let device2 = host2
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device2_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;
    device2.add_peer(Peer::new(
        device1_public_key,
        vec![IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST1_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

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
async fn connects_after_adding_peers_at_runtime() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, device2_public_key) = key_pair();

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    let device2 = host2
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device2_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;
    device2.add_peer(Peer::new(
        device1_public_key,
        vec![IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST1_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

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
async fn connects_after_updating_peer_endpoint() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, device2_public_key) = key_pair();

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    let device2 = host2
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device2_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, WRONG_DEVICE_LISTEN_PORT)?,
    ))?;
    device2.add_peer(Peer::new(
        device1_public_key,
        vec![IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST1_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

    device1.update_peer_endpoint(
        &device2_public_key,
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    )?;

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
async fn learns_peer_endpoint_from_authenticated_packet() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, device2_public_key) = key_pair();

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    let device2 = host2
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .listen_port(WRONG_DEVICE_LISTEN_PORT)
                .private_key(device2_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;
    device2.add_peer(Peer::new(
        device1_public_key,
        vec![IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST1_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

    assert_udp_echo_succeeds(
        &host2,
        &host1,
        socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?,
        socket_addr(DEVICE1_VIRTUAL_IP, DEVICE1_MSG_PORT)?,
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn sends_wireguard_handshake_on_underlay() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, _) = key_pair();
    let (_, device2_public_key) = key_pair();

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

    let outer_endpoint = socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?;
    let (ready_tx, ready_rx) = oneshot::channel();
    let capture = host2.spawn(move || capture_outer_udp_packet(outer_endpoint, ready_tx))?;
    ready_rx.await?;

    let client_addr = socket_addr(DEVICE1_VIRTUAL_IP, DEVICE1_MSG_PORT)?;
    let target_addr = socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?;
    let sender = host1.spawn(move || send_udp_datagram(client_addr, target_addr))?;

    sender.await?;
    let packet = capture.await?;

    assert_wireguard_handshake_init(&packet)?;

    Ok(())
}

#[tokio::test]
async fn retries_wireguard_handshake_on_timer() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, _) = key_pair();
    let (_, device2_public_key) = key_pair();

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

    let outer_endpoint = socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?;
    let (ready_tx, ready_rx) = oneshot::channel();
    let capture = host2.spawn(move || capture_outer_udp_packets(outer_endpoint, 2, ready_tx))?;
    ready_rx.await?;

    let client_addr = socket_addr(DEVICE1_VIRTUAL_IP, DEVICE1_MSG_PORT)?;
    let target_addr = socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?;
    let sender = host1.spawn(move || send_udp_datagram(client_addr, target_addr))?;

    sender.await?;
    let packets = capture.await?;

    ensure!(
        packets.len() == 2,
        "expected 2 WireGuard handshake packets, got {}",
        packets.len()
    );
    assert_wireguard_handshake_init(&packets[0])?;
    assert_wireguard_handshake_init(&packets[1])?;

    Ok(())
}

fn assert_wireguard_handshake_init(packet: &[u8]) -> Result<()> {
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

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    let device2 = host2
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device2_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        wrong_device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;
    device2.add_peer(Peer::new(
        device1_public_key,
        vec![IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST1_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

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

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    let device2 = host2
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device2_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;
    device2.add_peer(Peer::new(
        device1_public_key,
        vec![IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST1_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

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
async fn drops_authenticated_packet_from_unallowed_source() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, device2_public_key) = key_pair();

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(UNALLOWED_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    let device2 = host2
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device2_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;
    device2.add_peer(Peer::new(
        device1_public_key,
        vec![IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST1_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

    assert_udp_echo_fails(
        &host1,
        &host2,
        socket_addr(UNALLOWED_VIRTUAL_IP, DEVICE1_MSG_PORT)?,
        socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?,
        socket_addr(DEVICE2_VIRTUAL_IP, DEVICE2_MSG_PORT)?,
        "UDP echo unexpectedly succeeded from an unallowed virtual source",
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn does_not_echo_to_wrong_udp_port() -> Result<()> {
    let (host1, host2) = connected_hosts().await?;
    let (device1_private_key, device1_public_key) = key_pair();
    let (device2_private_key, device2_public_key) = key_pair();

    let device1 = host1
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device1_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    let device2 = host2
        .run(move || async move {
            let virtual_address = IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, PREFIX_LEN)?;

            Device::builder()
                .private_key(device2_private_key)
                .add_virtual_address(virtual_address)
                .build()
                .await
        })
        .await?;

    device1.add_peer(Peer::new(
        device2_public_key,
        vec![IpNet::new(DEVICE2_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST2_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;
    device2.add_peer(Peer::new(
        device1_public_key,
        vec![IpNet::new(DEVICE1_VIRTUAL_IP.parse()?, 32)?],
        socket_addr(HOST1_IP, DEFAULT_DEVICE_LISTEN_PORT)?,
    ))?;

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

    timeout(TEST_TIMEOUT, async {
        tokio::try_join!(server, client)?;
        Ok::<(), anyhow::Error>(())
    })
    .await??;

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
    let (server_result, client_result) =
        timeout(TEST_TIMEOUT, async { tokio::join!(server, client) }).await?;

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

async fn capture_outer_udp_packets(
    bind_addr: SocketAddr,
    count: usize,
    ready: oneshot::Sender<()>,
) -> Result<Vec<Vec<u8>>> {
    let socket = UdpSocket::bind(bind_addr).await?;
    let _ = ready.send(());

    let mut packets = Vec::with_capacity(count);
    for _ in 0..count {
        let mut buf = [0u8; 2048];
        let (len, _) = timeout(HANDSHAKE_RETRY_TEST_TIMEOUT, socket.recv_from(&mut buf)).await??;
        packets.push(buf[..len].to_vec());
    }

    Ok(packets)
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
