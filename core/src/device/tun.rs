use anyhow::{Result, bail};
use ipnet::IpNet;
use std::io;
#[cfg(unix)]
use std::os::fd::{IntoRawFd, OwnedFd};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TunConfig {
    pub name: String,
    pub addresses: Vec<IpNet>,
}

pub struct TunDevice {
    inner: tun_rs::AsyncDevice,
}

impl TunDevice {
    pub fn open(config: TunConfig) -> Result<Self> {
        open(config)
    }

    #[cfg(unix)]
    pub fn from_owned_fd(fd: OwnedFd) -> Result<Self> {
        let inner = unsafe { tun_rs::AsyncDevice::from_fd(fd.into_raw_fd())? };

        Ok(Self { inner })
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.recv(buf).await
    }

    pub async fn send(&self, packet: &[u8]) -> io::Result<usize> {
        self.inner.send(packet).await
    }
}

#[cfg(any(
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos")),
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
))]
fn open(config: TunConfig) -> Result<TunDevice> {
    if config.addresses.is_empty() {
        bail!("At least one TUN address must be configured");
    }

    let device = tun_rs::DeviceBuilder::new()
        .name(config.name)
        .build_async()?;

    for address in config.addresses {
        match address {
            IpNet::V4(address) => device.add_address_v4(address.addr(), address.prefix_len())?,
            IpNet::V6(address) => device.add_address_v6(address.addr(), address.prefix_len())?,
        };
    }

    Ok(TunDevice { inner: device })
}

#[cfg(not(any(
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos")),
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
)))]
fn open(_config: TunConfig) -> Result<TunDevice> {
    bail!("Creating a TUN device by name is not supported on this platform");
}
