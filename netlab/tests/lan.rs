#![cfg(target_os = "linux")]

use anyhow::Result;
use netlab::Net;

#[tokio::test]
async fn lan_allocates_host_addresses_in_order() -> Result<()> {
    let net = Net::new();
    let lan = net.lan("10.21.0.0/29".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;
    let host3 = net.host().await?;

    let host1_addr = lan.attach(&host1).await?;
    let host2_addr = lan.attach(&host2).await?;
    let host3_addr = lan.attach(&host3).await?;

    assert_eq!(host1_addr, "10.21.0.1/29".parse()?);
    assert_eq!(host2_addr, "10.21.0.2/29".parse()?);
    assert_eq!(host3_addr, "10.21.0.3/29".parse()?);

    Ok(())
}

#[tokio::test]
async fn lan_connects_multiple_host_addresses() -> Result<()> {
    let net = Net::new();
    let lan = net.lan("10.20.0.0/24".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;
    let host3 = net.host().await?;

    lan.attach(&host1).await?;
    let host2_addr = lan.attach(&host2).await?;
    lan.attach(&host3).await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;
    host3.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}

#[tokio::test]
async fn lan_reports_exhausted_address_pool() -> Result<()> {
    let net = Net::new();
    let lan = net.lan("10.22.0.0/30".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;
    let host3 = net.host().await?;

    lan.attach(&host1).await?;
    lan.attach(&host2).await?;

    let err = lan.attach(&host3).await.unwrap_err();
    assert!(err.to_string().contains("lan address pool is exhausted"));

    Ok(())
}
