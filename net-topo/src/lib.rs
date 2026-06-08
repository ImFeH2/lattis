mod executor;
mod netns;

use std::{
    future::Future,
    net::IpAddr,
    sync::{Arc, atomic::AtomicU64},
};

use anyhow::{Context, Result};
use futures_util::stream::TryStreamExt;
use netns::NetworkNamespace;
use rtnetlink::{LinkUnspec, LinkVeth, packet_route::link::LinkAttribute};

use crate::executor::NamespaceExecutor;
pub use crate::executor::{HostTask, RuntimeConfig};

#[derive(Debug)]
struct Node {
    executor: NamespaceExecutor,
    namespace: NetworkNamespace,
}

impl Node {
    async fn run_netlink<T, F, Fut>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(rtnetlink::Handle) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.executor
            .run(move || async move {
                let (connection, handle, _) = rtnetlink::new_connection()?;
                tokio::spawn(connection);
                f(handle).await
            })
            .await
    }
}

#[derive(Debug, Clone)]
pub struct Host {
    node: Arc<Node>,
}

impl Host {
    pub async fn new(name: &str) -> Result<Self> {
        Self::new_with_runtime(name, RuntimeConfig::CurrentThread).await
    }

    pub async fn new_with_runtime(name: &str, config: RuntimeConfig) -> Result<Self> {
        let namespace = NetworkNamespace::new(name).await?;
        let executor = NamespaceExecutor::new(&namespace, config).await?;

        Ok(Self {
            node: Arc::new(Node {
                executor,
                namespace,
            }),
        })
    }

    pub fn name(&self) -> &str {
        &self.node.namespace.name
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

static VETH_ID: AtomicU64 = AtomicU64::new(0);

async fn link_exists(handle: &rtnetlink::Handle, name: &str) -> Result<bool> {
    let mut links = handle.link().get().execute();

    while let Some(link) = links.try_next().await? {
        if link
            .attributes
            .iter()
            .any(|attr| matches!(attr, LinkAttribute::IfName(if_name) if if_name == name))
        {
            return Ok(true);
        }
    }

    Ok(false)
}

async fn allocate_veth_names(handle: &rtnetlink::Handle) -> Result<(String, String)> {
    loop {
        let id = VETH_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name1 = format!("veth{}a", id);
        let name2 = format!("veth{}b", id);

        if !link_exists(handle, &name1).await? && !link_exists(handle, &name2).await? {
            return Ok((name1, name2));
        }
    }
}

pub struct DirectLink;

impl DirectLink {
    pub async fn connect(host1: &Host, host2: &Host) -> Result<(Interface, Interface)> {
        let (connection, handle, _) = rtnetlink::new_connection()?;
        tokio::spawn(connection);

        let (name1, name2) = allocate_veth_names(&handle).await?;

        let veth = LinkVeth::new(&name1, &name2).build();
        handle.link().add(veth).execute().await?;

        let index1 = handle
            .link()
            .get()
            .match_name(name1.clone())
            .execute()
            .try_next()
            .await?
            .context(format!("failed to find link: {}", name1))?
            .header
            .index;

        let index2 = handle
            .link()
            .get()
            .match_name(name2.clone())
            .execute()
            .try_next()
            .await?
            .context(format!("failed to find link: {}", name2))?
            .header
            .index;

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

        let iface1 = Interface {
            name: name1.clone(),
            host: host1.clone(),
        };
        let iface2 = Interface {
            name: name2.clone(),
            host: host2.clone(),
        };

        Ok((iface1, iface2))
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

#[derive(Debug)]
pub struct Interface {
    name: String,
    host: Host,
}

impl Interface {
    pub fn configure(&self) -> InterfaceConfig<'_> {
        InterfaceConfig {
            interface: self,
            address: Vec::new(),
            up: None,
        }
    }

    pub async fn index(&self) -> Result<u32> {
        let name = self.name.clone();

        self.host
            .node
            .run_netlink(move |handle| async move {
                let mut links = handle.link().get().match_name(name.clone()).execute();
                let link = links
                    .try_next()
                    .await?
                    .context(format!("failed to find link: {}", name))?;
                Ok(link.header.index)
            })
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

    pub async fn add_address(&self, address: IpAddr, prefix_len: u8) -> Result<()> {
        let index = self.index().await?;

        self.host
            .node
            .run_netlink(move |handle| async move {
                handle
                    .address()
                    .add(index, address, prefix_len)
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

pub struct InterfaceConfig<'a> {
    interface: &'a Interface,
    address: Vec<(IpAddr, u8)>,
    up: Option<bool>,
}

impl<'a> InterfaceConfig<'a> {
    pub fn add_address(mut self, address: IpAddr, prefix_len: u8) -> Self {
        self.address.push((address, prefix_len));
        self
    }

    pub fn up(mut self) -> Self {
        self.up = Some(true);
        self
    }

    pub fn down(mut self) -> Self {
        self.up = Some(false);
        self
    }

    pub async fn apply(self) -> Result<()> {
        for (address, prefix_len) in self.address {
            self.interface.add_address(address, prefix_len).await?;
        }

        if let Some(up) = self.up {
            if up {
                self.interface.up().await?;
            } else {
                self.interface.down().await?;
            }
        }
        Ok(())
    }
}
