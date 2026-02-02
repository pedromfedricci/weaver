//! Custom error types for the weave logo discovery tool.

#![allow(dead_code)]

use std::error::Error;
use std::fmt;

/// Errors from the async executor (HTTP fetching, stdin reading).
#[derive(Debug)]
pub enum AsyncExecutorError {
    /// Failed to build HTTP client.
    HttpClientBuild(reqwest::Error),
    /// Failed to read from stdin.
    StdinRead(std::io::Error),
    /// HTTP request failed.
    HttpRequest { url: String, source: reqwest::Error },
    /// HTTP response had non-success status.
    HttpStatus { url: String, status: reqwest::StatusCode },
    /// Response body was not HTML content type.
    NonHtmlContent { url: String, content_type: String },
    /// Failed to decode response body.
    BodyDecode { url: String, source: reqwest::Error },
    /// Channel send failed (receiver dropped).
    ChannelSend,
    /// Semaphore closed (shutdown in progress).
    SemaphoreClosed,
}

impl fmt::Display for AsyncExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HttpClientBuild(e) => write!(f, "failed to build HTTP client: {e}"),
            Self::StdinRead(e) => write!(f, "stdin read error: {e}"),
            Self::HttpRequest { url, source } => {
                write!(f, "HTTP request failed for {url}: {source}")
            }
            Self::HttpStatus { url, status } => write!(f, "HTTP {status} for {url}"),
            Self::NonHtmlContent { url, content_type } => {
                write!(f, "non-HTML content type '{content_type}' for {url}")
            }
            Self::BodyDecode { url, source } => {
                write!(f, "body decode failed for {url}: {source}")
            }
            Self::ChannelSend => write!(f, "channel send failed"),
            Self::SemaphoreClosed => write!(f, "semaphore closed during shutdown"),
        }
    }
}

impl Error for AsyncExecutorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::HttpClientBuild(e) => Some(e),
            Self::StdinRead(e) => Some(e),
            Self::HttpRequest { source, .. } | Self::BodyDecode { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Errors from the coordinator.
#[derive(Debug)]
pub enum CoordinatorError {
    /// Failed to write to progress log.
    ProgressWrite(std::io::Error),
    /// Worker channel send failed (workers shut down).
    WorkerChannelSend,
    /// Async channel send failed (executor shut down).
    AsyncChannelSend,
    /// Domain state not found (logic error).
    DomainStateNotFound(String),
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProgressWrite(e) => write!(f, "progress log write failed: {e}"),
            Self::WorkerChannelSend => write!(f, "worker channel send failed"),
            Self::AsyncChannelSend => write!(f, "async channel send failed"),
            Self::DomainStateNotFound(d) => write!(f, "domain state not found for '{d}'"),
        }
    }
}

impl Error for CoordinatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ProgressWrite(e) => Some(e),
            _ => None,
        }
    }
}

/// Errors from worker threads.
#[derive(Debug)]
pub enum WorkerError {
    /// Result channel send failed (coordinator shut down).
    ResultChannelSend,
    /// Work channel recv failed (coordinator shut down).
    WorkChannelRecv,
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResultChannelSend => write!(f, "result channel send failed"),
            Self::WorkChannelRecv => write!(f, "work channel closed"),
        }
    }
}

impl Error for WorkerError {}
