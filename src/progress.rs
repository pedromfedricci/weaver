//! Progress logging and crash recovery.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::str::FromStr;

use log::{trace, warn};

use crate::messages::LogoFound;

/// Logo information for a found logo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogoInfo {
    pub page_path: String,
    pub logo_url: String,
    pub hash: String,
}

/// Completion reasons for a domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionReason {
    LogoFound(LogoInfo),
    NoLogo,
    LimitReached,
}

impl fmt::Display for CompletionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LogoFound(logo) => {
                write!(f, "LOGO_FOUND {} {} {}", logo.page_path, logo.logo_url, logo.hash)
            }
            Self::NoLogo => write!(f, "NO_LOGO"),
            Self::LimitReached => write!(f, "LIMIT_REACHED"),
        }
    }
}

impl From<LogoFound> for LogoInfo {
    fn from(logo: LogoFound) -> Self {
        let page_path = logo.page_path;
        let logo_url = logo.logo_url;
        let hash = logo.hash;
        Self { page_path, logo_url, hash }
    }
}

/// A parsed log entry.
#[derive(Debug, Clone)]
pub enum LogEntry {
    /// Started domain entry.
    Started(String),
    /// Queued path for domain entry (discovered but not yet searched).
    Queued { domain: String, path: String, depth: usize },
    /// Searched domain and path entry.
    Searched { domain: String, path: String },
    /// Completed search for domain entry.
    Completed { domain: String, reason: CompletionReason },
}

impl fmt::Display for LogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Started(domain) => write!(f, "STARTED {domain}"),
            Self::Queued { domain, path, depth } => write!(f, "QUEUED {domain} {path} {depth}"),
            Self::Searched { domain, path } => write!(f, "SEARCHED {domain} {path}"),
            Self::Completed { domain, reason } => write!(f, "COMPLETED {domain} {reason}"),
        }
    }
}

/// Error when parsing a log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseLogEntryError;

impl fmt::Display for ParseLogEntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid log entry format")
    }
}

impl std::error::Error for ParseLogEntryError {}

impl FromStr for LogEntry {
    type Err = ParseLogEntryError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let line = line.trim();
        if line.is_empty() {
            return Err(ParseLogEntryError);
        }

        let (entry_type, rest) = line.split_once(' ').ok_or(ParseLogEntryError)?;

        match entry_type {
            "STARTED" => Ok(Self::Started(rest.to_string())),

            "QUEUED" => {
                let parts: Vec<&str> = rest.splitn(3, ' ').collect();
                if parts.len() != 3 {
                    return Err(ParseLogEntryError);
                }
                let depth = parts[2].parse().map_err(|_| ParseLogEntryError)?;
                Ok(Self::Queued { domain: parts[0].to_string(), path: parts[1].to_string(), depth })
            }

            "SEARCHED" => {
                let (domain, path) = rest.split_once(' ').ok_or(ParseLogEntryError)?;
                Ok(Self::Searched { domain: domain.to_string(), path: path.to_string() })
            }

            "COMPLETED" => {
                let parts: Vec<&str> = rest.splitn(5, ' ').collect();
                if parts.len() < 2 {
                    return Err(ParseLogEntryError);
                }
                let domain = parts[0].to_string();
                let reason = match parts[1] {
                    "LOGO_FOUND" if parts.len() == 5 => CompletionReason::LogoFound(LogoInfo {
                        page_path: parts[2].to_string(),
                        logo_url: parts[3].to_string(),
                        hash: parts[4].to_string(),
                    }),
                    "NO_LOGO" => CompletionReason::NoLogo,
                    "LIMIT_REACHED" => CompletionReason::LimitReached,
                    _ => return Err(ParseLogEntryError),
                };
                Ok(Self::Completed { domain, reason })
            }

            _ => Err(ParseLogEntryError),
        }
    }
}

/// Progress state for a single domain during recovery.
#[derive(Debug, Default)]
pub struct DomainProgress {
    searched_paths: HashSet<String>,
    queued_paths: HashMap<String, usize>,
}

impl DomainProgress {
    /// Get the set of searched paths.
    pub const fn searched_paths(&self) -> &HashSet<String> {
        &self.searched_paths
    }

    /// Get the map of queued paths to their depths.
    pub const fn queued_paths(&self) -> &HashMap<String, usize> {
        &self.queued_paths
    }

    /// Mark a path as queued with the given depth.
    fn queue_path(&mut self, path: String, depth: usize) {
        self.queued_paths.insert(path, depth);
    }

