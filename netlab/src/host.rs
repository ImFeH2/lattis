use std::{future::Future, sync::Arc};

use anyhow::Result;

use crate::{
    executor::{HostTask, RuntimeConfig},
    node::Node,
};

#[derive(Debug, Clone)]
pub struct Host {
    pub(crate) node: Arc<Node>,
}

impl Host {
    pub async fn new() -> Result<Self> {
        Self::with_runtime(RuntimeConfig::CurrentThread).await
    }

    pub async fn named(name: &str) -> Result<Self> {
        Self::named_with_runtime(name, RuntimeConfig::CurrentThread).await
    }

    pub async fn with_runtime(config: RuntimeConfig) -> Result<Self> {
        Self::named_with_runtime("host", config).await
    }

    pub async fn named_with_runtime(name: &str, config: RuntimeConfig) -> Result<Self> {
        Ok(Self {
            node: Node::new(name, config).await?,
        })
    }

    pub fn name(&self) -> &str {
        &self.node.label
    }

    pub(crate) fn node(&self) -> Arc<Node> {
        self.node.clone()
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
