use std::net::{Ipv4Addr, SocketAddrV4};

use anyhow::Result;
use netlab::{NatTable, NatType};

const PRIVATE_SOURCE: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 2), 5000);
const OTHER_PRIVATE_SOURCE: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 3), 5000);
const PUBLIC_ADDR: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 10);
const REMOTE_ONE: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 1), 3478);
const REMOTE_ONE_OTHER_PORT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 1), 4000);
const REMOTE_TWO: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 2), 3478);
const FIRST_DYNAMIC_PORT: u16 = 49152;

#[test]
fn full_cone_reuses_mapping_for_different_remotes() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::FullCone);

    let first = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let second = table.translate_outbound(PRIVATE_SOURCE, REMOTE_TWO)?;

    assert_eq!(first.source, SocketAddrV4::new(PUBLIC_ADDR, 5000));
    assert_eq!(second.source, first.source);

    Ok(())
}

#[test]
fn full_cone_allows_any_remote_after_mapping_exists() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::FullCone);

    let outbound = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let inbound = table
        .translate_inbound(REMOTE_TWO, outbound.source)
        .expect("full cone mapping should accept any remote");

    assert_eq!(inbound.source, REMOTE_TWO);
    assert_eq!(inbound.destination, PRIVATE_SOURCE);

    Ok(())
}

#[test]
fn full_cone_rejects_unknown_public_ports() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::FullCone);

    table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let unknown_mapping = SocketAddrV4::new(PUBLIC_ADDR, 60000);

    assert!(
        table
            .translate_inbound(REMOTE_TWO, unknown_mapping)
            .is_none()
    );

    Ok(())
}

#[test]
fn restricted_cone_reuses_mapping_for_different_remotes() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::RestrictedCone);

    let first = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let second = table.translate_outbound(PRIVATE_SOURCE, REMOTE_TWO)?;

    assert_eq!(first.source, SocketAddrV4::new(PUBLIC_ADDR, 5000));
    assert_eq!(second.source, first.source);

    Ok(())
}

#[test]
fn restricted_cone_allows_known_address_from_different_port() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::RestrictedCone);

    let outbound = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let inbound = table
        .translate_inbound(REMOTE_ONE_OTHER_PORT, outbound.source)
        .expect("restricted cone mapping should accept known remote address");

    assert_eq!(inbound.source, REMOTE_ONE_OTHER_PORT);
    assert_eq!(inbound.destination, PRIVATE_SOURCE);

    Ok(())
}

#[test]
fn restricted_cone_rejects_unknown_addresses() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::RestrictedCone);

    let outbound = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;

    assert!(
        table
            .translate_inbound(REMOTE_TWO, outbound.source)
            .is_none()
    );

    Ok(())
}

#[test]
fn port_restricted_cone_reuses_mapping_for_different_remotes() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::PortRestrictedCone);

    let first = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let second = table.translate_outbound(PRIVATE_SOURCE, REMOTE_TWO)?;

    assert_eq!(first.source, SocketAddrV4::new(PUBLIC_ADDR, 5000));
    assert_eq!(second.source, first.source);

    Ok(())
}

#[test]
fn port_restricted_cone_allows_known_endpoint() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::PortRestrictedCone);

    let outbound = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let inbound = table
        .translate_inbound(REMOTE_ONE, outbound.source)
        .expect("port restricted cone mapping should accept known remote endpoint");

    assert_eq!(inbound.source, REMOTE_ONE);
    assert_eq!(inbound.destination, PRIVATE_SOURCE);

    Ok(())
}

#[test]
fn port_restricted_cone_rejects_known_address_with_different_port() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::PortRestrictedCone);

    let outbound = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;

    assert!(
        table
            .translate_inbound(REMOTE_ONE_OTHER_PORT, outbound.source)
            .is_none()
    );

    Ok(())
}

#[test]
fn symmetric_nat_uses_destination_specific_mappings() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::Symmetric);

    let first = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let second = table.translate_outbound(PRIVATE_SOURCE, REMOTE_TWO)?;

    assert_eq!(first.source, SocketAddrV4::new(PUBLIC_ADDR, 5000));
    assert_eq!(
        second.source,
        SocketAddrV4::new(PUBLIC_ADDR, FIRST_DYNAMIC_PORT)
    );

    Ok(())
}

#[test]
fn symmetric_nat_allows_matching_destination_endpoint() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::Symmetric);

    let outbound = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let inbound = table
        .translate_inbound(REMOTE_ONE, outbound.source)
        .expect("symmetric mapping should accept matching remote endpoint");

    assert_eq!(inbound.source, REMOTE_ONE);
    assert_eq!(inbound.destination, PRIVATE_SOURCE);

    Ok(())
}

#[test]
fn symmetric_nat_rejects_different_remote_on_an_existing_mapping() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::Symmetric);

    let first = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let second = table.translate_outbound(PRIVATE_SOURCE, REMOTE_TWO)?;

    assert!(table.translate_inbound(REMOTE_TWO, first.source).is_none());
    assert!(table.translate_inbound(REMOTE_ONE, second.source).is_none());

    Ok(())
}

#[test]
fn port_conflicts_allocate_dynamic_ports() -> Result<()> {
    let mut table = NatTable::new(PUBLIC_ADDR, NatType::FullCone);

    let first = table.translate_outbound(PRIVATE_SOURCE, REMOTE_ONE)?;
    let second = table.translate_outbound(OTHER_PRIVATE_SOURCE, REMOTE_ONE)?;

    assert_eq!(first.source, SocketAddrV4::new(PUBLIC_ADDR, 5000));
    assert_eq!(
        second.source,
        SocketAddrV4::new(PUBLIC_ADDR, FIRST_DYNAMIC_PORT)
    );

    Ok(())
}
