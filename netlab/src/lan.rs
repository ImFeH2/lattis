use std::sync::Arc;

use anyhow::Result;
use rtnetlink::{LinkBridge, LinkUnspec, LinkVeth};

use crate::{
    executor::RuntimeConfig,
    host::Host,
    interface::Interface,
    netlink::{allocate_lan_name, allocate_veth_names, link_index},
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

    pub async fn connect(&self, host: &Host) -> Result<Interface> {
        let bridge_index = self.index;
        let host = host.clone();
        let host_namespace = host.node.namespace.raw_fd();

        let host_name = self
            .node
            .run_netlink(move |handle| async move {
                let (host_name, bridge_name) = allocate_veth_names(&handle).await?;
                let veth = LinkVeth::new(&host_name, &bridge_name).build();
                handle.link().add(veth).execute().await?;

                let host_index = link_index(&handle, &host_name).await?;
                let port_index = link_index(&handle, &bridge_name).await?;

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

                handle
                    .link()
                    .set(
                        LinkUnspec::new_with_index(host_index)
                            .setns_by_fd(host_namespace)
                            .build(),
                    )
                    .execute()
                    .await?;

                Ok(host_name)
            })
            .await?;

        let interface = Interface::new(host_name, host).await?;

        Ok(interface)
    }

    pub async fn connect_named(&self, host: &Host, name: &str) -> Result<Interface> {
        let mut interface = self.connect(host).await?;

        interface.rename(name).await?;

        Ok(interface)
    }

    pub fn name(&self) -> &str {
        &self.node.label
    }
}