    /// Mark a path as searched, removing it from queued if present.
    fn mark_searched(&mut self, path: &str) {
        self.searched_paths.insert(path.to_string());
        self.queued_paths.remove(path);
    }
}

/// Overall crawl state recovered from progress log.
#[derive(Debug, Default)]
pub struct CrawlState {
    in_progress: HashMap<String, DomainProgress>,
    completed: HashMap<String, CompletionReason>,
}

impl CrawlState {
    /// Check if a domain should be skipped (already completed).
    pub fn should_skip(&self, domain: &str) -> bool {
        self.completed.contains_key(domain)
    }

    /// Get domains that are in progress.
    pub const fn in_progress(&self) -> &HashMap<String, DomainProgress> {
        &self.in_progress
    }

    /// Get completed domains.
    pub const fn completed(&self) -> &HashMap<String, CompletionReason> {
        &self.completed
    }

    /// Record that a domain was started.
    fn start_domain(&mut self, domain: String) {
        self.in_progress.insert(domain, DomainProgress::default());
    }

    /// Record that a path was queued for a domain.
    fn queue_path(&mut self, domain: &str, path: String, depth: usize) {
        if let Some(dp) = self.in_progress.get_mut(domain) {
            dp.queue_path(path, depth);
        }
    }

    /// Record that a path was searched for a domain.
    fn mark_searched(&mut self, domain: &str, path: &str) {
        if let Some(dp) = self.in_progress.get_mut(domain) {
            dp.mark_searched(path);
        }
    }

    /// Record that a domain was completed.
    fn complete_domain(&mut self, domain: String, reason: CompletionReason) {
        self.in_progress.remove(&domain);
        self.completed.insert(domain, reason);
    }
}

/// Load progress state from an existing log file.
pub fn load_progress(path: &Path) -> CrawlState {
    let mut state = CrawlState::default();

    let Ok(file) = File::open(path) else { return state };

    for (line_num, line_result) in BufReader::new(file).lines().enumerate() {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                warn!("IO error reading progress log at line {}: {}.", line_num + 1, e);
                continue;
            }
        };

        match line.parse::<LogEntry>() {
            Ok(entry) => match entry {
                LogEntry::Started(domain) => state.start_domain(domain),
                LogEntry::Queued { domain, path, depth } => state.queue_path(&domain, path, depth),
                LogEntry::Searched { domain, path } => state.mark_searched(&domain, &path),
                LogEntry::Completed { domain, reason } => state.complete_domain(domain, reason),
            },
            Err(e) => {
                trace!(
                    "Skipping unparseable log entry at line {}: '{}', error: {e}",
                    line_num + 1,
                    line
                );
            }
        }
    }

    state
}

/// Progress log writer.
pub struct ProgressWriter {
    file: File,
}

impl ProgressWriter {
    /// Create a new progress writer, appending to the file.
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file })
    }

    /// Write a log entry to the progress file.
    fn write_entry(&mut self, entry: &LogEntry) -> std::io::Result<()> {
        writeln!(self.file, "{entry}")?;
        self.file.flush()
    }

    /// Log that a domain crawl has started.
    pub fn log_started(&mut self, domain: &str) -> std::io::Result<()> {
        self.write_entry(&LogEntry::Started(domain.to_string()))
    }

    /// Log that a path was queued for fetching.
    pub fn log_queued(&mut self, domain: &str, path: &str, depth: usize) -> std::io::Result<()> {
        self.write_entry(&LogEntry::Queued {
            domain: domain.to_string(),
            path: path.to_string(),
            depth,
        })
    }

    /// Log that a path was searched.
    pub fn log_searched(&mut self, domain: &str, path: &str) -> std::io::Result<()> {
        self.write_entry(&LogEntry::Searched { domain: domain.to_string(), path: path.to_string() })
    }

    /// Log that a domain is completed.
    pub fn log_completed(
        &mut self,
        domain: &str,
        reason: &CompletionReason,
    ) -> std::io::Result<()> {
        self.write_entry(&LogEntry::Completed {
            domain: domain.to_string(),
            reason: reason.clone(),
        })
    }
}

