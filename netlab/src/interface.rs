use std::{net::Ipv4Addr, sync::Arc};

use anyhow::Result;
use ipnet::IpNet;
use rtnetlink::{LinkUnspec, RouteMessageBuilder};

use crate::{netlink::link_index, netns::NamespaceNode};

#[derive(Debug, Clone)]
pub(crate) struct Interface {
    name: String,
    node: Arc<NamespaceNode>,
}

impl Interface {
    pub(crate) async fn new(name: String, node: Arc<NamespaceNode>) -> Result<Self> {
        let interface = Self { name, node };

        interface.up().await?;

        Ok(interface)
    }

    pub(crate) async fn index(&self) -> Result<u32> {
        let name = self.name.clone();

        self.node
            .run_netlink(move |handle| async move { link_index(&handle, &name).await })
            .await
    }

    async fn up(&self) -> Result<()> {
        let index = self.index().await?;

        self.node
            .run_netlink(move |handle| async move {
                handle
                    .link()
                    .set(LinkUnspec::new_with_index(index).up().build())
                    .execute()
                    .await?;
                Ok(())
            })
            .await
    }

    pub(crate) async fn add_address(&self, address: IpNet) -> Result<()> {
        let index = self.index().await?;
        let ip = address.addr();
        let prefix_len = address.prefix_len();

        self.node
            .run_netlink(move |handle| async move {
                handle
                    .address()
                    .add(index, ip, prefix_len)
                    .execute()
                    .await?;
                Ok(())
            })
            .await
    }

    pub(crate) async fn set_default_route(&self, gateway: Ipv4Addr) -> Result<()> {
        let index = self.index().await?;

        self.node
            .run_netlink(move |handle| async move {
                let route = RouteMessageBuilder::<Ipv4Addr>::new()
                    .gateway(gateway)
                    .output_interface(index)
                    .build();

                handle.route().add(route).replace().execute().await?;

                Ok(())
            })
            .await
    }
}
