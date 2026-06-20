use std::{collections::HashSet, net::SocketAddrV4, sync::Arc};

use anyhow::{Context, Result};
use ipnet::Ipv4Net;
use sysctl::Sysctl;

use crate::{
    nat::{self, NatRule, NatType, UdpNat},
    net::{LanKey, Net, RouterKey},
    network::netns::NamespaceNode,
    runtime::executor::RuntimeConfig,
    topology::lan::Lan,
};

#[derive(Debug, Clone)]
pub struct Router {
    net: Net,
    key: RouterKey,
    name: String,
}

#[derive(Debug)]
pub(crate) struct RouterEntry {
    pub(crate) lans: HashSet<LanKey>,
    pub(crate) masquerade_lans: HashSet<LanKey>,
    pub(crate) node: Arc<NamespaceNode>,
    pub(crate) udp_nats: Vec<UdpNat>,
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

    pub async fn enable_masquerade(&self, lan: &Lan) -> Result<Ipv4Net> {
        self.net.ensure_same(lan.net())?;

        let address = self.attach(lan).await?;
        let rules = self.masquerade_rules_with(lan.key())?;

        self.apply_masquerade(rules).await?;
        self.remember_masquerade_lan(lan.key())?;

        Ok(address)
    }

    pub async fn enable_udp_nat(
        &self,
        private_lan: &Lan,
        public_lan: &Lan,
        private_peer: SocketAddrV4,
        nat_type: NatType,
        remotes: Vec<(u16, SocketAddrV4)>,
    ) -> Result<()> {
        self.net.ensure_same(private_lan.net())?;
        self.net.ensure_same(public_lan.net())?;

        let private_addr = self.attach(private_lan).await?.addr();
        let public_addr = self.attach(public_lan).await?.addr();

        let nat = self
            .node()
            .executor
            .run(move || async move {
                UdpNat::start(private_addr, public_addr, private_peer, nat_type, remotes).await
            })
            .await?;

        self.remember_udp_nat(nat)
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

    async fn apply_masquerade(&self, rules: Vec<NatRule>) -> Result<()> {
        self.node()
            .run_blocking(move || nat::apply_masquerade(rules))
            .await
    }

    pub(crate) fn key(&self) -> RouterKey {
        self.key
    }

    pub(crate) fn net(&self) -> &Net {
        &self.net
    }

    fn node(&self) -> Arc<NamespaceNode> {
        self.net
            .with_state(|state| Ok(state.routers[self.key].node.clone()))
            .expect("router is no longer registered in net")
    }

    fn remember_lan(&self, lan: LanKey) -> Result<()> {
        self.net.with_state_mut(|state| {
            state.routers[self.key].lans.insert(lan);

            Ok(())
        })
    }

    fn masquerade_rules_with(&self, lan: LanKey) -> Result<Vec<NatRule>> {
        self.net.with_state(|state| {
            let router = &state.routers[self.key];
            let mut lans = router.masquerade_lans.clone();

            lans.insert(lan);

            Ok(lans
                .iter()
                .map(|lan| NatRule {
                    network: state.lans[*lan].network,
                })
                .collect())
        })
    }

    fn remember_masquerade_lan(&self, lan: LanKey) -> Result<()> {
        self.net.with_state_mut(|state| {
            state.routers[self.key].masquerade_lans.insert(lan);

            Ok(())
        })
    }

    fn remember_udp_nat(&self, nat: UdpNat) -> Result<()> {
        self.net.with_state_mut(|state| {
            state.routers[self.key].udp_nats.push(nat);

            Ok(())
        })
    }

    pub(crate) async fn create(net: Net, name: &str, runtime: RuntimeConfig) -> Result<Self> {
        let node = NamespaceNode::new(name, runtime).await?;
        let key = net.with_state_mut(|state| {
            Ok(state.routers.insert(RouterEntry {
                lans: HashSet::new(),
                masquerade_lans: HashSet::new(),
                node,
                udp_nats: Vec::new(),
            }))
        })?;

        Ok(Self {
            net,
            key,
            name: name.to_string(),
        })
    }
}
