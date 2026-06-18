use anyhow::{Context, Result, anyhow, bail};
use std::fmt;
use std::future::Future;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::thread;

use crate::netns::{NetworkNamespace, NetworkNamespaceHandle};

type BlockingNamespaceJob = Box<dyn FnOnce() + Send + 'static>;
type AsyncNamespaceJob = Box<dyn FnOnce(tokio::runtime::Handle) + Send + 'static>;

const MAX_BLOCKING_WORKERS: usize = 512;

#[derive(Debug)]
pub(crate) struct NamespaceExecutor {
    async_executor: AsyncNamespaceExecutor,
    blocking_executor: BlockingNamespaceExecutor,
}

pub struct HostTask<T> {
    result: tokio::sync::oneshot::Receiver<Result<T>>,
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

struct BlockingNamespaceExecutor {
    namespace: NetworkNamespaceHandle,
    workers: Mutex<Vec<BlockingNamespaceWorker>>,
}

struct BlockingNamespaceWorker {
    jobs: Option<mpsc::Sender<BlockingNamespaceJob>>,
    pending: Arc<AtomicUsize>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}

impl NamespaceExecutor {
    pub(crate) async fn new(namespace: &NetworkNamespace, config: RuntimeConfig) -> Result<Self> {
        let namespace = namespace.handle();
        let async_executor =
            AsyncNamespaceExecutor::new_in_namespace(namespace.clone(), config).await?;
        let blocking_executor = BlockingNamespaceExecutor::new(namespace)?;

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

    pub(crate) fn spawn_blocking<T, F>(&self, f: F) -> Result<HostTask<T>>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        self.blocking_executor.spawn(f)
    }
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

impl AsyncNamespaceExecutor {
    async fn new_in_namespace(
        namespace: NetworkNamespaceHandle,
        config: RuntimeConfig,
    ) -> Result<Self> {
        tokio::task::spawn_blocking(move || {
            let context = namespace.enter()?;
            let runtime = build_runtime(config)?;
            let executor = Self::from_runtime(runtime);
            context.restore()?;
            executor
        })
        .await
        .context("async namespace executor bootstrap panicked")?
    }

    fn from_runtime(runtime: tokio::runtime::Runtime) -> Result<Self> {
        let (jobs, mut rx) = tokio::sync::mpsc::unbounded_channel::<AsyncNamespaceJob>();

        let thread = thread::spawn(move || -> Result<()> {
            let handle = runtime.handle().clone();

            runtime.block_on(async move {
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
                if tx.send(result).is_err() {
                    eprintln!("async namespace task result receiver dropped");
                }
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

impl BlockingNamespaceExecutor {
    fn new(namespace: NetworkNamespaceHandle) -> Result<Self> {
        Ok(Self {
            namespace,
            workers: Mutex::new(Vec::new()),
        })
    }

    fn spawn<T, F>(&self, f: F) -> Result<HostTask<T>>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<T>>();

        let job: BlockingNamespaceJob = Box::new(move || {
            let result = f();
            if tx.send(result).is_err() {
                eprintln!("blocking namespace task result receiver dropped");
            }
        });

        let mut workers = self
            .workers
            .lock()
            .map_err(|_| anyhow!("blocking namespace executor lock poisoned"))?;

        let worker_index = if let Some(index) = workers.iter().position(|worker| worker.is_idle()) {
            index
        } else if workers.len() < MAX_BLOCKING_WORKERS {
            workers.push(BlockingNamespaceWorker::new(self.namespace.clone())?);
            workers.len() - 1
        } else {
            workers
                .iter()
                .enumerate()
                .min_by_key(|(_, worker)| worker.pending_count())
                .map(|(index, _)| index)
                .context("blocking namespace executor has no workers")?
        };

        workers[worker_index].spawn(job)?;

        Ok(HostTask { result: rx })
    }
}

impl fmt::Debug for BlockingNamespaceExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let worker_count = self
            .workers
            .lock()
            .map(|workers| workers.len())
            .unwrap_or_default();

        f.debug_struct("BlockingNamespaceExecutor")
            .field("worker_count", &worker_count)
            .finish()
    }
}

impl BlockingNamespaceWorker {
    fn new(namespace: NetworkNamespaceHandle) -> Result<Self> {
        let (jobs, rx) = mpsc::channel::<BlockingNamespaceJob>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_pending = Arc::clone(&pending);

        let thread = thread::spawn(move || -> Result<()> {
            let context = match namespace.enter() {
                Ok(context) => context,
                Err(err) => {
                    if ready_tx.send(Err(err.to_string())).is_err() {
                        eprintln!("failed to report blocking namespace worker setup failure");
                    }
                    return Err(err);
                }
            };

            if ready_tx.send(Ok(())).is_err() {
                eprintln!("failed to report blocking namespace worker setup success");
            }

            while let Ok(job) = rx.recv() {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                worker_pending.fetch_sub(1, Ordering::AcqRel);

                if result.is_err() {
                    eprintln!("blocking namespace job panicked");
                }
            }

            drop(context);
            Ok(())
        });

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                log_thread_join("blocking namespace worker", thread.join());
                bail!("{}", message);
            }
            Err(_) => {
                log_thread_join("blocking namespace worker", thread.join());
                bail!("blocking namespace worker stopped before setup completed");
            }
        }

        Ok(Self {
            jobs: Some(jobs),
            pending,
            thread: Some(thread),
        })
    }

    fn spawn(&self, job: BlockingNamespaceJob) -> Result<()> {
        self.pending.fetch_add(1, Ordering::AcqRel);
        self.jobs
            .as_ref()
            .context("blocking namespace executor has stopped")?
            .send(job)
            .map_err(|_| {
                self.pending.fetch_sub(1, Ordering::AcqRel);
                anyhow!("blocking namespace executor has stopped")
            })?;

        Ok(())
    }

    fn is_idle(&self) -> bool {
        self.pending_count() == 0
    }

    fn pending_count(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }
}

impl Drop for BlockingNamespaceWorker {
    fn drop(&mut self) {
        self.jobs.take();

        if let Some(thread) = self.thread.take() {
            log_thread_join("blocking namespace worker", thread.join());
        }
    }
}

fn build_runtime(config: RuntimeConfig) -> Result<tokio::runtime::Runtime> {
    match config {
        RuntimeConfig::CurrentThread => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to build current-thread namespace executor"),
        RuntimeConfig::MultiThread { worker_threads } => {
            if worker_threads == 0 {
                bail!("worker_threads must be greater than 0");
            }

            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(worker_threads)
                .enable_all()
                .build()
                .context("failed to build multi-thread namespace executor")
        }
    }
}

fn log_thread_join(name: &str, result: thread::Result<Result<()>>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => eprintln!("{} failed: {}", name, err),
        Err(_) => eprintln!("{} panicked", name),
    }
}
