#![cfg(target_os = "linux")]

use anyhow::Result;
use netlab::Net;

#[tokio::test]
async fn host_connects_to_peer_addresses() -> Result<()> {
    let net = Net::new();
    let lan = net.lan("10.10.0.0/24".parse()?).await?;
    let host1 = net.host().await?;
    let host2 = net.host().await?;

    lan.attach(&host1).await?;
    let host2_addr = lan.attach(&host2).await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}
