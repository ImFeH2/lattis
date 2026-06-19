use std::{
    net::Ipv4Addr,
    sync::{Arc, Mutex},
};

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
    netlink::{allocate_lan_name, link_index},
    node::Node,
};

#[derive(Debug)]
pub struct Lan {
    index: u32,
    network: Ipv4Net,
    node: Arc<Node>,
    state: Mutex<LanState>,
}

#[derive(Debug)]
struct LanState {
    gateway: Option<Ipv4Addr>,
    next_host: usize,
}

impl Lan {
    pub async fn new(network: Ipv4Net) -> Result<Self> {
        Self::named("lan", network).await
    }

    pub async fn named(name: &str, network: Ipv4Net) -> Result<Self> {
        let node = Node::new(name, RuntimeConfig::CurrentThread).await?;

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

        Ok(Self {
            index,
            network,
            node,
            state: Mutex::new(LanState {
                gateway: None,
                next_host: 0,
            }),
        })
    }

    pub async fn attach(&self, host: &Host) -> Result<(Interface, Ipv4Net)> {
        let (interface, _port) = self.attach_node(host.node()).await?;
        let address = self.allocate_address()?;

        interface.add_address(address.into()).await?;

        if let Some(gateway) = self.gateway()? {
            interface.add_default_route(gateway).await?;
        }

        Ok((interface, address))
    }

    pub fn name(&self) -> &str {
        &self.node.label
    }

    pub fn network(&self) -> Ipv4Net {
        self.network
    }

    pub(crate) async fn attach_node(&self, node: Arc<Node>) -> Result<(Interface, Interface)> {
        let (lan_port, node_interface) = create_veth_pair(self.node.clone(), node).await?;

        self.attach_port(&lan_port).await?;

        Ok((node_interface, lan_port))
    }

    pub(crate) fn ensure_gateway_available(&self) -> Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("lan state lock poisoned"))?;

        ensure!(state.gateway.is_none(), "lan already has a gateway");

        Ok(())
    }

    pub(crate) fn allocate_address(&self) -> Result<Ipv4Net> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("lan state lock poisoned"))?;

        let address = self
            .network
            .hosts()
            .nth(state.next_host)
            .context("lan address pool is exhausted")?;
        state.next_host += 1;

        Ok(Ipv4Net::new(address, self.network.prefix_len())?)
    }

    pub(crate) fn set_gateway(&self, gateway: Ipv4Addr) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("lan state lock poisoned"))?;

        ensure!(state.gateway.is_none(), "lan already has a gateway");

        state.gateway = Some(gateway);

        Ok(())
    }

    async fn attach_port(&self, interface: &Interface) -> Result<()> {
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

    fn gateway(&self) -> Result<Option<Ipv4Addr>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("lan state lock poisoned"))?;

        Ok(state.gateway)
    }
}
