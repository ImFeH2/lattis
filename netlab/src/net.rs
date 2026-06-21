use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::Ipv4Addr,
    sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow};
use ipnet::Ipv4Net;
use rtnetlink::RouteMessageBuilder;
use slotmap::{SlotMap, new_key_type};

use crate::{
    network::netns::NamespaceNode,
    runtime::executor::RuntimeConfig,
    topology::{
        host::{Host, HostEntry},
        lan::{Lan, LanEntry},
        router::{Router, RouterEntry},
    },
};

new_key_type! {
    pub(crate) struct HostKey;
    pub(crate) struct LanKey;
    pub(crate) struct RouterKey;
}

#[derive(Clone, Debug)]
pub struct Net {
    inner: Arc<NetInner>,
}

#[derive(Debug)]
pub(crate) struct NetInner {
    state: Mutex<NetState>,
}

#[derive(Debug)]
pub(crate) struct NetState {
    pub(crate) hosts: SlotMap<HostKey, HostEntry>,
    pub(crate) lans: SlotMap<LanKey, LanEntry>,
    pub(crate) routers: SlotMap<RouterKey, RouterEntry>,
}

#[derive(Clone)]
struct LanSnapshot {
    network: Ipv4Net,
    routers: HashMap<RouterKey, Ipv4Net>,
}

struct RouteUpdate {
    destination: Ipv4Net,
    gateway: Option<Ipv4Addr>,
    node: Arc<NamespaceNode>,
}

#[derive(Clone)]
struct RouterSnapshot {
    lans: HashSet<LanKey>,
    masquerade_lans: HashSet<LanKey>,
    node: Arc<NamespaceNode>,
}

struct TopologySnapshot {
    lans: HashMap<LanKey, LanSnapshot>,
    routers: HashMap<RouterKey, RouterSnapshot>,
}

impl Net {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(NetInner {
                state: Mutex::new(NetState::new()),
            }),
        }
    }

    pub async fn host(&self) -> Result<Host> {
        Host::create(self.clone(), "host", RuntimeConfig::CurrentThread).await
    }

    pub async fn lan(&self, network: Ipv4Net) -> Result<Lan> {
        Lan::create(self.clone(), network, "lan", RuntimeConfig::CurrentThread).await
    }

    pub async fn router(&self) -> Result<Router> {
        Router::create(self.clone(), "router", RuntimeConfig::CurrentThread).await
    }

    pub async fn converge(&self) -> Result<()> {
        let snapshot = self.snapshot()?;
        let mut updates = Vec::new();

        for source in snapshot.routers.keys().copied() {
            updates.extend(snapshot.route_updates_from(source)?);
        }

        for update in updates {
            apply_route(update).await?;
        }

        Ok(())
    }

    pub(crate) fn ensure_same(&self, other: &Self) -> Result<()> {
        if Arc::ptr_eq(&self.inner, &other.inner) {
            Ok(())
        } else {
            Err(anyhow!("netlab devices belong to different nets"))
        }
    }

    pub(crate) fn with_state<T>(&self, f: impl FnOnce(&NetState) -> Result<T>) -> Result<T> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| anyhow!("net state lock poisoned"))?;

        f(&state)
    }

    pub(crate) fn with_state_mut<T>(
        &self,
        f: impl FnOnce(&mut NetState) -> Result<T>,
    ) -> Result<T> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| anyhow!("net state lock poisoned"))?;

        f(&mut state)
    }

    fn snapshot(&self) -> Result<TopologySnapshot> {
        self.with_state(|state| {
            let lans = state
                .lans
                .iter()
                .map(|(key, lan)| {
                    (
                        key,
                        LanSnapshot {
                            network: lan.network,
                            routers: lan.routers.clone(),
                        },
                    )
                })
                .collect();
            let routers = state
                .routers
                .iter()
                .map(|(key, router)| {
                    (
                        key,
                        RouterSnapshot {
                            lans: router.lans.clone(),
                            masquerade_lans: router.masquerade_lans.clone(),
                            node: Arc::clone(&router.node),
                        },
                    )
                })
                .collect();

            Ok(TopologySnapshot { lans, routers })
        })
    }
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

impl NetState {
    fn new() -> Self {
        Self {
            hosts: SlotMap::with_key(),
            lans: SlotMap::with_key(),
            routers: SlotMap::with_key(),
        }
    }
}

impl TopologySnapshot {
    fn route_updates_from(&self, source: RouterKey) -> Result<Vec<RouteUpdate>> {
        let source_router = &self.routers[&source];
        let mut gateways = HashMap::<RouterKey, Ipv4Addr>::new();
        let mut queue = VecDeque::from([source]);
        let mut reached_lans = HashMap::<LanKey, Ipv4Addr>::new();
        let mut visited_routers = HashSet::from([source]);

        while let Some(current) = queue.pop_front() {
            let router = &self.routers[&current];

            for lan in &router.lans {
                if current != source && router.masquerade_lans.contains(lan) {
                    continue;
                }

                if current != source
                    && !source_router.lans.contains(lan)
                    && let Some(gateway) = gateways.get(&current).copied()
                {
                    reached_lans.entry(*lan).or_insert(gateway);
                }

                let Some(lan_snapshot) = self.lans.get(lan) else {
                    continue;
                };

                for next in lan_snapshot.routers.keys().copied() {
                    if !visited_routers.insert(next) {
                        continue;
                    }

                    let gateway = if current == source {
                        lan_snapshot.routers[&next].addr()
                    } else {
                        gateways[&current]
                    };

                    gateways.insert(next, gateway);
                    queue.push_back(next);
                }
            }
        }

        let deletes = self
            .lans
            .keys()
            .filter(|lan| !source_router.lans.contains(lan))
            .map(|lan| RouteUpdate {
                destination: self.lans[lan].network,
                gateway: None,
                node: Arc::clone(&source_router.node),
            });
        let adds = reached_lans.into_iter().map(|(lan, gateway)| RouteUpdate {
            destination: self.lans[&lan].network,
            gateway: Some(gateway),
            node: Arc::clone(&source_router.node),
        });

        Ok(deletes.chain(adds).collect())
    }
}

async fn apply_route(update: RouteUpdate) -> Result<()> {
    update
        .node
        .run_netlink(move |handle| async move {
            let builder = RouteMessageBuilder::<Ipv4Addr>::new().destination_prefix(
                update.destination.network(),
                update.destination.prefix_len(),
            );

            let route = match update.gateway {
                Some(gateway) => builder.gateway(gateway).build(),
                None => builder.build(),
            };

            if update.gateway.is_some() {
                handle.route().add(route).replace().execute().await?;
            } else if let Err(err) = handle.route().del(route).execute().await
                && !is_missing_route_error(&err)
            {
                return Err(err.into());
            }

            Ok(())
        })
        .await
}

fn is_missing_route_error(err: &rtnetlink::Error) -> bool {
    matches!(
        err,
        rtnetlink::Error::NetlinkError(message)
            if message.code.is_some_and(|code| matches!(code.get(), -2 | -3))
    )
}
