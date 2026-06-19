use anyhow::Result;
use rtnetlink::{LinkUnspec, LinkVeth};

use crate::{
    host::Host,
    interface::Interface,
    netlink::{allocate_veth_names, link_index},
};

pub struct DirectLink;

impl DirectLink {
    pub async fn connect(host1: &Host, host2: &Host) -> Result<(Interface, Interface)> {
        let (connection, handle, _) = rtnetlink::new_connection()?;
        tokio::spawn(connection);

        let (name1, name2) = allocate_veth_names(&handle).await?;

        let veth = LinkVeth::new(&name1, &name2).build();
        handle.link().add(veth).execute().await?;

        let index1 = link_index(&handle, &name1).await?;
        let index2 = link_index(&handle, &name2).await?;

        handle
            .link()
            .set(
                LinkUnspec::new_with_index(index1)
                    .setns_by_fd(host1.node.namespace.raw_fd())
                    .build(),
            )
            .execute()
            .await?;

        handle
            .link()
            .set(
                LinkUnspec::new_with_index(index2)
                    .setns_by_fd(host2.node.namespace.raw_fd())
                    .build(),
            )
            .execute()
            .await?;

        Ok((
            Interface::new(name1, host1.clone()),
            Interface::new(name2, host2.clone()),
        ))
    }

    pub async fn connect_named(
        host1: &Host,
        name1: &str,
        host2: &Host,
        name2: &str,
    ) -> Result<(Interface, Interface)> {
        let (mut iface1, mut iface2) = Self::connect(host1, host2).await?;

        iface1.rename(name1).await?;
        iface2.rename(name2).await?;

        Ok((iface1, iface2))
    }
}
