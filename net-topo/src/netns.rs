use anyhow::{Context, Result};
use rtnetlink::NETNS_PATH;
use std::fs::File;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct NetworkNamespace {
    pub(crate) name: String,
    path: PathBuf,
    file: File,
}

impl NetworkNamespace {
    pub(crate) async fn new(name: &str) -> Result<Self> {
        let name = name.to_string();

        rtnetlink::NetworkNamespace::add(name.clone())
            .await
            .with_context(|| format!("failed to add network namespace: {}", name))?;

        let ns_path = Path::new(NETNS_PATH).join(&name);
        let ns_file = File::open(&ns_path)?;

        Ok(Self {
            name,
            path: ns_path,
            file: ns_file,
        })
    }

    pub(crate) fn raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    pub(crate) fn try_clone_file(&self) -> Result<File> {
        self.file
            .try_clone()
            .context("failed to clone network namespace file")
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