/// Generate CSV from progress log.
pub fn generate_csv(log_path: &Path, csv_path: &Path) -> std::io::Result<()> {
    let state = load_progress(log_path);
    let mut writer = csv::Writer::from_path(csv_path)?;

    writer.write_record(["domain", "page_path", "logo_url", "hash"])?;

    for (domain, reason) in state.completed() {
        if let CompletionReason::LogoFound(logo) = reason {
            writer.write_record([
                domain.as_str(),
                logo.page_path.as_str(),
                logo.logo_url.as_str(),
                logo.hash.as_str(),
            ])?;
        }
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn test_parse_log_entry() {
        assert!(matches!(
            "STARTED example.com".parse::<LogEntry>(),
            Ok(LogEntry::Started(d)) if d == "example.com"
        ));

        assert!(matches!(
            "QUEUED example.com /about 2".parse::<LogEntry>(),
            Ok(LogEntry::Queued { domain, path, depth })
            if domain == "example.com" && path == "/about" && depth == 2
        ));

        assert!(matches!(
            "SEARCHED example.com /about".parse::<LogEntry>(),
            Ok(LogEntry::Searched { domain, path }) if domain == "example.com" && path == "/about"
        ));

        let entry: Result<LogEntry, _> =
            "COMPLETED example.com LOGO_FOUND /about https://example.com/logo.png abc123".parse();
        assert!(matches!(
            entry,
            Ok(LogEntry::Completed { domain, reason: CompletionReason::LogoFound(ref logo) })
            if domain == "example.com"
                && logo.page_path == "/about"
                && logo.logo_url == "https://example.com/logo.png"
                && logo.hash == "abc123"
        ));

        assert!(matches!(
            "COMPLETED example.com NO_LOGO".parse::<LogEntry>(),
            Ok(LogEntry::Completed { domain, reason: CompletionReason::NoLogo }) if domain == "example.com"
        ));

        // Invalid entries should fail to parse.
        assert!("".parse::<LogEntry>().is_err());
        assert!("INVALID".parse::<LogEntry>().is_err());
        assert!("QUEUED example.com".parse::<LogEntry>().is_err());
    }

    #[test]
    fn test_log_entry_roundtrip() {
        let entries = [
            LogEntry::Started("example.com".to_string()),
            LogEntry::Queued {
                domain: "example.com".to_string(),
                path: "/about".to_string(),
                depth: 2,
            },
            LogEntry::Searched { domain: "example.com".to_string(), path: "/contact".to_string() },
            LogEntry::Completed {
                domain: "example.com".to_string(),
                reason: CompletionReason::NoLogo,
            },
            LogEntry::Completed {
                domain: "example.com".to_string(),
                reason: CompletionReason::LimitReached,
            },
            LogEntry::Completed {
                domain: "example.com".to_string(),
                reason: CompletionReason::LogoFound(LogoInfo {
                    page_path: "/".to_string(),
                    logo_url: "https://example.com/logo.png".to_string(),
                    hash: "abc123".to_string(),
                }),
            },
        ];

        for entry in entries {
            let serialized = entry.to_string();
            let parsed: LogEntry = serialized.parse().expect("should parse");
            assert_eq!(serialized, parsed.to_string());
        }
    }

    #[test]
    fn test_load_progress() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "STARTED example.com").unwrap();
        writeln!(file, "QUEUED example.com /about 1").unwrap();
        writeln!(file, "QUEUED example.com /contact 1").unwrap();
        writeln!(file, "SEARCHED example.com /").unwrap();
        writeln!(file, "SEARCHED example.com /about").unwrap();
        writeln!(file, "STARTED other.com").unwrap();
        writeln!(file, "COMPLETED other.com NO_LOGO").unwrap();

        let state = load_progress(file.path());

        assert!(state.in_progress().contains_key("example.com"));
        let dp = state.in_progress().get("example.com").unwrap();
        assert!(dp.searched_paths().contains("/"));
        assert!(dp.searched_paths().contains("/about"));
        // /about was queued then searched, so not in queued_paths.
        assert!(!dp.queued_paths().contains_key("/about"));
        // /contact was queued but not searched.
        assert_eq!(dp.queued_paths().get("/contact"), Some(&1));

        assert!(state.completed().contains_key("other.com"));
        assert!(!state.in_progress().contains_key("other.com"));
    }

    #[test]
    fn test_should_skip() {
        let mut state = CrawlState::default();
        state.complete_domain(
            "done.com".to_string(),
            CompletionReason::LogoFound(LogoInfo {
                page_path: "/".to_string(),
                logo_url: "https://example.com/logo.png".to_string(),
                hash: "abc123".to_string(),
            }),
        );

        assert!(state.should_skip("done.com"));
        assert!(!state.should_skip("new.com"));
    }
}
