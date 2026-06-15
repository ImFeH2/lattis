use anyhow::{Result, bail};
use ipnet::IpNet;

#[cfg(any(
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos")),
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
))]
pub fn open_tun_device(name: &str, addresses: Vec<IpNet>) -> Result<tun_rs::AsyncDevice> {
    if addresses.is_empty() {
        bail!("At least one TUN address must be configured");
    }

    let device = tun_rs::DeviceBuilder::new().name(name).build_async()?;

    for address in addresses {
        match address {
            IpNet::V4(address) => device.add_address_v4(address.addr(), address.prefix_len())?,
            IpNet::V6(address) => device.add_address_v6(address.addr(), address.prefix_len())?,
        };
    }

    Ok(device)
}

#[cfg(not(any(
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos")),
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
)))]
pub fn open_tun_device(_name: &str, _addresses: Vec<IpNet>) -> Result<tun_rs::AsyncDevice> {
    bail!("Creating a TUN device by name is not supported on this platform");
}
