use anyhow::{Context, Result, anyhow, bail};
use nix::sched::{CloneFlags, setns};
use std::fs::File;
use std::future::Future;
use std::sync::mpsc;
use std::thread;

use crate::netns::NetworkNamespace;

type BlockingNamespaceJob = Box<dyn FnOnce() + Send + 'static>;
type AsyncNamespaceJob = Box<dyn FnOnce(tokio::runtime::Handle) + Send + 'static>;

#[derive(Debug)]
pub(crate) struct NamespaceExecutor {
    async_executor: AsyncNamespaceExecutor,
    blocking_executor: BlockingNamespaceExecutor,
}

impl NamespaceExecutor {
    pub(crate) async fn new(namespace: &NetworkNamespace, config: RuntimeConfig) -> Result<Self> {
        let ns_file = namespace.try_clone_file()?;
        let blocking_executor = BlockingNamespaceExecutor::new(ns_file)?;
        let async_executor = blocking_executor
            .run(move || AsyncNamespaceExecutor::new(config))
            .await?;

        Ok(Self {
            async_executor,
            blocking_executor,
        })
    }

    pub(crate) fn spawn<T, F, Fut>(&self, f: F) -> Result<HostTask<T>>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.async_executor.spawn(f)
    }

    pub(crate) async fn run<T, F, Fut>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.async_executor.run(f).await
    }

    pub(crate) async fn run_blocking<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        self.blocking_executor.run(f).await
    }
}

#[derive(Debug)]
struct BlockingNamespaceExecutor {
    jobs: Option<mpsc::Sender<BlockingNamespaceJob>>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}

impl BlockingNamespaceExecutor {
    fn new(namespace_file: File) -> Result<Self> {
        let (jobs, rx) = mpsc::channel::<BlockingNamespaceJob>();

        let thread = thread::spawn(move || -> Result<()> {
            setns(&namespace_file, CloneFlags::CLONE_NEWNET)
                .context("failed to enter network namespace")?;

            while let Ok(job) = rx.recv() {
                job();
            }

            Ok(())
        });

        Ok(Self {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    async fn run<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<T>>();

        let job: BlockingNamespaceJob = Box::new(move || {
            let result = f();
            let _ = tx.send(result);
        });

        self.jobs
            .as_ref()
            .context("blocking namespace executor has stopped")?
            .send(job)
            .map_err(|_| anyhow!("blocking namespace executor has stopped"))?;

        rx.await
            .context("blocking namespace executor stopped before returning result")?
    }
}

impl Drop for BlockingNamespaceExecutor {
    fn drop(&mut self) {
        self.jobs.take();

        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => eprintln!("blocking namespace executor failed: {}", err),
                Err(_) => eprintln!("blocking namespace executor panicked"),
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RuntimeConfig {
    CurrentThread,
    MultiThread { worker_threads: usize },
}

#[derive(Debug)]
struct AsyncNamespaceExecutor {
    jobs: Option<tokio::sync::mpsc::UnboundedSender<AsyncNamespaceJob>>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}

impl AsyncNamespaceExecutor {
    fn new(config: RuntimeConfig) -> Result<Self> {
        let (jobs, mut rx) = tokio::sync::mpsc::unbounded_channel::<AsyncNamespaceJob>();

        let thread = thread::spawn(move || -> Result<()> {
            let rt = match config {
                RuntimeConfig::CurrentThread => tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .context("failed to build current-thread namespace executor")?,
                RuntimeConfig::MultiThread { worker_threads } => {
                    if worker_threads == 0 {
                        bail!("worker_threads must be greater than 0");
                    }

                    tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(worker_threads)
                        .enable_all()
                        .build()
                        .context("failed to build multi-thread namespace executor")?
                }
            };

            let handle = rt.handle().clone();

            rt.block_on(async move {
                while let Some(job) = rx.recv().await {
                    job(handle.clone());
                }
            });

            Ok(())
        });

        Ok(Self {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    fn spawn<T, F, Fut>(&self, f: F) -> Result<HostTask<T>>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<T>>();

        let job: AsyncNamespaceJob = Box::new(move |handle| {
            handle.spawn(async move {
                let result = f().await;
                let _ = tx.send(result);
            });
        });

        self.jobs
            .as_ref()
            .context("async namespace executor has stopped")?
            .send(job)
            .map_err(|_| anyhow!("async namespace executor has stopped"))?;

        Ok(HostTask { result: rx })
    }

    async fn run<T, F, Fut>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.spawn(f)?.join().await
    }
}

impl Drop for AsyncNamespaceExecutor {
    fn drop(&mut self) {
        self.jobs.take();

        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => eprintln!("async namespace executor failed: {}", err),
                Err(_) => eprintln!("async namespace executor panicked"),
            }
        }
    }
}

pub struct HostTask<T> {
    result: tokio::sync::oneshot::Receiver<Result<T>>,
}

impl<T> HostTask<T> {
    pub async fn join(self) -> Result<T> {
        self.result
            .await
            .context("host task stopped before returning result")?
    }
}

impl<T> Future for HostTask<T> {
    type Output = Result<T>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.result).poll(cx).map(|result| {
            result
                .context("host task stopped before returning result")
                .and_then(|res| res)
        })
    }
}
