use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use rtnetlink::{LinkBridge, LinkUnspec};

use crate::{
    connect::ConnectableInternals,
    executor::RuntimeConfig,
    interface::Interface,
    netlink::{allocate_lan_name, link_index},
    node::Node,
};

#[derive(Debug)]
pub struct Lan {
    index: u32,
    node: Arc<Node>,
}

impl Lan {
    pub async fn new(name: &str) -> Result<Self> {
        let node = Node::new(name, RuntimeConfig::CurrentThread).await?;

        let label = name.to_string();
        let index = node
            .run_netlink(move |handle| async move {
                let bridge = allocate_lan_name(&label, &handle).await?;
                handle
                    .link()
                    .add(
                        LinkBridge::new(&bridge)
                            .nf_call_iptables(false)
                            .nf_call_ip6tables(false)
                            .nf_call_arptables(false)
                            .up()
                            .build(),
                    )
                    .execute()
                    .await?;

                link_index(&handle, &bridge).await
            })
            .await?;

        Ok(Self { index, node })
    }

    pub fn name(&self) -> &str {
        &self.node.label
    }
}

#[async_trait]
impl ConnectableInternals for Lan {
    fn node(&self) -> Arc<Node> {
        self.node.clone()
    }

    async fn on_connected(&self, interface: &Interface) -> Result<()> {
        let bridge_index = self.index;
        let port_index = interface.index().await?;

        self.node
            .run_netlink(move |handle| async move {
                handle
                    .link()
                    .set(
                        LinkUnspec::new_with_index(port_index)
                            .controller(bridge_index)
                            .up()
                            .build(),
                    )
                    .execute()
                    .await?;

                Ok(())
            })
            .await
    }
}
