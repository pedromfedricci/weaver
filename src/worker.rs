//! Worker pool for CPU-bound tasks: HTML parsing, heuristics, and hashing.

use std::collections::HashSet;
use std::thread;

use crossbeam::channel::{Receiver, Sender};
use log::{debug, warn};
use scraper::{Html, Selector};
use sha2::{Digest, Sha256};

use crate::heuristics::find_logo_candidates;
use crate::messages::{
    CpuToAsync, LogoContext, LogoFound, WorkItem, WorkPayload, WorkerResult, WorkerResultPayload,
};
use crate::url_utils::{build_base_url, normalize_to_internal_path, resolve_logo_url};

/// Spawn a pool of worker threads.
pub fn spawn_worker_pool(
    num_workers: usize,
    work_rx: &Receiver<WorkItem>,
    result_tx: &Sender<WorkerResult>,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::with_capacity(num_workers);

    for _ in 0..num_workers {
        let work_rx = work_rx.clone();
        let result_tx = result_tx.clone();

        let handle = thread::spawn(move || {
            while let Ok(item) = work_rx.recv() {
                let result = process_work_item(item);
                if result_tx.send(result).is_err() {
                    warn!("Result channel closed, worker exiting.");
                    break;
                }
            }
            debug!("Worker thread exiting (work channel closed).");
        });

        handles.push(handle);
    }

    handles
}

/// Process a single work item.
fn process_work_item(item: WorkItem) -> WorkerResult {
    match item.payload {
        WorkPayload::Html(body) => process_html(item.domain, item.path, item.depth, &body),
        WorkPayload::LogoBitmap { context, bytes } => process_logo(item.domain, context, &bytes),
    }
}

/// Process an HTML document: find logos and extract links.
fn process_html(domain: String, path: String, depth: usize, body: &str) -> WorkerResult {
    let document = Html::parse_document(body);
    let logo_candidates = find_logo_candidates(&document);
    let mut new_work = Vec::new();

    for candidate_url in logo_candidates {
        if let Some(url) = resolve_logo_url(&candidate_url, &domain, &path) {
            let (found_on_path, domain) = (path.clone(), domain.clone());
            let context = LogoContext { found_on_path, candidate_url };

            new_work.push(CpuToAsync::FetchLogo { domain, url, context });
        }
    }

    let internal_links = extract_internal_links(&document, &domain, &path);

    for link_path in internal_links {
        new_work.push(CpuToAsync::FetchPath {
            domain: domain.clone(),
            path: link_path,
            depth: depth + 1,
        });
    }

    let searched_path = path;
    let payload = WorkerResultPayload::Html { searched_path, new_work };

    WorkerResult { domain, payload }
}

/// Process a logo bitmap: hash the bytes.
fn process_logo(domain: String, context: LogoContext, bytes: &[u8]) -> WorkerResult {
    if bytes.is_empty() {
        return WorkerResult { domain, payload: WorkerResultPayload::LogoEmpty };
    }

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let hash = format!("{:x}", hasher.finalize());

    let page_path = context.found_on_path;
    let logo_url = context.candidate_url;
    let logo = LogoFound { page_path, logo_url, hash };
    let payload = WorkerResultPayload::LogoFound(logo);

    WorkerResult { domain, payload }
}

/// Extract internal links from an HTML document.
fn extract_internal_links(document: &Html, domain: &str, current_path: &str) -> Vec<String> {
    let mut paths = HashSet::new();

    let base_url = match build_base_url(domain) {
        Some(url) => url.join(current_path).unwrap_or(url),
        None => return vec![],
    };

    let a_selector = Selector::parse("a[href]").unwrap();

    for element in document.select(&a_selector) {
        if let Some(href) = element.value().attr("href")
            && let Some(path) = normalize_to_internal_path(href, domain, &base_url)
        {
            paths.insert(path);
        }
    }

    paths.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_html_finds_logos() {
        let html = r#"
            <html>
            <head>
                <link rel="icon" href="/favicon.ico">
            </head>
            <body>
                <img src="/logo.png" alt="Logo">
                <a href="/about">About</a>
            </body>
            </html>
        "#;

        let result = process_html("example.com".to_string(), "/".to_string(), 0, html);

        assert_eq!(result.domain, "example.com");

        let WorkerResultPayload::Html { searched_path, new_work } = result.payload else {
            panic!("Expected Html payload");
        };

        assert_eq!(searched_path, "/");

        let logo_fetches = new_work.iter().find(|w| matches!(w, CpuToAsync::FetchLogo { .. }));
        assert!(logo_fetches.is_some());

        let path_fetches = new_work.iter().find(|w| matches!(w, CpuToAsync::FetchPath { .. }));
        assert!(path_fetches.is_some());
    }

    #[test]
    fn test_process_logo_hashes() {
        let bytes = vec![0x89, 0x50, 0x4E, 0x47];
        let context =
            LogoContext { found_on_path: "/".to_string(), candidate_url: "/logo.png".to_string() };

        let result = process_logo("example.com".to_string(), context, &bytes);

        let WorkerResultPayload::LogoFound(logo) = result.payload else {
            panic!("Expected LogoFound payload");
        };

        assert!(!logo.hash.is_empty());
        assert_eq!(logo.page_path, "/");
        assert_eq!(logo.logo_url, "/logo.png");
        assert_eq!(logo.hash.len(), 64);
    }

    #[test]
    fn test_process_logo_empty_bytes() {
        let context =
            LogoContext { found_on_path: "/".to_string(), candidate_url: "/logo.png".to_string() };

        let result = process_logo("example.com".to_string(), context, &[]);

        assert!(matches!(result.payload, WorkerResultPayload::LogoEmpty));
    }

    #[test]
    fn test_extract_internal_links() {
        let html = r#"
            <html>
            <body>
                <a href="/about">About</a>
                <a href="/contact/">Contact</a>
                <a href="https://example.com/page">Page</a>
                <a href="https://other.com/external">External</a>
                <a href="javascript:void(0)">JS</a>
                <a href="mailto:test@example.com">Email</a>
            </body>
            </html>
        "#;

        let document = Html::parse_document(html);
        let links = extract_internal_links(&document, "example.com", "/");

        assert!(links.contains(&"/about".to_string()));
        assert!(links.contains(&"/contact".to_string()));
        assert!(links.contains(&"/page".to_string()));
        assert!(!links.iter().any(|l| l.contains("other.com")));
        assert!(!links.iter().any(|l| l.contains("javascript")));
        assert!(!links.iter().any(|l| l.contains("mailto")));
    }
}
