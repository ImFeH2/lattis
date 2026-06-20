use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddrV4},
};

use anyhow::{Context, Result, anyhow};
use ipnet::Ipv4Net;
use nftables::{
    batch::Batch,
    expr::{CT, Expression, NamedExpression, Payload, PayloadField, Prefix},
    helper,
    schema::{Chain, FlushObject, NfCmd, NfListObject, NfObject, Rule, Table},
    stmt::{Match, Operator, Statement},
    types::{NfChainPolicy, NfChainType, NfFamily, NfHook},
};

const MASQUERADE_TABLE: &str = "netlab_masquerade";
const FORWARD_CHAIN: &str = "forward";
const POSTROUTING_CHAIN: &str = "postrouting";
const FIRST_DYNAMIC_PORT: u16 = 49152;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NatType {
    FullCone,
    RestrictedCone,
    PortRestrictedCone,
    Symmetric,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NatRule {
    pub(crate) network: Ipv4Net,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UdpTranslation {
    pub destination: SocketAddrV4,
    pub source: SocketAddrV4,
}

#[derive(Debug)]
pub struct NatTable {
    mappings: HashMap<NatMappingKey, NatMapping>,
    nat_type: NatType,
    next_port: u16,
    ports: HashMap<u16, NatMappingKey>,
    public_addr: Ipv4Addr,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct NatMappingKey {
    destination: Option<SocketAddrV4>,
    source: SocketAddrV4,
}

#[derive(Debug)]
struct NatMapping {
    contacts: HashSet<SocketAddrV4>,
    public: SocketAddrV4,
    source: SocketAddrV4,
}

pub(crate) fn apply_masquerade(rules: Vec<NatRule>) -> Result<()> {
    let ruleset = helper::get_current_ruleset().context("failed to read nftables ruleset")?;
    let mut batch = Batch::new();

    if has_masquerade_table(&ruleset.objects) {
        batch.add_cmd(NfCmd::Flush(FlushObject::Table(masquerade_table())));
        batch.delete(NfListObject::Table(masquerade_table()));
    }

    batch.add(NfListObject::Table(masquerade_table()));
    batch.add(NfListObject::Chain(forward_chain()));
    batch.add(NfListObject::Chain(postrouting_chain()));

    for rule in rules {
        batch.add(NfListObject::Rule(established_inbound_rule(rule.network)));
        batch.add(NfListObject::Rule(private_outbound_rule(rule.network)));
        batch.add(NfListObject::Rule(new_inbound_drop_rule(rule.network)));
        batch.add(NfListObject::Rule(masquerade_rule(rule.network)));
    }

    helper::apply_ruleset(&batch.to_nftables()).context("failed to apply nftables nat rules")
}

impl NatTable {
    pub fn new(public_addr: Ipv4Addr, nat_type: NatType) -> Self {
        Self {
            mappings: HashMap::new(),
            nat_type,
            next_port: FIRST_DYNAMIC_PORT,
            ports: HashMap::new(),
            public_addr,
        }
    }

    pub fn translate_inbound(
        &self,
        source: SocketAddrV4,
        destination: SocketAddrV4,
    ) -> Option<UdpTranslation> {
        if *destination.ip() != self.public_addr {
            return None;
        }

        let key = self.ports.get(&destination.port())?;
        let mapping = self.mappings.get(key)?;

        if !self.nat_type.allows_inbound(&mapping.contacts, source) {
            return None;
        }

        Some(UdpTranslation {
            destination: mapping.source,
            source,
        })
    }

    pub fn translate_outbound(
        &mut self,
        source: SocketAddrV4,
        destination: SocketAddrV4,
    ) -> Result<UdpTranslation> {
        let key = self.nat_type.mapping_key(source, destination);

        if let Some(mapping) = self.mappings.get_mut(&key) {
            mapping.contacts.insert(destination);

            return Ok(UdpTranslation {
                destination,
                source: mapping.public,
            });
        }

        let public = SocketAddrV4::new(self.public_addr, self.allocate_port(source.port())?);
        self.ports.insert(public.port(), key);
        self.mappings.insert(
            key,
            NatMapping {
                contacts: HashSet::from([destination]),
                public,
                source,
            },
        );

        Ok(UdpTranslation {
            destination,
            source: public,
        })
    }

    fn allocate_port(&mut self, preferred: u16) -> Result<u16> {
        if preferred != 0 && !self.ports.contains_key(&preferred) {
            return Ok(preferred);
        }

        for _ in FIRST_DYNAMIC_PORT..=u16::MAX {
            let port = self.next_port;
            self.next_port = if self.next_port == u16::MAX {
                FIRST_DYNAMIC_PORT
            } else {
                self.next_port + 1
            };

            if !self.ports.contains_key(&port) {
                return Ok(port);
            }
        }

        Err(anyhow!("nat port pool is exhausted"))
    }
}

impl NatType {
    fn allows_inbound(self, contacts: &HashSet<SocketAddrV4>, source: SocketAddrV4) -> bool {
        match self {
            Self::FullCone => true,
            Self::RestrictedCone => contacts.iter().any(|contact| contact.ip() == source.ip()),
            Self::PortRestrictedCone | Self::Symmetric => contacts.contains(&source),
        }
    }

    fn mapping_key(self, source: SocketAddrV4, destination: SocketAddrV4) -> NatMappingKey {
        NatMappingKey {
            destination: match self {
                Self::FullCone | Self::RestrictedCone | Self::PortRestrictedCone => None,
                Self::Symmetric => Some(destination),
            },
            source,
        }
    }
}

fn has_masquerade_table(objects: &[NfObject<'_>]) -> bool {
    objects.iter().any(|object| match object {
        NfObject::ListObject(NfListObject::Table(table)) => {
            table.family == NfFamily::IP && table.name == MASQUERADE_TABLE
        }
        NfObject::CmdObject(_) => false,
        _ => false,
    })
}

fn established_inbound_rule(network: Ipv4Net) -> Rule<'static> {
    Rule {
        family: NfFamily::IP,
        table: MASQUERADE_TABLE.into(),
        chain: FORWARD_CHAIN.into(),
        expr: vec![
            conntrack_state_match(["established", "related"]),
            destination_match(network),
            Statement::Accept(None),
        ]
        .into(),
        ..Default::default()
    }
}

fn masquerade_rule(network: Ipv4Net) -> Rule<'static> {
    Rule {
        family: NfFamily::IP,
        table: MASQUERADE_TABLE.into(),
        chain: POSTROUTING_CHAIN.into(),
        expr: vec![source_match(network), Statement::Masquerade(None)].into(),
        ..Default::default()
    }
}

fn new_inbound_drop_rule(network: Ipv4Net) -> Rule<'static> {
    Rule {
        family: NfFamily::IP,
        table: MASQUERADE_TABLE.into(),
        chain: FORWARD_CHAIN.into(),
        expr: vec![destination_match(network), Statement::Drop(None)].into(),
        ..Default::default()
    }
}

fn private_outbound_rule(network: Ipv4Net) -> Rule<'static> {
    Rule {
        family: NfFamily::IP,
        table: MASQUERADE_TABLE.into(),
        chain: FORWARD_CHAIN.into(),
        expr: vec![source_match(network), Statement::Accept(None)].into(),
        ..Default::default()
    }
}

