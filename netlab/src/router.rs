use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use ipnet::Ipv4Net;
use sysctl::Sysctl;

use crate::{
    executor::RuntimeConfig,
    lan::Lan,
    nat::{self, NatRule, NatType},
    net::{LanKey, Net, RouterKey},
    node::Node,
};

#[derive(Debug, Clone)]
pub struct Router {
    net: Net,
    key: RouterKey,
    name: String,
}

#[derive(Debug)]
pub(crate) struct RouterEntry {
    pub(crate) lans: Vec<LanKey>,
    pub(crate) nat_lans: HashMap<LanKey, NatType>,
    pub(crate) node: Arc<Node>,
}

impl Router {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn attach(&self, lan: &Lan) -> Result<Ipv4Net> {
        self.net.ensure_same(lan.net())?;

        if let Some(address) = lan.router_address(self)? {
            return Ok(address);
        }

        self.enable_ipv4_forwarding().await?;

        let interface = lan.attach_node(self.node()).await?;
        let address = lan.allocate_address()?;

        interface.add_address(address.into()).await?;
        self.remember_lan(lan.key())?;
        lan.remember_router(self.key, address)?;

        Ok(address)
    }

    pub async fn enable_nat(&self, lan: &Lan, nat_type: NatType) -> Result<Ipv4Net> {
        self.net.ensure_same(lan.net())?;

        let address = self.attach(lan).await?;
        let rules = self.nat_rules_with(lan.key(), nat_type)?;

        self.apply_nat(rules).await?;
        self.remember_nat_lan(lan.key(), nat_type)?;

        Ok(address)
    }

    async fn enable_ipv4_forwarding(&self) -> Result<()> {
        self.node()
            .run_blocking(|| {
                let ip_forward = sysctl::Ctl::new("net.ipv4.ip_forward")
                    .context("failed to open net.ipv4.ip_forward sysctl")?;
                ip_forward
                    .set_value_string("1")
                    .context("failed to enable IPv4 forwarding")?;
                Ok(())
            })
            .await
    }

    async fn apply_nat(&self, rules: Vec<NatRule>) -> Result<()> {
        self.node()
            .run_blocking(move || nat::apply_nat(rules))
            .await
    }

    pub(crate) fn key(&self) -> RouterKey {
        self.key
    }

    pub(crate) fn net(&self) -> &Net {
        &self.net
    }

    fn node(&self) -> Arc<Node> {
        self.net
            .with_state(|state| Ok(state.routers[self.key].node.clone()))
            .expect("router is no longer registered in net")
    }

    fn remember_lan(&self, lan: LanKey) -> Result<()> {
        self.net.with_state_mut(|state| {
            state.routers[self.key].lans.push(lan);

            Ok(())
        })
    }

    fn nat_rules_with(&self, lan: LanKey, nat_type: NatType) -> Result<Vec<NatRule>> {
        self.net.with_state(|state| {
            let router = &state.routers[self.key];
            let mut lans = router.nat_lans.clone();

            lans.insert(lan, nat_type);

            Ok(lans
                .into_iter()
                .map(|(lan, nat_type)| NatRule {
                    network: state.lans[lan].network,
                    nat_type,
                })
                .collect())
        })
    }

    fn remember_nat_lan(&self, lan: LanKey, nat_type: NatType) -> Result<()> {
        self.net.with_state_mut(|state| {
            state.routers[self.key].nat_lans.insert(lan, nat_type);

            Ok(())
        })
    }

    pub(crate) async fn create(net: Net, name: &str, runtime: RuntimeConfig) -> Result<Self> {
        let node = Node::new(name, runtime).await?;
        let key = net.with_state_mut(|state| {
            Ok(state.routers.insert(RouterEntry {
                lans: Vec::new(),
                nat_lans: HashMap::new(),
                node,
            }))
        })?;

        Ok(Self {
            net,
            key,
            name: name.to_string(),
        })
    }
}
