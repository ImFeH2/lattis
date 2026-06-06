use anyhow::Result;
use if_addrs::get_if_addrs;

pub fn print_info() -> Result<()> {
    let interfaces = get_if_addrs()?;

    for iface in interfaces {
        println!("Interface: {}", iface.name);
        println!("  ip: {}", iface.ip());
        println!("  loopback: {}", iface.is_loopback());
        println!("  p2p: {}", iface.is_p2p);
        println!();
    }

    Ok(())
}
