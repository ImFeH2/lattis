use std::{future::Future, sync::Arc};

use anyhow::Result;

use crate::{
    executor::{NamespaceExecutor, RuntimeConfig},
    netns::NetworkNamespace,
};

#[derive(Debug)]
pub struct Node {
    pub(crate) label: String,
    pub(crate) executor: NamespaceExecutor,
    pub(crate) namespace: NetworkNamespace,
}

impl Node {
    pub(crate) async fn new(name: &str, config: RuntimeConfig) -> Result<Arc<Self>> {
        let namespace = NetworkNamespace::new(name).await?;
        let executor = NamespaceExecutor::new(&namespace, config).await?;

        Ok(Arc::new(Self {
            label: name.to_string(),
            executor,
            namespace,
        }))
    }

    pub(crate) async fn run_netlink<T, F, Fut>(&self, f: F) -> Result<T>
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

    pub(crate) async fn run_blocking<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        self.executor.spawn_blocking(f)?.await
    }
}
