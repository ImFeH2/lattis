use anyhow::Result;

#[cfg(not(target_os = "linux"))]
use anyhow::bail;

#[cfg(target_os = "linux")]
use rtnetlink::RouteMessageBuilder;

#[cfg(target_os = "linux")]
use crate::model::{LATTIS_NETWORK_PREFIX, LATTIS_NETWORK_PREFIX_LEN};

pub(super) struct RouteGuard {
    #[cfg(target_os = "linux")]
    interface_index: u32,
}

#[cfg(target_os = "linux")]
pub(super) async fn add_lattis_network_route(device: &tun_rs::AsyncDevice) -> Result<RouteGuard> {
    let interface_index = device.if_index()?;
    add_route(interface_index).await?;

    Ok(RouteGuard { interface_index })
}

#[cfg(not(target_os = "linux"))]
pub(super) async fn add_lattis_network_route(_device: &tun_rs::AsyncDevice) -> Result<RouteGuard> {
    bail!("Lattis network route configuration is not supported on this platform yet")
}

#[cfg(target_os = "linux")]
impl Drop for RouteGuard {
    fn drop(&mut self) {
        let interface_index = self.interface_index;

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(error) = delete_route(interface_index).await {
                        eprintln!("Failed to remove Lattis network route: {error}");
                    }
                });
            }
            Err(error) => {
                eprintln!("Failed to remove Lattis network route: {error}");
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Drop for RouteGuard {
    fn drop(&mut self) {}
}

#[cfg(target_os = "linux")]
async fn add_route(interface_index: u32) -> Result<()> {
    let (connection, handle, _) = rtnetlink::new_connection()?;
    tokio::spawn(connection);

    let route = RouteMessageBuilder::<std::net::Ipv4Addr>::new()
        .destination_prefix(LATTIS_NETWORK_PREFIX, LATTIS_NETWORK_PREFIX_LEN)
        .output_interface(interface_index)
        .build();

    handle.route().add(route).execute().await?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn delete_route(interface_index: u32) -> Result<()> {
    let (connection, handle, _) = rtnetlink::new_connection()?;
    tokio::spawn(connection);

    let route = RouteMessageBuilder::<std::net::Ipv4Addr>::new()
        .destination_prefix(LATTIS_NETWORK_PREFIX, LATTIS_NETWORK_PREFIX_LEN)
        .output_interface(interface_index)
        .build();

    handle.route().del(route).execute().await?;
    Ok(())
}
