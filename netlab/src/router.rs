use std::sync::Arc;

use anyhow::{Context, Result};
use sysctl::Sysctl;

use crate::{executor::RuntimeConfig, lan::Lan, node::Node};

#[derive(Debug, Clone)]
pub struct Router {
    node: Arc<Node>,
}

#[derive(Debug, Clone)]
pub struct RouterBuilder {
    name: String,
    runtime: RuntimeConfig,
}

impl Router {
    pub async fn new() -> Result<Self> {
        Self::builder().build().await
    }

    pub fn builder() -> RouterBuilder {
        RouterBuilder::new()
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

impl RouterBuilder {
    fn new() -> Self {
        Self {
            name: "router".to_string(),
            runtime: RuntimeConfig::CurrentThread,
        }
    }

    pub async fn build(self) -> Result<Router> {
        Ok(Router {
            node: Node::new(&self.name, self.runtime).await?,
        })
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn runtime(mut self, runtime: RuntimeConfig) -> Self {
        self.runtime = runtime;
        self
    }
}
