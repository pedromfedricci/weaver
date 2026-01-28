//! Message types for inter-component communication.

/// Preserved context through logo fetch round-trip.
#[derive(Debug, Clone)]
pub struct LogoContext {
    pub found_on_path: String,
    pub candidate_url: String,
}

/// Messages from async executor to coordinator.
#[derive(Debug)]
pub enum AsyncToCpu {
    Html {
        domain: String,
        path: String,
        depth: usize,
        body: String,
    },
    LogoBitmap {
        domain: String,
        context: LogoContext,
        bytes: Vec<u8>,
    },
    /// Signal that a fetch failed (HTTP error, network error, etc.).
    FetchFailed {
        domain: String,
    },
    /// Signal that stdin is closed and all fetch tasks are complete.
    Eof,
}

/// Messages from coordinator to async executor.
#[derive(Debug, Clone)]
pub enum CpuToAsync {
    FetchPath { domain: String, path: String, depth: usize },
    FetchLogo { domain: String, url: String, context: LogoContext },
}

/// Payload for work items sent to workers.
#[derive(Debug)]
pub enum WorkPayload {
    Html(String),
    LogoBitmap { context: LogoContext, bytes: Vec<u8> },
}

/// Work item sent from coordinator to worker pool.
#[derive(Debug)]
pub struct WorkItem {
    pub domain: String,
    pub path: String,
    pub depth: usize,
    pub payload: WorkPayload,
}

/// Result of finding a logo.
#[derive(Debug, Clone)]
pub struct LogoFound {
    pub page_path: String,
    pub logo_url: String,
    pub hash: String,
}

/// Payload variants for worker results.
#[derive(Debug)]
pub enum WorkerResultPayload {
    /// Result from HTML processing: searched path and new work to dispatch.
    Html { searched_path: String, new_work: Vec<CpuToAsync> },
    /// Result from logo processing: logo was found and hashed.
    LogoFound(LogoFound),
    /// Result from logo processing: empty bytes (no logo).
    LogoEmpty,
}

/// Result from a worker back to coordinator.
#[derive(Debug)]
pub struct WorkerResult {
    pub domain: String,
    pub payload: WorkerResultPayload,
}
