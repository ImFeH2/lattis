#![cfg(target_os = "linux")]

use anyhow::Result;
use netlab::{Host, Lan};

#[tokio::test]
async fn lan_connects_multiple_host_addresses() -> Result<()> {
    let lan = Lan::new("10.20.0.0/24".parse()?).await?;
    let host1 = Host::new().await?;
    let host2 = Host::new().await?;
    let host3 = Host::new().await?;

    let (_iface1, _host1_addr) = lan.attach(&host1).await?;
    let (_iface2, host2_addr) = lan.attach(&host2).await?;
    let (_iface3, _host3_addr) = lan.attach(&host3).await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;
    host3.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}
