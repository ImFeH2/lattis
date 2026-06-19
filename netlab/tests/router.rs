#![cfg(target_os = "linux")]

use anyhow::Result;
use netlab::{Host, Lan, Router};

#[tokio::test]
async fn router_forwards_between_served_lans() -> Result<()> {
    let router = Router::new().await?;
    let lan1 = Lan::new("10.30.1.0/24".parse()?).await?;
    let lan2 = Lan::new("10.30.2.0/24".parse()?).await?;
    let host1 = Host::new().await?;
    let host2 = Host::new().await?;

    router.serve(&lan1).await?;
    router.serve(&lan2).await?;

    lan1.attach(&host1).await?;
    let host2_addr = lan2.attach(&host2).await?;

    host1.assert_can_reach(&host2, host2_addr.addr()).await?;

    Ok(())
}
