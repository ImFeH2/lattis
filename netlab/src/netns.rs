use std::future::Future;

use anyhow::{Context, Result};
use nix::sched::{CloneFlags, setns};
use rtnetlink::NETNS_PATH;
use std::fs::File;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crate::executor::{NamespaceExecutor, RuntimeConfig};

const THREAD_SELF_NS_PATH: &str = "/proc/thread-self/ns/net";

static NETNS_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct NetworkNamespace {
    pub(crate) name: String,
    path: PathBuf,
    file: Arc<File>,
}

#[derive(Clone, Debug)]
pub(crate) struct NetworkNamespaceHandle {
    file: Arc<File>,
}

#[derive(Debug)]
pub(crate) struct NetworkNamespaceContext {
    original: Option<File>,
}

#[derive(Debug)]
pub(crate) struct NamespaceNode {
    pub(crate) executor: NamespaceExecutor,
    pub(crate) namespace: NetworkNamespace,
}

impl NamespaceNode {
    pub(crate) async fn new(name: &str, config: RuntimeConfig) -> Result<Arc<Self>> {
        let namespace = NetworkNamespace::new(name).await?;
        let executor = NamespaceExecutor::new(&namespace, config).await?;

        Ok(Arc::new(Self {
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

impl NetworkNamespace {
    pub(crate) async fn new(name: &str) -> Result<Self> {
        let name = unique_netns_name(name);

        rtnetlink::NetworkNamespace::add(name.clone())
            .await
            .with_context(|| format!("failed to add network namespace: {}", name))?;

        let ns_path = Path::new(NETNS_PATH).join(&name);
        let ns_file = File::open(&ns_path)?;

        Ok(Self {
            name,
            path: ns_path,
            file: Arc::new(ns_file),
        })
    }

    pub(crate) fn raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    pub(crate) fn handle(&self) -> NetworkNamespaceHandle {
        NetworkNamespaceHandle {
            file: Arc::clone(&self.file),
        }
    }
}

impl Drop for NetworkNamespace {
    fn drop(&mut self) {
        if let Err(err) = nix::mount::umount2(&self.path, nix::mount::MntFlags::MNT_DETACH) {
            eprintln!("failed to unmount network namespace {}: {}", self.name, err);
        }
        if let Err(err) = nix::unistd::unlink(&self.path) {
            eprintln!("failed to remove network namespace {}: {}", self.name, err);
        }
    }
}

impl NetworkNamespaceHandle {
    pub(crate) fn enter(&self) -> Result<NetworkNamespaceContext> {
        NetworkNamespaceContext::enter(&self.file)
    }
}

impl NetworkNamespaceContext {
    fn enter(namespace_file: &File) -> Result<Self> {
        let original = File::open(THREAD_SELF_NS_PATH)
            .context("failed to open current thread network namespace")?;

        setns(namespace_file, CloneFlags::CLONE_NEWNET)
            .context("failed to enter network namespace")?;

        Ok(Self {
            original: Some(original),
        })
    }

    pub(crate) fn restore(mut self) -> Result<()> {
        if let Some(original) = self.original.as_ref() {
            setns(original, CloneFlags::CLONE_NEWNET)
                .context("failed to restore original network namespace")?;
        }

        self.original.take();
        Ok(())
    }
}

impl Drop for NetworkNamespaceContext {
    fn drop(&mut self) {
        if let Some(original) = self.original.as_ref()
            && let Err(err) = setns(original, CloneFlags::CLONE_NEWNET)
        {
            eprintln!("failed to restore original network namespace: {}", err);
        }
    }
}

fn unique_netns_name(label: &str) -> String {
    let id = NETNS_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id();

    format!("netlab-{}-{}-{}", label, pid, id)
}
