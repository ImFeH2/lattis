use std::{future::Future, sync::Arc};

use anyhow::Result;
use rtnetlink::{LinkUnspec, LinkVeth};

use crate::{
    executor::{HostTask, RuntimeConfig},
    interface::Interface,
    netlink::{allocate_veth_names, link_index},
    node::Node,
};

#[derive(Debug, Clone)]
pub struct Host {
    pub(crate) node: Arc<Node>,
}

impl Host {
    pub async fn new(name: &str) -> Result<Self> {
        Self::new_with_runtime(name, RuntimeConfig::CurrentThread).await
    }

    pub async fn new_with_runtime(name: &str, config: RuntimeConfig) -> Result<Self> {
        Ok(Self {
            node: Node::new(name, config).await?,
        })
    }

    pub fn name(&self) -> &str {
        &self.node.label
    }

    pub async fn connect(&self, peer: &Host) -> Result<(Interface, Interface)> {
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
                    .setns_by_fd(self.node.namespace.raw_fd())
                    .build(),
            )
            .execute()
            .await?;

        handle
            .link()
            .set(
                LinkUnspec::new_with_index(index2)
                    .setns_by_fd(peer.node.namespace.raw_fd())
                    .build(),
            )
            .execute()
            .await?;

        let iface1 = Interface::new(name1, self.clone()).await?;
        let iface2 = Interface::new(name2, peer.clone()).await?;

        Ok((iface1, iface2))
    }

    pub fn spawn<T, F, Fut>(&self, f: F) -> Result<HostTask<T>>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.node.executor.spawn(f)
    }

    pub async fn run<T, F, Fut>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.node.executor.run(f).await
    }

    pub fn spawn_blocking<T, F>(&self, f: F) -> Result<HostTask<T>>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        self.node.executor.spawn_blocking(f)
    }
}
