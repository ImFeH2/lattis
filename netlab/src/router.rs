use std::sync::Arc;

use anyhow::{Context, Result};
use sysctl::Sysctl;

use crate::{executor::RuntimeConfig, lan::Lan, node::Node};

#[derive(Debug, Clone)]
pub struct Router {
    node: Arc<Node>,
}

impl Router {
    pub async fn new() -> Result<Self> {
        Self::named("router").await
    }

    pub async fn named(name: &str) -> Result<Self> {
        Ok(Self {
            node: Node::new(name, RuntimeConfig::CurrentThread).await?,
        })
    }

    pub fn name(&self) -> &str {
        &self.node.label
    }

    pub async fn serve(&self, lan: &Lan) -> Result<()> {
        self.enable_ipv4_forwarding().await?;
        lan.ensure_gateway_available()?;

        let interface = lan.attach_node(self.node.clone()).await?;
        let address = lan.allocate_address()?;

        interface.add_address(address.into()).await?;
        lan.set_gateway(address.addr()).await?;

        Ok(())
    }

    async fn enable_ipv4_forwarding(&self) -> Result<()> {
        self.node
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
}
