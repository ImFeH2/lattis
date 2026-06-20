use std::sync::Arc;

use anyhow::{Context, Result};
use ipnet::Ipv4Net;
use sysctl::Sysctl;

use crate::{
    executor::RuntimeConfig,
    lan::Lan,
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
    pub(crate) node: Arc<Node>,
}

impl Router {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn attach(&self, lan: &Lan) -> Result<Ipv4Net> {
        self.net.ensure_same(lan.net())?;
        self.enable_ipv4_forwarding().await?;

        let interface = lan.attach_node(self.node()).await?;
        let address = lan.allocate_address()?;

        interface.add_address(address.into()).await?;
        self.remember_lan(lan.key())?;
        lan.remember_router(self.key, address)?;

        Ok(address)
    }

    pub async fn serve(&self, lan: &Lan) -> Result<()> {
        lan.ensure_gateway_available()?;

        let address = self.attach(lan).await?;
        lan.set_gateway(address.addr()).await?;

        Ok(())
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

    pub(crate) async fn create(net: Net, name: &str, runtime: RuntimeConfig) -> Result<Self> {
        let node = Node::new(name, runtime).await?;
        let key = net.with_state_mut(|state| {
            Ok(state.routers.insert(RouterEntry {
                lans: Vec::new(),
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
