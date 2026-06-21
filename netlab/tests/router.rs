#![cfg(target_os = "linux")]

use anyhow::Result;
use netlab::Net;

#[tokio::test]
async fn router_forwards_between_served_lans() -> Result<()> {
    let net = Net::new();
    let router = net.router().await?;
    let lan1 = net.lan("10.30.1.0/24".parse()?).await?;
    let lan2 = net.lan("10.30.2.0/24".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;

    lan1.set_gateway(&router).await?;
    lan2.set_gateway(&router).await?;

    host1.join(&lan1).await?;
    let host2_addr = host2.join(&lan2).await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}

#[tokio::test]
async fn converge_routes_between_lans_through_multiple_routers() -> Result<()> {
    let net = Net::new();
    let router1 = net.router().await?;
    let router2 = net.router().await?;
    let router3 = net.router().await?;
    let lan1 = net.lan("10.42.1.0/24".parse()?).await?;
    let lan2 = net.lan("10.42.2.0/24".parse()?).await?;
    let transit1 = net.lan("10.42.255.0/24".parse()?).await?;
    let transit2 = net.lan("10.42.254.0/24".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;

    lan1.set_gateway(&router1).await?;
    lan2.set_gateway(&router3).await?;
    router1.attach(&transit1).await?;
    router2.attach(&transit1).await?;
    router2.attach(&transit2).await?;
    router3.attach(&transit2).await?;

    host1.join(&lan1).await?;
    let host2_addr = host2.join(&lan2).await?;

    assert!(
        host1
            .assert_can_reach(&host2, host2_addr.addr())
            .await
            .is_err()
    );

    net.converge().await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}

#[tokio::test]
async fn converge_routes_across_ring_topologies() -> Result<()> {
    let net = Net::new();
    let router1 = net.router().await?;
    let router2 = net.router().await?;
    let router3 = net.router().await?;
    let router4 = net.router().await?;
    let lan1 = net.lan("10.44.1.0/24".parse()?).await?;
    let lan2 = net.lan("10.44.2.0/24".parse()?).await?;
    let lan3 = net.lan("10.44.3.0/24".parse()?).await?;
    let transit12 = net.lan("10.44.250.0/24".parse()?).await?;
    let transit23 = net.lan("10.44.251.0/24".parse()?).await?;
    let transit34 = net.lan("10.44.252.0/24".parse()?).await?;
    let transit41 = net.lan("10.44.253.0/24".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;
    let host3 = net.host().await?;

    lan1.set_gateway(&router1).await?;
    lan2.set_gateway(&router2).await?;
    lan3.set_gateway(&router3).await?;
    router1.attach(&transit12).await?;
    router2.attach(&transit12).await?;
    router2.attach(&transit23).await?;
    router3.attach(&transit23).await?;
    router3.attach(&transit34).await?;
    router4.attach(&transit34).await?;
    router4.attach(&transit41).await?;
    router1.attach(&transit41).await?;

    let host1_addr = host1.join(&lan1).await?;
    let host2_addr = host2.join(&lan2).await?;
    let host3_addr = host3.join(&lan3).await?;

    net.converge().await?;
    net.converge().await?;

    host1.assert_can_reach(&host3, host3_addr.addr()).await?;
    host3.assert_can_reach(&host1, host1_addr.addr()).await?;
    host2.assert_can_reach(&host3, host3_addr.addr()).await?;
    host3.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}

#[tokio::test]
async fn converge_keeps_disconnected_components_unreachable() -> Result<()> {
    let net = Net::new();
    let router1 = net.router().await?;
    let router2 = net.router().await?;
    let lan1 = net.lan("10.45.1.0/24".parse()?).await?;
    let lan2 = net.lan("10.45.2.0/24".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;

    lan1.set_gateway(&router1).await?;
    lan2.set_gateway(&router2).await?;
    host1.join(&lan1).await?;
    let host2_addr = host2.join(&lan2).await?;

    net.converge().await?;

    assert!(
        host1
            .assert_can_reach(&host2, host2_addr.addr())
            .await
            .is_err()
    );

    Ok(())
}

#[tokio::test]
async fn converge_does_not_route_external_traffic_into_nat_lans() -> Result<()> {
    let net = Net::new();
    let nat_router = net.router().await?;
    let external_router = net.router().await?;
    let private_lan = net.lan("10.43.1.0/24".parse()?).await?;
    let external_lan = net.lan("10.43.2.0/24".parse()?).await?;
    let transit = net.lan("10.43.255.0/24".parse()?).await?;
    let private_host = net.host().await?;
    let external_host = net.host().await?;

    external_lan.set_gateway(&external_router).await?;
    private_lan.set_gateway(&nat_router).await?;
    nat_router.attach(&transit).await?;
    external_router.attach(&transit).await?;

    let private_addr = private_host.join(&private_lan).await?;
    let external_addr = external_host.join(&external_lan).await?;

    net.converge().await?;
    external_host
        .assert_can_reach(&private_host, private_addr.addr())
        .await?;

    nat_router.enable_masquerade(&private_lan).await?;
    net.converge().await?;

    private_host
        .assert_can_reach(&external_host, external_addr.addr())
        .await?;
    assert!(
        external_host
            .assert_can_reach(&private_host, private_addr.addr())
            .await
            .is_err()
    );

    Ok(())
}

#[tokio::test]
async fn converge_allows_local_nat_lans_without_using_them_as_transit() -> Result<()> {
    let net = Net::new();
    let nat_router = net.router().await?;
    let external_router = net.router().await?;
    let internal_router = net.router().await?;
    let local_lan = net.lan("10.46.1.0/24".parse()?).await?;
    let external_lan = net.lan("10.46.2.0/24".parse()?).await?;
    let internal_lan = net.lan("10.46.3.0/24".parse()?).await?;
    let normal_transit = net.lan("10.46.252.0/24".parse()?).await?;
    let nat_transit = net.lan("10.46.253.0/24".parse()?).await?;
    let local_host = net.host().await?;
    let external_host = net.host().await?;
    let internal_host = net.host().await?;

    local_lan.set_gateway(&nat_router).await?;
    external_lan.set_gateway(&external_router).await?;
    internal_lan.set_gateway(&internal_router).await?;
    nat_router.attach(&normal_transit).await?;
    external_router.attach(&normal_transit).await?;
    nat_router.enable_masquerade(&nat_transit).await?;
    internal_router.attach(&nat_transit).await?;

    local_host.join(&local_lan).await?;
    external_host.join(&external_lan).await?;
    let internal_addr = internal_host.join(&internal_lan).await?;

    net.converge().await?;

    local_host
        .assert_can_reach(&internal_host, internal_addr.addr())
        .await?;
    assert!(
        external_host
            .assert_can_reach(&internal_host, internal_addr.addr())
            .await
            .is_err()
    );

    Ok(())
}

#[tokio::test]
async fn router_returns_existing_address_when_attached_twice() -> Result<()> {
    let net = Net::new();
    let router = net.router().await?;
    let lan = net.lan("10.32.0.0/30".parse()?).await?;
    let host = net.host().await?;

    let router_addr = router.attach(&lan).await?;
    let router_addr_again = router.attach(&lan).await?;
    let host_addr = host.join(&lan).await?;

    assert_eq!(router_addr, "10.32.0.1/30".parse()?);
    assert_eq!(router_addr_again, router_addr);
    assert_eq!(host_addr, "10.32.0.2/30".parse()?);

    Ok(())
}

#[tokio::test]
async fn router_gateway_setup_is_idempotent() -> Result<()> {
    let net = Net::new();
    let router = net.router().await?;
    let lan = net.lan("10.33.0.0/30".parse()?).await?;
    let host = net.host().await?;

    lan.set_gateway(&router).await?;
    lan.set_gateway(&router).await?;
    let host_addr = host.join(&lan).await?;

    assert_eq!(host_addr, "10.33.0.2/30".parse()?);

    Ok(())
}

#[tokio::test]
async fn router_updates_existing_hosts_when_serving_lans() -> Result<()> {
    let net = Net::new();
    let router = net.router().await?;
    let lan1 = net.lan("10.31.1.0/24".parse()?).await?;
    let lan2 = net.lan("10.31.2.0/24".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;

    host1.join(&lan1).await?;
    let host2_addr = host2.join(&lan2).await?;

    lan1.set_gateway(&router).await?;
    lan2.set_gateway(&router).await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}
