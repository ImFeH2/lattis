#![cfg(target_os = "linux")]

use anyhow::Result;
use netlab::{Host, Lan};

#[tokio::test]
async fn host_connects_to_peer_addresses() -> Result<()> {
    let lan = Lan::new("10.10.0.0/24".parse()?).await?;
    let host1 = Host::new().await?;
    let host2 = Host::new().await?;

    let (_iface1, _host1_addr) = lan.attach(&host1).await?;
    let (_iface2, host2_addr) = lan.attach(&host2).await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}

#[tokio::test]
async fn connected_interface_can_be_modified_while_up() -> Result<()> {
    let lan = Lan::new("10.11.0.0/24".parse()?).await?;
    let host1 = Host::new().await?;
    let host2 = Host::new().await?;

    let (mut iface1, _host1_addr) = lan.attach(&host1).await?;
    let (_iface2, host2_addr) = lan.attach(&host2).await?;

    iface1.rename("uplink0").await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}
