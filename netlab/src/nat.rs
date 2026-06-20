use anyhow::{Context, Result};
use ipnet::Ipv4Net;
use nftables::{
    batch::Batch,
    expr::{CT, Expression, NamedExpression, Payload, PayloadField, Prefix},
    helper,
    schema::{Chain, FlushObject, NfCmd, NfListObject, NfObject, Rule, Table},
    stmt::{Match, Operator, Statement},
    types::{NfChainPolicy, NfChainType, NfFamily, NfHook},
};

const NAT_TABLE: &str = "netlab_nat";
const FORWARD_CHAIN: &str = "forward";
const POSTROUTING_CHAIN: &str = "postrouting";

pub(crate) fn apply_nat(networks: Vec<Ipv4Net>) -> Result<()> {
    let ruleset = helper::get_current_ruleset().context("failed to read nftables ruleset")?;
    let mut batch = Batch::new();

    if has_nat_table(&ruleset.objects) {
        batch.add_cmd(NfCmd::Flush(FlushObject::Table(nat_table())));
        batch.delete(NfListObject::Table(nat_table()));
    }

    batch.add(NfListObject::Table(nat_table()));
    batch.add(NfListObject::Chain(forward_chain()));
    batch.add(NfListObject::Chain(postrouting_chain()));

    for network in networks {
        batch.add(NfListObject::Rule(established_inbound_rule(network)));
        batch.add(NfListObject::Rule(private_outbound_rule(network)));
        batch.add(NfListObject::Rule(new_inbound_drop_rule(network)));
        batch.add(NfListObject::Rule(masquerade_rule(network)));
    }

    helper::apply_ruleset(&batch.to_nftables()).context("failed to apply nftables nat rules")
}

fn has_nat_table(objects: &[NfObject<'_>]) -> bool {
    objects.iter().any(|object| match object {
        NfObject::ListObject(NfListObject::Table(table)) => {
            table.family == NfFamily::IP && table.name == NAT_TABLE
        }
        NfObject::CmdObject(_) => false,
        _ => false,
    })
}

fn established_inbound_rule(network: Ipv4Net) -> Rule<'static> {
    Rule {
        family: NfFamily::IP,
        table: NAT_TABLE.into(),
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
        table: NAT_TABLE.into(),
        chain: POSTROUTING_CHAIN.into(),
        expr: vec![source_match(network), Statement::Masquerade(None)].into(),
        ..Default::default()
    }
}

fn new_inbound_drop_rule(network: Ipv4Net) -> Rule<'static> {
    Rule {
        family: NfFamily::IP,
        table: NAT_TABLE.into(),
        chain: FORWARD_CHAIN.into(),
        expr: vec![destination_match(network), Statement::Drop(None)].into(),
        ..Default::default()
    }
}

fn private_outbound_rule(network: Ipv4Net) -> Rule<'static> {
    Rule {
        family: NfFamily::IP,
        table: NAT_TABLE.into(),
        chain: FORWARD_CHAIN.into(),
        expr: vec![source_match(network), Statement::Accept(None)].into(),
        ..Default::default()
    }
}

fn nat_table() -> Table<'static> {
    Table {
        family: NfFamily::IP,
        name: NAT_TABLE.into(),
        handle: None,
    }
}

fn forward_chain() -> Chain<'static> {
    Chain {
        family: NfFamily::IP,
        table: NAT_TABLE.into(),
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
        table: NAT_TABLE.into(),
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
