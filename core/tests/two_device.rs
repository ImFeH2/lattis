use anyhow::Result;
use net_topo::{DirectLink, Host};

#[cfg(target_os = "linux")]
#[ignore = "requires Linux network namespaces"]
#[tokio::test]
async fn two_device_direct_link() -> Result<()> {
    let host1 = Host::new("host1").await?;
    let host2 = Host::new("host2").await?;

    let (iface1, iface2) = DirectLink::connect(&host1, &host2).await?;

    iface1
        .configure()
        .add_address("10.10.0.1".parse()?, 24)
        .up()
        .apply()
        .await?;

    iface2
        .configure()
        .add_address("10.10.0.2".parse()?, 24)
        .up()
        .apply()
        .await?;

    host1
        .run_blocking(|| {
            lattis_core::print_info()?;
            Ok(())
        })
        .await?;

    host2
        .run_blocking(|| {
            lattis_core::print_info()?;
            Ok(())
        })
        .await?;

    Ok(())
}
