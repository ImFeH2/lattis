use anyhow::Result;
use ipnet::IpNet;
use lattis_core::{Device, Peer, PrivateKey, PublicKey};
use net_topo::{
    DirectLink, Host,
    testing::{run_udp_echo_client, run_udp_echo_server},
};
use rand_core::OsRng;
use std::net::SocketAddr;
use tokio::sync::oneshot;

#[tokio::test]
async fn connects_virtual_addresses() -> Result<()> {
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

    let device1_private_key = PrivateKey::random_from_rng(OsRng);
    let device1_public_key = PublicKey::from(&device1_private_key);
    let device2_private_key = PrivateKey::random_from_rng(OsRng);
    let device2_public_key = PublicKey::from(&device2_private_key);

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
            let peer = Peer::new(
                device2_public_key,
                vec![IpNet::new(device2_virtual_ip.parse()?, 32)?],
                format!("{}:{}", host2_ip, device2_listen_port).parse()?,
            );
            let local_address = IpNet::new(device1_virtual_ip.parse()?, prefix_len)?;

            Device::builder()
                .listen_port(device1_listen_port)
                .private_key(device1_private_key)
                .add_local_address(local_address)
                .add_peer(peer)
                .build()
                .await
        })
        .await?;

    let _device2 = host2
        .run(move || async move {
            let peer = Peer::new(
                device1_public_key,
                vec![IpNet::new(device1_virtual_ip.parse()?, 32)?],
                format!("{}:{}", host1_ip, device1_listen_port).parse()?,
            );
            let local_address = IpNet::new(device2_virtual_ip.parse()?, prefix_len)?;

            Device::builder()
                .listen_port(device2_listen_port)
                .private_key(device2_private_key)
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
