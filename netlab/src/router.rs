use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result};
use ipnet::Ipv4Net;
use sysctl::Sysctl;

use crate::{
    executor::RuntimeConfig,
    lan::Lan,
    nat,
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
    pub(crate) nat_lans: HashSet<LanKey>,
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

    pub async fn enable_nat(&self, lan: &Lan) -> Result<Ipv4Net> {
        self.net.ensure_same(lan.net())?;

        let address = self.attach(lan).await?;
        let networks = self.nat_networks_with(lan.key())?;

        self.apply_nat(networks).await?;
        self.remember_nat_lan(lan.key())?;

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

    async fn apply_nat(&self, networks: Vec<Ipv4Net>) -> Result<()> {
        self.node()
            .run_blocking(move || nat::apply_nat(networks))
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

    fn nat_networks_with(&self, lan: LanKey) -> Result<Vec<Ipv4Net>> {
        self.net.with_state(|state| {
            let router = &state.routers[self.key];
            let mut lans = router.nat_lans.iter().copied().collect::<Vec<_>>();

            if !lans.contains(&lan) {
                lans.push(lan);
            }

            Ok(lans
                .into_iter()
                .map(|lan| state.lans[lan].network)
                .collect())
        })
    }

    fn remember_nat_lan(&self, lan: LanKey) -> Result<()> {
        self.net.with_state_mut(|state| {
            state.routers[self.key].nat_lans.insert(lan);

            Ok(())
        })
    }

    pub(crate) async fn create(net: Net, name: &str, runtime: RuntimeConfig) -> Result<Self> {
        let node = Node::new(name, runtime).await?;
        let key = net.with_state_mut(|state| {
            Ok(state.routers.insert(RouterEntry {
                lans: Vec::new(),
                nat_lans: HashSet::new(),
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
