pub mod network;

pub use network::{
    DEFAULT_DEVICE_LISTEN_PORT, Device, DeviceIdentity, PeerConfig, PrivateKey, PublicKey,
};

use anyhow::Result;
use if_addrs::get_if_addrs;

pub fn print_iface_info() -> Result<()> {
    let interfaces = get_if_addrs()?;

    for iface in interfaces {
        println!("Interface: {}", iface.name);

        match &iface.addr {
            if_addrs::IfAddr::V4(addr) => {
                println!("  IPv4: {}/{}", addr.ip, addr.prefixlen);
                println!("  Netmask: {}", addr.netmask);
                if let Some(broadcast) = addr.broadcast {
                    println!("  Broadcast: {}", broadcast);
                }
            }
            if_addrs::IfAddr::V6(addr) => {
                println!("  IPv6: {}/{}", addr.ip, addr.prefixlen);
                println!("  Netmask: {}", addr.netmask);
            }
        }

        println!("  Loopback: {}", iface.is_loopback());
        println!("  Point-to-Point: {}", iface.is_p2p);
        println!();
    }

    Ok(())
}
