use anyhow::Result;
use ipnet::IpNet;
use rtnetlink::LinkUnspec;

use crate::{host::Host, netlink::link_index};

#[derive(Debug)]
pub struct Interface {
    pub(crate) name: String,
    pub(crate) host: Host,
}

impl Interface {
    pub(crate) fn new(name: String, host: Host) -> Self {
        Self { name, host }
    }

    pub async fn index(&self) -> Result<u32> {
        let name = self.name.clone();

        self.host
            .node
            .run_netlink(move |handle| async move { link_index(&handle, &name).await })
            .await
    }

    pub async fn up(&self) -> Result<()> {
        let index = self.index().await?;

        self.host
            .node
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

    pub async fn down(&self) -> Result<()> {
        let index = self.index().await?;

        self.host
            .node
            .run_netlink(move |handle| async move {
                handle
                    .link()
                    .set(LinkUnspec::new_with_index(index).down().build())
                    .execute()
                    .await?;
                Ok(())
            })
            .await
    }

    pub async fn add_address(&self, address: IpNet) -> Result<()> {
        let index = self.index().await?;
        let ip = address.addr();
        let prefix_len = address.prefix_len();

        self.host
            .node
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

    pub async fn rename(&mut self, new_name: &str) -> Result<()> {
        let index = self.index().await?;
        let new_name = new_name.to_string();
        let link_name = new_name.clone();

        self.host
            .node
            .run_netlink(move |handle| async move {
                handle
                    .link()
                    .set(LinkUnspec::new_with_index(index).name(link_name).build())
                    .execute()
                    .await?;

                Ok(())
            })
            .await?;

        self.name = new_name;
        Ok(())
    }
}
