use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::{Duration, timeout},
};

use crate::{
    executor::{HostTask, RuntimeConfig},
    node::Node,
};

const REACHABILITY_TIMEOUT: Duration = Duration::from_secs(1);

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

    pub async fn assert_can_reach(&self, peer: &Host, peer_addr: impl Into<IpAddr>) -> Result<()> {
        let peer_addr = peer_addr.into();
        let (port_tx, port_rx) = oneshot::channel();

        let server = peer.spawn(move || async move {
            let listener = TcpListener::bind(SocketAddr::new(peer_addr, 0)).await?;
            let port = listener.local_addr()?.port();
            let _ = port_tx.send(port);

            timeout(REACHABILITY_TIMEOUT, listener.accept()).await??;

            Ok(())
        })?;

        let port = match port_rx.await {
            Ok(port) => port,
            Err(_) => {
                server.await.context("peer listener failed to start")?;
                return Err(anyhow!("peer listener stopped before reporting port"));
            }
        };

        let peer_socket = SocketAddr::new(peer_addr, port);
        let client = self.spawn(move || async move {
            timeout(REACHABILITY_TIMEOUT, TcpStream::connect(peer_socket)).await??;

            Ok(())
        })?;

        let (server_result, client_result) = tokio::join!(server, client);
        client_result?;
        server_result?;

        Ok(())
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
