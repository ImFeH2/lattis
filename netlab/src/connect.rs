use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use rtnetlink::{LinkUnspec, LinkVeth};

use crate::{
    interface::Interface,
    netlink::{allocate_veth_names, link_index},
    node::Node,
};

#[async_trait]
pub trait Connectable: private::ConnectableInternals {
    async fn connect<T>(&self, peer: &T) -> Result<(Interface, Interface)>
    where
        T: Connectable + ?Sized,
    {
        let left_node = self.node();
        let right_node = peer.node();
        let right_namespace = right_node.namespace.raw_fd();

        let (left_name, right_name) = left_node
            .run_netlink(move |handle| async move {
                let (left_name, right_name) = allocate_veth_names(&handle).await?;

                let veth = LinkVeth::new(&left_name, &right_name).build();
                handle.link().add(veth).execute().await?;

                let right_index = link_index(&handle, &right_name).await?;

                handle
                    .link()
                    .set(
                        LinkUnspec::new_with_index(right_index)
                            .setns_by_fd(right_namespace)
                            .build(),
                    )
                    .execute()
                    .await?;

                Ok((left_name, right_name))
            })
            .await?;

        let left_interface = Interface::new(left_name, left_node).await?;
        let right_interface = Interface::new(right_name, right_node).await?;

        self.on_connected(&left_interface).await?;
        peer.on_connected(&right_interface).await?;

        Ok((left_interface, right_interface))
    }
}

impl<T> Connectable for T where T: private::ConnectableInternals {}

pub(crate) use private::ConnectableInternals;

mod private {
    use super::*;

    #[async_trait]
    pub trait ConnectableInternals: Sync {
        fn node(&self) -> Arc<Node>;

        async fn on_connected(&self, _interface: &Interface) -> Result<()> {
            Ok(())
        }
    }
}
