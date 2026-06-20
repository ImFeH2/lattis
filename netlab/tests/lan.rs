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

    let host1_addr = host1.join(&lan).await?;
    let host2_addr = host2.join(&lan).await?;
    let host3_addr = host3.join(&lan).await?;

    assert_eq!(host1_addr, "10.21.0.1/29".parse()?);
    assert_eq!(host2_addr, "10.21.0.2/29".parse()?);
    assert_eq!(host3_addr, "10.21.0.3/29".parse()?);

    Ok(())
}

#[tokio::test]
async fn lan_returns_existing_host_address_when_joined_twice() -> Result<()> {
    let net = Net::new();
    let lan = net.lan("10.23.0.0/30".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;

    let host1_addr = host1.join(&lan).await?;
    let host1_addr_again = host1.join(&lan).await?;
    let host2_addr = host2.join(&lan).await?;

    assert_eq!(host1_addr, "10.23.0.1/30".parse()?);
    assert_eq!(host1_addr_again, host1_addr);
    assert_eq!(host2_addr, "10.23.0.2/30".parse()?);

    Ok(())
}

#[tokio::test]
async fn lan_connects_multiple_host_addresses() -> Result<()> {
    let net = Net::new();
    let lan = net.lan("10.20.0.0/24".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;
    let host3 = net.host().await?;

    host1.join(&lan).await?;
    let host2_addr = host2.join(&lan).await?;
    host3.join(&lan).await?;

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

    host1.join(&lan).await?;
    host2.join(&lan).await?;

    let err = host3.join(&lan).await.unwrap_err();
    assert!(err.to_string().contains("lan address pool is exhausted"));

    Ok(())
}
