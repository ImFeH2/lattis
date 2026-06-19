mod executor;
mod netns;
pub mod testing;

use std::{
    future::Future,
    sync::{Arc, atomic::AtomicU64},
};

use anyhow::{Context, Result};
use futures_util::stream::TryStreamExt;
use ipnet::IpNet;
use netns::NetworkNamespace;
use rtnetlink::{LinkBridge, LinkUnspec, LinkVeth, packet_route::link::LinkAttribute};

use crate::executor::NamespaceExecutor;
pub use crate::executor::{HostTask, RuntimeConfig};

static VETH_ID: AtomicU64 = AtomicU64::new(0);
static LAN_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct Node {
    label: String,
    executor: NamespaceExecutor,
    namespace: NetworkNamespace,
}

#[derive(Debug, Clone)]
pub struct Host {
    node: Arc<Node>,
}

pub struct DirectLink;

#[derive(Debug)]
pub struct Lan {
    index: u32,
    node: Arc<Node>,
}

#[derive(Debug)]
pub struct Interface {
    name: String,
    host: Host,
}

pub struct InterfaceConfig<'a> {
    interface: &'a Interface,
    addresses: Vec<IpNet>,
    up: Option<bool>,
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

impl Host {
    pub async fn new(name: &str) -> Result<Self> {
        Self::new_with_runtime(name, RuntimeConfig::CurrentThread).await
    }

    pub async fn new_with_runtime(name: &str, config: RuntimeConfig) -> Result<Self> {
        let namespace = NetworkNamespace::new(name).await?;
        let executor = NamespaceExecutor::new(&namespace, config).await?;

        Ok(Self {
            node: Arc::new(Node {
                label: name.to_string(),
                executor,
                namespace,
            }),
        })
    }

    pub fn name(&self) -> &str {
        &self.node.label
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

impl Lan {
    pub async fn new(name: &str) -> Result<Self> {
        let namespace = NetworkNamespace::new(name).await?;
        let executor = NamespaceExecutor::new(&namespace, RuntimeConfig::CurrentThread).await?;
        let node = Arc::new(Node {
            label: name.to_string(),
            executor,
            namespace,
        });

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

                let index = link_index(&handle, &bridge).await?;

                Ok(index)
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

        Ok(Interface {
            name: host_name,
            host,
        })
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

impl Interface {
    pub fn configure(&self) -> InterfaceConfig<'_> {
        InterfaceConfig {
            interface: self,
            addresses: Vec::new(),
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

impl<'a> InterfaceConfig<'a> {
    pub fn add_address(mut self, address: IpNet) -> Self {
        self.addresses.push(address);
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
        for address in self.addresses {
            self.interface.add_address(address).await?;
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

async fn link_index(handle: &rtnetlink::Handle, name: &str) -> Result<u32> {
    let link = handle
        .link()
        .get()
        .match_name(name.to_string())
        .execute()
        .try_next()
        .await?
        .context(format!("failed to find link: {}", name))?;

    Ok(link.header.index)
}

async fn allocate_lan_name(label: &str, handle: &rtnetlink::Handle) -> Result<String> {
    let label: String = label
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(5)
        .collect();
    let label = if label.is_empty() {
        "lan".to_string()
    } else {
        label
    };

    loop {
        let id = LAN_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("br-{}-{}", label, id);

        if !link_exists(handle, &name).await? {
            return Ok(name);
        }
    }
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