fn masquerade_table() -> Table<'static> {
    Table {
        family: NfFamily::IP,
        name: MASQUERADE_TABLE.into(),
        handle: None,
    }
}

fn forward_chain() -> Chain<'static> {
    Chain {
        family: NfFamily::IP,
        table: MASQUERADE_TABLE.into(),
        name: FORWARD_CHAIN.into(),
        _type: Some(NfChainType::Filter),
        hook: Some(NfHook::Forward),
        prio: Some(0),
        policy: Some(NfChainPolicy::Accept),
        ..Default::default()
    }
}

fn postrouting_chain() -> Chain<'static> {
    Chain {
        family: NfFamily::IP,
        table: MASQUERADE_TABLE.into(),
        name: POSTROUTING_CHAIN.into(),
        _type: Some(NfChainType::NAT),
        hook: Some(NfHook::Postrouting),
        prio: Some(100),
        policy: Some(NfChainPolicy::Accept),
        ..Default::default()
    }
}

fn conntrack_state_match(states: [&'static str; 2]) -> Statement<'static> {
    Statement::Match(Match {
        left: Expression::Named(NamedExpression::CT(CT {
            key: "state".into(),
            family: None,
            dir: None,
        })),
        right: Expression::List(
            states
                .into_iter()
                .map(|state| Expression::String(state.into()))
                .collect(),
        ),
        op: Operator::IN,
    })
}

fn destination_match(network: Ipv4Net) -> Statement<'static> {
    address_match("daddr", network)
}

fn source_match(network: Ipv4Net) -> Statement<'static> {
    address_match("saddr", network)
}

fn address_match(field: &'static str, network: Ipv4Net) -> Statement<'static> {
    Statement::Match(Match {
        left: Expression::Named(NamedExpression::Payload(Payload::PayloadField(
            PayloadField {
                protocol: "ip".into(),
                field: field.into(),
            },
        ))),
        right: Expression::Named(NamedExpression::Prefix(Prefix {
            addr: Box::new(Expression::String(network.network().to_string().into())),
            len: network.prefix_len() as u32,
        })),
        op: Operator::EQ,
    })
}
