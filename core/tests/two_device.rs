use anyhow::Result;
use ipnet::IpNet;
use lattis_core::{Device, Peer};
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

    println!(
        "Server {} received from {}: {}",
        bind_addr,
        peer,
        String::from_utf8_lossy(&buf[..n])
    );

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

    println!(
        "Client {} received from {}: {}",
        bind_addr,
        peer,
        String::from_utf8_lossy(&buf[..n])
    );

    assert_eq!(peer, server_addr);
    assert_eq!(&buf[..n], b"pong");

    Ok(())
}

#[cfg(target_os = "linux")]
#[ignore = "requires Linux network namespaces"]
#[tokio::test]
async fn two_host_direct_link() -> Result<()> {
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
        .spawn_blocking(|| {
            lattis_core::print_iface_info()?;
            Ok(())
        })?
        .await?;

    host2
        .spawn_blocking(|| {
            lattis_core::print_iface_info()?;
            Ok(())
        })?
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

#[cfg(target_os = "linux")]
#[ignore = "requires Linux network namespaces"]
#[tokio::test]
async fn two_device_direct_link() -> Result<()> {
    let host1 = Host::new("host1").await?;
    let host2 = Host::new("host2").await?;

    let host1_ip = "10.10.0.1";
    let device1_virtual_ip = "100.100.100.1";
    let device1_listen_port = 8000;
    let device1_msg_port = 8111;
    let host2_ip = "10.10.0.2";
    let device2_virtual_ip = "100.100.100.2";
    let device2_listen_port = 9000;
    let device2_msg_port = 9111;

    let prefix_len = 24;

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

    let _device1 = host1
        .run(move || async move {
            let peer = Peer {
                allowed_ips: vec![IpNet::new(device2_virtual_ip.parse()?, 32)?],
                endpoint: format!("{}:{}", host2_ip, device2_listen_port).parse()?,
            };
            let local_address = IpNet::new(device1_virtual_ip.parse()?, prefix_len)?;

            Device::builder()
                .listen_port(device1_listen_port)
                .add_local_address(local_address)
                .add_peer(peer)
                .build()
                .await
        })
        .await?;

    let _device2 = host2
        .run(move || async move {
            let peer = Peer {
                allowed_ips: vec![IpNet::new(device1_virtual_ip.parse()?, 32)?],
                endpoint: format!("{}:{}", host1_ip, device1_listen_port).parse()?,
            };
            let local_address = IpNet::new(device2_virtual_ip.parse()?, prefix_len)?;

            Device::builder()
                .listen_port(device2_listen_port)
                .add_local_address(local_address)
                .add_peer(peer)
                .build()
                .await
        })
        .await?;

    host1
        .spawn_blocking(|| {
            lattis_core::print_iface_info()?;
            Ok(())
        })?
        .await?;

    host2
        .spawn_blocking(|| {
            lattis_core::print_iface_info()?;
            Ok(())
        })?
        .await?;

    let device1_socket: SocketAddr =
        format!("{}:{}", device1_virtual_ip, device1_msg_port).parse()?;
    let device2_socket: SocketAddr =
        format!("{}:{}", device2_virtual_ip, device2_msg_port).parse()?;

    let (server_ready_tx, server_ready_rx) = oneshot::channel();
    let server = host2
        .spawn(move || run_udp_echo_server(device2_socket, device1_socket, server_ready_tx))?;

    server_ready_rx.await?;

    let client = host1.spawn(move || run_udp_echo_client(device1_socket, device2_socket))?;

    tokio::try_join!(server, client)?;

    Ok(())
}
