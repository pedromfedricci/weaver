//! Async executor: handles HTTP fetching with tokio.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use log::{debug, error, info, trace, warn};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::signal;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::error::AsyncExecutorError;
use crate::messages::{AsyncToCpu, CpuToAsync, LogoContext};
use crate::progress::CrawlState;

// TODO: Explore options.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:10.0) Gecko/20100101 Firefox/10.0";

/// Configuration for the async executor.
pub struct AsyncExecutorConfig {
    /// Maximum number of concurrent HTTP fetches.
    pub max_concurrent_fetches: usize,
    /// HTTP request timeout.
    pub http_timeout: Duration,
}

impl AsyncExecutorConfig {
    /// Create new async executor configuration.
    #[must_use]
    pub const fn new(max_concurrent_fetches: usize) -> Self {
        Self { max_concurrent_fetches, http_timeout: Duration::from_secs(10) }
    }
}

impl Default for AsyncExecutorConfig {
    fn default() -> Self {
        Self { max_concurrent_fetches: 100, http_timeout: Duration::from_secs(10) }
    }
}

/// Shared state for the async executor.
struct ExecutorState {
    searched: Arc<DashMap<String, HashSet<String>>>,
    semaphore: Arc<Semaphore>,
    client: Arc<reqwest::Client>,
    cpu_tx: mpsc::Sender<AsyncToCpu>,
    crawl_state: CrawlState,
}

/// Run the async executor.
///
/// # Errors
///
/// Returns an error if the HTTP client fails to build.
#[allow(clippy::too_many_lines)] // TODO: How come? Not sure why.
pub async fn run(
    cpu_tx: mpsc::Sender<AsyncToCpu>,
    mut cpu_rx: mpsc::Receiver<CpuToAsync>,
    crawl_state: CrawlState,
    config: AsyncExecutorConfig,
) -> Result<(), AsyncExecutorError> {
    let state = ExecutorState::new(cpu_tx, crawl_state, &config)?;
    let mut tasks: JoinSet<()> = JoinSet::new();

    state.requeue_pending_paths(&mut tasks);

    let mut stdin_closed = false;
    let mut eof_sent = false;

    let stdin = tokio::io::stdin();
    let mut stdin_reader = BufReader::new(stdin).lines();

    loop {
        tokio::select! {
            biased;

            // TODO: Not working? investigate later, and also send EOF to coordinator.
            _ = signal::ctrl_c(), if !stdin_closed => {
                info!("Received shutdown signal, stopping...");
                stdin_closed = true;
            }

            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                debug!("Task reaped, tasks.len()={}, result={:?}.", tasks.len(), result.is_ok());
                if let Err(e) = result {
                    error!("Fetch task panicked: {e:?}.");
                }
            }

            msg = cpu_rx.recv() => {
                if !handle_coordinator_message(msg, &state, &mut tasks).await {
                    break;
                }
            }

            line = stdin_reader.next_line(), if !stdin_closed => {
                handle_stdin_line(line, &state, &mut tasks, &mut stdin_closed);
            }
        }

        if stdin_closed && tasks.is_empty() && !eof_sent {
            debug!("Sending EOF to coordinator (tasks.len()={}).", tasks.len());
            eof_sent = true;
            if state.cpu_tx.send(AsyncToCpu::Eof).await.is_err() {
                debug!("Failed to send EOF, coordinator already closed.");
                break;
            }
        }

        if eof_sent && tasks.is_empty() {
            debug!("Waiting for coordinator to close or send more work.");
            if !handle_coordinator_message(cpu_rx.recv().await, &state, &mut tasks).await {
                break;
            }
        }
    }

    drain_tasks(&mut tasks).await;
    Ok(())
}

impl ExecutorState {
    /// Build executor state, pre-populating searched map from recovery.
    fn new(
        cpu_tx: mpsc::Sender<AsyncToCpu>,
        crawl_state: CrawlState,
        config: &AsyncExecutorConfig,
    ) -> Result<Self, AsyncExecutorError> {
        let searched: Arc<DashMap<String, HashSet<String>>> = Arc::new(DashMap::new());

        // Pre-populate searched map with recovered state.
        for (domain, progress) in crawl_state.in_progress() {
            let mut paths = progress.searched_paths().clone();
            paths.extend(progress.queued_paths().keys().cloned());
            searched.insert(domain.clone(), paths);
        }

        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_fetches));

        let client = reqwest::Client::builder()
            .timeout(config.http_timeout)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(AsyncExecutorError::HttpClientBuild)?;

        Ok(Self { searched, semaphore, client: Arc::new(client), cpu_tx, crawl_state })
    }

    /// Spawn fetch tasks for paths queued before crash.
    fn requeue_pending_paths(&self, tasks: &mut JoinSet<()>) {
        for (domain, progress) in self.crawl_state.in_progress() {
            for (path, depth) in progress.queued_paths() {
                spawn_fetch_path(
                    tasks,
                    &self.client,
                    &self.semaphore,
                    &self.cpu_tx,
                    domain.clone(),
                    path.clone(),
                    *depth,
                );
            }
        }
    }

    /// Atomically insert path if not seen; returns true if newly inserted.
    fn try_insert_path(&self, domain: &str, path: &str) -> bool {
        self.searched.entry(domain.to_string()).or_default().insert(path.to_string())
    }
}

