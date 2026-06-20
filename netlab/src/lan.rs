use std::{collections::HashMap, net::Ipv4Addr, sync::Arc};

use anyhow::{Context, Result, ensure};
use ipnet::Ipv4Net;
use rtnetlink::{
    LinkBridge, LinkBridgePort, LinkUnspec,
    packet_route::link::{BridgePortState, BridgeStpState},
};

use crate::{
    executor::RuntimeConfig,
    host::Host,
    interface::Interface,
    link::create_veth_pair,
    net::{HostKey, LanKey, Net, RouterKey},
    netlink::{allocate_lan_name, link_index},
    netns::NamespaceNode,
    router::Router,
};

#[derive(Clone, Debug)]
pub struct Lan {
    net: Net,
    key: LanKey,
    name: String,
}

#[derive(Debug)]
pub(crate) struct LanEntry {
    pub(crate) gateway: Option<Ipv4Addr>,
    pub(crate) hosts: HashMap<HostKey, (Ipv4Net, Interface)>,
    pub(crate) index: u32,
    pub(crate) network: Ipv4Net,
    pub(crate) node: Arc<NamespaceNode>,
    pub(crate) next_host: usize,
    pub(crate) routers: HashMap<RouterKey, Ipv4Net>,
}

impl Lan {
    pub(crate) async fn join_host(&self, host: &Host) -> Result<Ipv4Net> {
        self.net.ensure_same(host.net())?;

        if let Some(address) = self.host_address(host)? {
            return Ok(address);
        }

        let interface = self.attach_node(host.node()).await?;
        let address = self.allocate_address()?;

        interface.add_address(address.into()).await?;

        if let Some(gateway) = self.remember_host(host.key(), address, &interface)? {
            interface.set_default_route(gateway).await?;
        }

        Ok(address)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) async fn attach_node(&self, node: Arc<NamespaceNode>) -> Result<Interface> {
        let (lan_port, node_interface) = create_veth_pair(self.node(), node).await?;

        self.attach_port(&lan_port).await?;

        Ok(node_interface)
    }

    pub(crate) fn allocate_address(&self) -> Result<Ipv4Net> {
        self.net.with_state_mut(|state| {
            let lan = &mut state.lans[self.key];
            let network = lan.network;
            let address = network
                .hosts()
                .nth(lan.next_host)
                .context("lan address pool is exhausted")?;
            lan.next_host += 1;

            Ok(Ipv4Net::new(address, network.prefix_len())?)
        })
    }

    pub async fn set_gateway(&self, router: &Router) -> Result<()> {
        self.net.ensure_same(router.net())?;

        if self.router_address(router)?.is_none() {
            router.attach(self).await?;
        }

        let (gateway, host_interfaces) = self.net.with_state_mut(|state| {
            let lan = &mut state.lans[self.key];
            let gateway = lan.routers[&router.key()].addr();
            let host_interfaces = lan
                .hosts
                .values()
                .map(|(_, interface)| interface.clone())
                .collect::<Vec<_>>();

            if let Some(existing) = lan.gateway {
                ensure!(existing == gateway, "lan already has a different gateway");

                return Ok((gateway, host_interfaces));
            }

            lan.gateway = Some(gateway);
            Ok((gateway, host_interfaces))
        })?;

        for interface in host_interfaces {
            interface.set_default_route(gateway).await?;
        }

        Ok(())
    }

    async fn attach_port(&self, interface: &Interface) -> Result<()> {
        let bridge_index = self.index();
        let port_index = interface.index().await?;

        self.node()
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

                handle
                    .link()
                    .set(
                        LinkBridgePort::new(port_index)
                            .state(BridgePortState::Forwarding)
                            .learning(true)
                            .flood(true)
                            .bcast_flood(true)
                            .build(),
                    )
                    .execute()
                    .await?;

                Ok(())
            })
            .await
    }

    fn host_address(&self, host: &Host) -> Result<Option<Ipv4Net>> {
        self.net.with_state(|state| {
            Ok(state.lans[self.key]
                .hosts
                .get(&host.key())
                .map(|(address, _)| *address))
        })
    }

    fn remember_host(
        &self,
        host: HostKey,
        address: Ipv4Net,
        interface: &Interface,
    ) -> Result<Option<Ipv4Addr>> {
        self.net.with_state_mut(|state| {
            let lan = &mut state.lans[self.key];
            lan.hosts.insert(host, (address, interface.clone()));

            Ok(lan.gateway)
        })
    }

    pub(crate) fn net(&self) -> &Net {
        &self.net
    }

    pub(crate) fn key(&self) -> LanKey {
        self.key
    }

    fn index(&self) -> u32 {
        self.net
            .with_state(|state| Ok(state.lans[self.key].index))
            .expect("lan is no longer registered in net")
    }

    fn node(&self) -> Arc<NamespaceNode> {
        self.net
            .with_state(|state| Ok(state.lans[self.key].node.clone()))
            .expect("lan is no longer registered in net")
    }

    pub(crate) fn remember_router(&self, router: RouterKey, address: Ipv4Net) -> Result<()> {
        self.net.with_state_mut(|state| {
            state.lans[self.key].routers.insert(router, address);

            Ok(())
        })
    }

    pub(crate) fn router_address(&self, router: &Router) -> Result<Option<Ipv4Net>> {
        self.net
            .with_state(|state| Ok(state.lans[self.key].routers.get(&router.key()).copied()))
    }

    pub(crate) async fn create(
        net: Net,
        network: Ipv4Net,
        name: &str,
        runtime: RuntimeConfig,
    ) -> Result<Self> {
        let node = NamespaceNode::new(name, runtime).await?;

        let label = name.to_string();
        let index = node
            .run_netlink(move |handle| async move {
                let bridge = allocate_lan_name(&label, &handle).await?;
                handle
                    .link()
                    .add(
                        LinkBridge::new(&bridge)
                            .stp_state(BridgeStpState::Disabled)
                            .forward_delay(0)
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

        let key = net.with_state_mut(|state| {
            Ok(state.lans.insert(LanEntry {
                gateway: None,
                hosts: HashMap::new(),
                index,
                network,
                node,
                next_host: 0,
                routers: HashMap::new(),
            }))
        })?;

        Ok(Lan {
            net,
            key,
            name: name.to_string(),
        })
    }
}
