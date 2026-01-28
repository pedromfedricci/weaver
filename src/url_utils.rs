//! URL normalization and validation utilities.

use url::Url;

/// Resolve a potentially relative URL against a base URL.
pub fn resolve_url(base: &Url, href: &str) -> Option<Url> {
    let href = href.trim();
    if href.is_empty()
        || href.starts_with("javascript:")
        || href.starts_with("mailto:")
        || href.starts_with("tel:")
        || href.starts_with("data:")
        || href.starts_with('#')
    {
        return None;
    }

    base.join(href).ok()
}

/// Normalize an href to an internal path for the given domain.
///
/// Returns `Some(path)` if the href points to an internal page on the domain,
/// `None` if it's external, invalid, or should be skipped.
pub fn normalize_to_internal_path(href: &str, domain: &str, base_url: &Url) -> Option<String> {
    let resolved = resolve_url(base_url, href)?;

    let host = resolved.host_str()?;
    if !is_same_domain(host, domain) {
        return None;
    }

    if resolved.scheme() != "http" && resolved.scheme() != "https" {
        return None;
    }

    let path = resolved.path();

    let lower_path = path.to_lowercase();
    if is_static_resource(&lower_path) {
        return None;
    }

    Some(normalize_path(path))
}

/// Check if two domains are considered the same (handles www prefix).
fn is_same_domain(host: &str, domain: &str) -> bool {
    let host = host.trim_start_matches("www.");
    let domain = domain.trim_start_matches("www.");
    host.eq_ignore_ascii_case(domain)
}

/// Static resource extensions that shouldn't be crawled.
const STATIC_EXTENSIONS: &[&str] = &[
    ".css", ".js", ".json", ".xml", ".txt", ".pdf", ".doc", ".docx", ".xls", ".xlsx", ".ppt",
    ".pptx", ".zip", ".tar", ".gz", ".rar", ".7z", ".mp3", ".mp4", ".avi", ".mov", ".wmv", ".flv",
    ".webm", ".ogg", ".wav", ".woff", ".woff2", ".ttf", ".eot", ".otf",
];

/// Check if a path points to a static resource that shouldn't be crawled.
fn is_static_resource(lower_path: &str) -> bool {
    STATIC_EXTENSIONS.iter().any(|ext| lower_path.ends_with(ext))
}

/// Normalize a URL path (remove trailing slash, handle empty).
fn normalize_path(path: &str) -> String {
    let path = path.trim();

    if path.is_empty() || path == "/" {
        return "/".to_string();
    }

    let path = path.trim_end_matches('/');

    if path.starts_with('/') { path.to_string() } else { format!("/{path}") }
}

/// Build a base URL for a domain.
pub fn build_base_url(domain: &str) -> Option<Url> {
    Url::parse(&format!("https://{domain}/")).ok()
}

/// Build a full URL from domain and path.
pub fn build_url(domain: &str, path: &str) -> Option<Url> {
    let base = build_base_url(domain)?;
    base.join(path).ok()
}

/// Resolve a logo URL (could be absolute or relative) to a full URL.
pub fn resolve_logo_url(logo_href: &str, domain: &str, page_path: &str) -> Option<String> {
    let href = logo_href.trim();

    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }

    if href.starts_with("//") {
        return Some(format!("https:{href}"));
    }

    let base = build_url(domain, page_path)?;
    let resolved = resolve_url(&base, href)?;

    Some(resolved.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("/about/"), "/about");
        assert_eq!(normalize_path("/about"), "/about");
        assert_eq!(normalize_path("about"), "/about");
    }

    #[test]
    fn test_is_same_domain() {
        assert!(is_same_domain("example.com", "example.com"));
        assert!(is_same_domain("www.example.com", "example.com"));
        assert!(is_same_domain("example.com", "www.example.com"));
        assert!(is_same_domain("EXAMPLE.COM", "example.com"));
        assert!(!is_same_domain("other.com", "example.com"));
    }

    #[test]
    fn test_normalize_to_internal_path() {
        let base = Url::parse("https://example.com/page").unwrap();

        // Internal paths
        assert_eq!(
            normalize_to_internal_path("/about", "example.com", &base),
            Some("/about".to_string())
        );
        assert_eq!(
            normalize_to_internal_path("./contact", "example.com", &base),
            Some("/contact".to_string())
        );
        assert_eq!(
            normalize_to_internal_path("https://example.com/test", "example.com", &base),
            Some("/test".to_string())
        );

        // External should return None
        assert_eq!(
            normalize_to_internal_path("https://other.com/page", "example.com", &base),
            None
        );

        // Special URLs should return None
        assert_eq!(normalize_to_internal_path("javascript:void(0)", "example.com", &base), None);
        assert_eq!(
            normalize_to_internal_path("mailto:test@example.com", "example.com", &base),
            None
        );
        assert_eq!(normalize_to_internal_path("#section", "example.com", &base), None);

        // Static resources should return None
        assert_eq!(normalize_to_internal_path("/style.css", "example.com", &base), None);
    }

    #[test]
    fn test_resolve_logo_url() {
        // Absolute URL
        assert_eq!(
            resolve_logo_url("https://example.com/logo.png", "example.com", "/"),
            Some("https://example.com/logo.png".to_string())
        );

        // Protocol-relative
        assert_eq!(
            resolve_logo_url("//cdn.example.com/logo.png", "example.com", "/"),
            Some("https://cdn.example.com/logo.png".to_string())
        );

        // Relative to root
        assert_eq!(
            resolve_logo_url("/images/logo.png", "example.com", "/about"),
            Some("https://example.com/images/logo.png".to_string())
        );

        // Relative to current path
        assert_eq!(
            resolve_logo_url("logo.png", "example.com", "/about/"),
            Some("https://example.com/about/logo.png".to_string())
        );
    }
}