/// Handle message from coordinator; returns false to exit loop.
async fn handle_coordinator_message(
    msg: Option<CpuToAsync>,
    state: &ExecutorState,
    tasks: &mut JoinSet<()>,
) -> bool {
    match msg {
        Some(CpuToAsync::FetchPath { domain, path, depth }) => {
            if state.try_insert_path(&domain, &path) {
                spawn_fetch_path(
                    tasks,
                    &state.client,
                    &state.semaphore,
                    &state.cpu_tx,
                    domain,
                    path,
                    depth,
                );
            } else {
                let _ = state.cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            }
            true
        }
        Some(CpuToAsync::FetchLogo { domain, url, context }) => {
            spawn_fetch_logo(
                tasks,
                &state.client,
                &state.semaphore,
                &state.cpu_tx,
                domain,
                url,
                context,
            );
            true
        }
        None => {
            debug!("Coordinator channel closed, exiting.");
            false
        }
    }
}

/// Parse stdin line and spawn fetch for new domain.
fn handle_stdin_line(
    line: Result<Option<String>, std::io::Error>,
    state: &ExecutorState,
    tasks: &mut JoinSet<()>,
    stdin_closed: &mut bool,
) {
    let line = match line {
        Ok(Some(line)) => line,
        Ok(None) => {
            debug!("stdin EOF, tasks.len()={}.", tasks.len());
            *stdin_closed = true;
            return;
        }
        Err(e) => {
            warn!("Stdin read error, closing stdin: {e}.");
            *stdin_closed = true;
            return;
        }
    };

    let domain = line.trim().to_string();
    if domain.is_empty() {
        debug!("Skip empty domain from stdin.");
        return;
    }

    if state.crawl_state.should_skip(&domain) {
        debug!("Skip {domain}, already completed.");
        return;
    }

    if !state.try_insert_path(&domain, "/") {
        debug!("Skip {domain}/, already searched.");
        return;
    }

    spawn_fetch_path(
        tasks,
        &state.client,
        &state.semaphore,
        &state.cpu_tx,
        domain.clone(),
        "/".to_string(),
        0,
    );
    debug!("Spawned fetch for {domain}, tasks.len()={}.", tasks.len());
}

/// Await all remaining tasks during shutdown.
async fn drain_tasks(tasks: &mut JoinSet<()>) {
    while let Some(result) = tasks.join_next().await {
        if let Err(e) = result {
            error!("Fetch task panicked during drain: {e:?}.");
        }
    }
}

/// Spawn a task to fetch an HTML page.
fn spawn_fetch_path(
    tasks: &mut JoinSet<()>,
    client: &Arc<reqwest::Client>,
    semaphore: &Arc<Semaphore>,
    cpu_tx: &mpsc::Sender<AsyncToCpu>,
    domain: String,
    path: String,
    depth: usize,
) {
    let client = Arc::clone(client);
    let semaphore = Arc::clone(semaphore);
    let cpu_tx = cpu_tx.clone();

    tasks.spawn(async move {
        let Ok(_permit) = semaphore.acquire().await else {
            debug!("Semaphore closed, shutting down fetch for {domain}{path}.");
            let _ = cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            return;
        };

        let url = format!("https://{domain}{path}");

        let Ok(response) = client.get(&url).send().await else {
            debug!("HTTP request failed for {url}.");
            let _ = cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            return;
        };

        if !response.status().is_success() {
            debug!("HTTP {} for {url}.", response.status());
            let _ = cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            return;
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !content_type.contains("text/html") && !content_type.is_empty() {
            trace!("Non-HTML content type '{content_type}' for {url}.");
            let _ = cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            return;
        }

        let Ok(body) = response.text().await else {
            debug!("Body decode failed for {url}.");
            let _ = cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            return;
        };

        if cpu_tx.send(AsyncToCpu::Html { domain, path, depth, body }).await.is_err() {
            debug!("Channel send failed for {url} (shutdown).");
        }
    });
}

/// Spawn a task to fetch a logo image.
fn spawn_fetch_logo(
    tasks: &mut JoinSet<()>,
    client: &Arc<reqwest::Client>,
    semaphore: &Arc<Semaphore>,
    cpu_tx: &mpsc::Sender<AsyncToCpu>,
    domain: String,
    url: String,
    context: LogoContext,
) {
    let client = Arc::clone(client);
    let semaphore = Arc::clone(semaphore);
    let cpu_tx = cpu_tx.clone();

    tasks.spawn(async move {
        let Ok(_permit) = semaphore.acquire().await else {
            debug!("Semaphore closed during logo fetch for {url}.");
            let _ = cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            return;
        };

        let Ok(response) = client.get(&url).send().await else {
            debug!("Logo fetch failed for {url}.");
            let _ = cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            return;
        };

        if !response.status().is_success() {
            debug!("Logo fetch HTTP {} for {url}.", response.status());
            let _ = cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            return;
        }

        let Ok(bytes) = response.bytes().await else {
            debug!("Logo bytes decode failed for {url}.");
            let _ = cpu_tx.send(AsyncToCpu::FetchFailed { domain }).await;
            return;
        };

        if cpu_tx
            .send(AsyncToCpu::LogoBitmap { domain, context, bytes: bytes.to_vec() })
            .await
            .is_err()
        {
            debug!("Channel send failed for logo {url} (shutdown).");
        }
    });
}
