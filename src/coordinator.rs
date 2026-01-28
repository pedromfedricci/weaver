//! Coordinator: bridges async/sync work, manages domain state, writes progress log.

use std::collections::HashMap;
use std::ops::ControlFlow;

use crossbeam::channel::{Receiver, Sender};
use log::{debug, error, info};
use tokio::sync::mpsc;

use crate::messages::{
    AsyncToCpu, CpuToAsync, LogoContext, LogoFound, WorkItem, WorkPayload, WorkerResult,
    WorkerResultPayload,
};
use crate::progress::{CompletionReason, LogoInfo, ProgressWriter};

/// State for a single domain being crawled.
#[derive(Debug, Default)]
struct DomainState {
    /// Number of in-flight requests (HTML pages + logo fetches).
    pending_fetches: usize,
    /// Number of pages crawled (breadth counter).
    pages_crawled: usize,
    /// Whether a logo has been found for this domain.
    logo_found: bool,
    /// Whether completion has been logged.
    logged_complete: bool,
}

impl DomainState {
    /// Check if domain has reached its page crawl limit.
    const fn at_page_limit(&self, config: &CoordinatorConfig) -> bool {
        self.pages_crawled >= config.max_pages_per_domain
    }
}

/// Configuration for the coordinator.
pub struct CoordinatorConfig {
    /// Max number of pages to be fetched per domain.
    pub max_pages_per_domain: usize,
    /// Max depth of page crawling for a domain, from root.
    pub max_depth: usize,
}

impl CoordinatorConfig {
    /// Create new coordinator configuration.
    #[must_use]
    pub const fn new(max_pages_per_domain: usize, max_depth: usize) -> Self {
        Self { max_pages_per_domain, max_depth }
    }
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self { max_pages_per_domain: 50, max_depth: 3 }
    }
}

/// The coordinator manages domain state and bridges async/sync channels.
pub struct Coordinator {
    async_rx: mpsc::Receiver<AsyncToCpu>,
    async_tx: Option<mpsc::Sender<CpuToAsync>>,
    work_tx: Sender<WorkItem>,
    result_rx: Receiver<WorkerResult>,
    domains: HashMap<String, DomainState>,
    progress: ProgressWriter,
    config: CoordinatorConfig,
    eof_received: bool,
}

impl Coordinator {
    /// Create coordinator.
    #[must_use]
    pub fn new(
        async_rx: mpsc::Receiver<AsyncToCpu>,
        async_tx: mpsc::Sender<CpuToAsync>,
        work_tx: Sender<WorkItem>,
        result_rx: Receiver<WorkerResult>,
        progress: ProgressWriter,
        config: CoordinatorConfig,
    ) -> Self {
        Self {
            async_rx,
            async_tx: Some(async_tx),
            work_tx,
            result_rx,
            domains: HashMap::new(),
            progress,
            config,
            eof_received: false,
        }
    }

