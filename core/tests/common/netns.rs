use anyhow::{Context, Result};
use nix::sched::{CloneFlags, setns};
use rtnetlink::{NETNS_PATH, NetworkNamespace, SELF_NS_PATH};
use std::fs::File;
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;

pub(crate) struct NetworkNamespaceGuard {
    name: String,
    file: File,
}

impl NetworkNamespaceGuard {
    pub(crate) async fn new(name: &str) -> Result<Self> {
        let name = name.to_string();

        NetworkNamespace::add(name.clone())
            .await
            .with_context(|| format!("Failed to add network namespace: {}", name))?;

        let ns_path = Path::new(NETNS_PATH).join(&name);
        let ns_file = File::open(&ns_path)?;

        Ok(Self {
            name,
            file: ns_file,
        })
    }

    pub(crate) fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    fn enter(&self) -> Result<NetworkNamespaceContext> {
        let last = File::open(SELF_NS_PATH)?;
        setns(&self.file, CloneFlags::CLONE_NEWNET)?;
        Ok(NetworkNamespaceContext { last })
    }

    pub(crate) fn run<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let _context = self.enter()?;
        f()
    }

    #[allow(dead_code)]
    pub(crate) async fn async_run<T, Fut>(&self, f: impl FnOnce() -> Fut) -> Result<T>
    where
        Fut: Future<Output = Result<T>>,
    {
        let _context = self.enter()?;
        f().await
    }

    pub(crate) async fn with_handle<T, Fut>(
        &self,
        f: impl FnOnce(rtnetlink::Handle) -> Fut,
    ) -> Result<T>
    where
        Fut: Future<Output = Result<T>>,
    {
        let _context = self.enter()?;
        let (connection, handle, _) = rtnetlink::new_connection()?;
        tokio::spawn(connection);

        f(handle).await
    }
}

impl Drop for NetworkNamespaceGuard {
    fn drop(&mut self) {
        let ns_path = Path::new(NETNS_PATH).join(&self.name);
        let _ = nix::mount::umount2(&ns_path, nix::mount::MntFlags::MNT_DETACH);
        let _ = nix::unistd::unlink(&ns_path);
    }
}

struct NetworkNamespaceContext {
    last: File,
}

impl Drop for NetworkNamespaceContext {
    fn drop(&mut self) {
        let _ = setns(&self.last, CloneFlags::CLONE_NEWNET);
    }
}
