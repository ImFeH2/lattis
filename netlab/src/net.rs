use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use ipnet::Ipv4Net;
use slotmap::{SlotMap, new_key_type};

use crate::{
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