    /// Run the coordinator loop.
    pub fn run(mut self) {
        use std::time::Duration;

        loop {
            // First, drain all available async messages to ensure fair processing.
            loop {
                match self.async_rx.try_recv() {
                    Ok(msg) => {
                        if self.handle_async_message(msg) == ControlFlow::Break(()) {
                            debug!("Shutdown after async message, draining worker results.");
                            self.drain_worker_results();
                            return;
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        debug!("Async executor disconnected, draining worker results.");
                        self.drain_worker_results();
                        return;
                    }
                }
            }

            // Then wait for worker results with a timeout so we can check async again.
            match self.result_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(worker_result) => {
                    self.handle_worker_result(worker_result);
                    if self.check_shutdown() == ControlFlow::Break(()) {
                        debug!("Shutdown after worker result, draining remaining results.");
                        self.drain_worker_results();
                        return;
                    }
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                    // No worker results, loop back to check async messages.
                }
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                    info!("All workers have exited, coordinator shutting down.");
                    return;
                }
            }
        }
    }

    /// Drain remaining worker results during shutdown.
    fn drain_worker_results(&mut self) {
        while let Ok(result) = self.result_rx.try_recv() {
            self.handle_worker_result(result);
        }
    }

    /// Handle a message from the async executor.
    fn handle_async_message(&mut self, msg: AsyncToCpu) -> ControlFlow<()> {
        match msg {
            AsyncToCpu::Html { ref domain, path, depth, body } => {
                self.handle_html(domain, path, depth, body)
            }
            AsyncToCpu::LogoBitmap { ref domain, context, bytes } => {
                self.handle_logo_bitmap(domain, context, bytes)
            }
            AsyncToCpu::FetchFailed { domain } => self.handle_fetch_failed(&domain),
            AsyncToCpu::Eof => {
                debug!("Received EOF from async executor.");
                self.eof_received = true;
                self.check_shutdown()
            }
        }
    }

    /// Process fetched HTML, dispatch to worker if within limits.
    fn handle_html(
        &mut self,
        domain: &str,
        path: String,
        depth: usize,
        body: String,
    ) -> ControlFlow<()> {
        let state = self.domains.entry(domain.to_string()).or_default();

        // Log start if this is the first time seeing this domain.
        if state.pages_crawled == 0
            && state.pending_fetches == 0
            && let Err(e) = self.progress.log_started(domain)
        {
            error!("Failed to log STARTED for {domain}: {e}.");
        }

        // Skip if domain is already complete, but decrement pending_fetches.
        if state.logo_found || state.logged_complete {
            state.pending_fetches = state.pending_fetches.saturating_sub(1);
            debug!(
                "Html for completed domain {domain}: pending_fetches={}.",
                state.pending_fetches
            );
            if state.pending_fetches == 0 {
                debug!("Removing completed domain {domain}.");
                self.domains.remove(domain);
                return self.check_shutdown();
            }
            return ControlFlow::Continue(());
        }

        // Skip if depth limit has been reached.
        if depth > self.config.max_depth {
            state.pending_fetches = state.pending_fetches.saturating_sub(1);
            debug!("Html depth limit for {domain}: pending_fetches={}.", state.pending_fetches);
            if state.pending_fetches == 0 {
                self.complete_domain(domain, &CompletionReason::LimitReached);
                return self.check_shutdown();
            }
            return ControlFlow::Continue(());
        }

        // Skip if max number of pages per domain has been reached.
        if state.at_page_limit(&self.config) {
            state.pending_fetches = state.pending_fetches.saturating_sub(1);
            debug!("Html pages limit for {domain}: pending_fetches={}.", state.pending_fetches);
            if state.pending_fetches == 0 {
                self.complete_domain(domain, &CompletionReason::LimitReached);
                return self.check_shutdown();
            }
            return ControlFlow::Continue(());
        }

        state.pages_crawled += 1;
        let payload = WorkPayload::Html(body);
        let work_item = WorkItem { domain: domain.to_string(), path, depth, payload };

        if self.work_tx.send(work_item).is_err() {
            error!("Worker channel closed, cannot dispatch work for {domain}.");
        }

        ControlFlow::Continue(())
    }

    /// Process fetched logo bitmap, dispatch to worker for hashing.
    fn handle_logo_bitmap(
        &mut self,
        domain: &str,
        context: LogoContext,
        bytes: Vec<u8>,
    ) -> ControlFlow<()> {
        if let Some(state) = self.domains.get_mut(domain)
            && (state.logo_found || state.logged_complete)
        {
            state.pending_fetches = state.pending_fetches.saturating_sub(1);
            debug!(
                "LogoBitmap for completed domain {domain}: pending_fetches={}.",
                state.pending_fetches
            );
            if state.pending_fetches == 0 {
                debug!("Removing completed domain {domain}.");
                self.domains.remove(domain);
                return self.check_shutdown();
            }
            return ControlFlow::Continue(());
        }

        let work_item = WorkItem {
            domain: domain.to_string(),
            path: context.found_on_path.clone(),
            depth: 0,
            payload: WorkPayload::LogoBitmap { context, bytes },
        };

        if self.work_tx.send(work_item).is_err() {
            error!("Worker channel closed, cannot dispatch logo work for {domain}.");
        }

        ControlFlow::Continue(())
    }

    /// Handle fetch failure, decrement pending and maybe complete domain.
    fn handle_fetch_failed(&mut self, domain: &str) -> ControlFlow<()> {
        let Some(state) = self.domains.get_mut(domain) else {
            return ControlFlow::Continue(());
        };

        state.pending_fetches = state.pending_fetches.saturating_sub(1);
        debug!("FetchFailed for {domain}: pending_fetches={}.", state.pending_fetches);

        if state.pending_fetches == 0 {
            if state.logged_complete {
                debug!("Removing completed domain {domain}.");
                self.domains.remove(domain);
            } else if !state.logo_found {
                let reason = if state.at_page_limit(&self.config) {
                    CompletionReason::LimitReached
                } else {
                    CompletionReason::NoLogo
                };
                self.complete_domain(domain, &reason);
            }
            return self.check_shutdown();
        }

        ControlFlow::Continue(())
    }

    /// Check if shutdown conditions are met.
    fn check_shutdown(&mut self) -> ControlFlow<()> {
        debug!(
            "check_shutdown: eof_received={}, domains.len()={}.",
            self.eof_received,
            self.domains.len()
        );

        if self.eof_received && self.domains.is_empty() {
            debug!("EOF received and all domains complete, closing async channel.");
            self.async_tx = None;
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    /// Handle a result from a worker.
    fn handle_worker_result(&mut self, result: WorkerResult) {
        debug!("Received worker result for {}.", result.domain);
        let domain = result.domain;

        let pending_added = match result.payload {
            WorkerResultPayload::LogoFound(logo) => {
                self.handle_logo_found(&domain, logo);
                0
            }
            WorkerResultPayload::LogoEmpty => 0,
            WorkerResultPayload::Html { searched_path, new_work } => {
                self.handle_html_result(&domain, &searched_path, new_work)
            }
        };

        let Some(state) = self.domains.get_mut(&domain) else {
            return;
        };

        state.pending_fetches = state.pending_fetches.saturating_sub(1);
        state.pending_fetches += pending_added;

        debug!(
            "Domain {domain} state: pending_fetches={}, logged_complete={}, logo_found={}.",
            state.pending_fetches, state.logged_complete, state.logo_found
        );

        if state.pending_fetches == 0 {
            if state.logged_complete {
                self.domains.remove(&domain);
            } else if !state.logo_found {
                let reason = if state.at_page_limit(&self.config) {
                    CompletionReason::LimitReached
                } else {
                    CompletionReason::NoLogo
                };
                self.complete_domain(&domain, &reason);
            }
        }
    }

    /// Record logo discovery and complete the domain.
    fn handle_logo_found(&mut self, domain: &str, logo: LogoFound) {
        let state = self.domains.entry(domain.to_string()).or_default();
        if !state.logo_found {
            state.logo_found = true;
            let logo = LogoInfo::from(logo);
            self.complete_domain(domain, &CompletionReason::LogoFound(logo));
        }
    }

    /// Process HTML worker result, dispatch new fetch requests.
    fn handle_html_result(
        &mut self,
        domain: &str,
        searched_path: &str,
        new_work: Vec<CpuToAsync>,
    ) -> usize {
        if let Err(e) = self.progress.log_searched(domain, searched_path) {
            error!("Failed to log SEARCHED for {domain} {searched_path}: {e}.");
        }

        let Some(state) = self.domains.get(domain) else {
            error!("Domain: {domain} not found, can't send HTML for processing.");
            return 0;
        };

        if state.logo_found || state.logged_complete || state.at_page_limit(&self.config) {
            debug!("Domain: {domain} is already mark as completed.");
            return 0;
        }

        let Some(ref async_tx) = self.async_tx else {
            debug!("Aysnc channel closed, shutdown in pogress.");
            return 0;
        };

        let mut added = 0;

        for work in new_work {
            if let CpuToAsync::FetchPath { ref path, depth, .. } = work {
                if depth > self.config.max_depth {
                    continue;
                }
                if let Err(e) = self.progress.log_queued(domain, path, depth) {
                    error!("Failed to log QUEUED for {domain} {path}: {e}.");
                }
            }

            match async_tx.blocking_send(work) {
                Ok(()) => added += 1,
                Err(_) => error!("Failed to send work through channel."),
            }
        }

        added
    }

    /// Mark a domain as complete.
    fn complete_domain(&mut self, domain: &str, reason: &CompletionReason) {
        let Some(state) = self.domains.get_mut(domain) else {
            return;
        };

        if !state.logged_complete {
            state.logged_complete = true;
            if let Err(e) = self.progress.log_completed(domain, reason) {
                error!("Failed to log COMPLETED for {domain}: {e}.");
            }
        }

        if state.pending_fetches == 0 {
            self.domains.remove(domain);
        }
    }
}
