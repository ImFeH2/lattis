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

    router.serve(&lan1).await?;
    router.serve(&lan2).await?;

    lan1.attach(&host1).await?;
    let host2_addr = lan2.attach(&host2).await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;

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

    lan1.attach(&host1).await?;
    let host2_addr = lan2.attach(&host2).await?;

    router.serve(&lan1).await?;
    router.serve(&lan2).await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}
