//! Logo detection heuristics.

use std::collections::HashSet;

use scraper::{Html, Selector};

/// Find logo candidates in an HTML document.
///
/// Returns URLs in priority order:
/// 1. `<link rel="icon">`
/// 2. `<meta property="og:image">`
/// 3. `<img>` with "logo" in src, alt, class, or id attributes
pub fn find_logo_candidates(document: &Html) -> Vec<String> {
    // Chain all sources in priority order, then deduplicate preserving order.
    let candidates = find_icon_links(document)
        .into_iter()
        .chain(find_og_image(document))
        .chain(find_logo_images(document));

    let mut seen = HashSet::new();
    candidates.filter(|url| seen.insert(url.clone())).collect()
}

/// Find Open Graph image meta tag.
fn find_og_image(document: &Html) -> Vec<String> {
    let selector = Selector::parse(r#"meta[property="og:image"]"#).unwrap();

    document
        .select(&selector)
        .filter_map(|el| {
            el.value().attr("content").map(str::trim).filter(|s| !s.is_empty()).map(String::from)
        })
        .collect()
}

/// Find img elements with "logo" in their attributes.
fn find_logo_images(document: &Html) -> Vec<String> {
    let selector = Selector::parse("img[src]").unwrap();

    document
        .select(&selector)
        .filter_map(|el| {
            let attrs = el.value();
            let src = attrs.attr("src").unwrap_or("");
            let alt = attrs.attr("alt").unwrap_or("");
            let class = attrs.attr("class").unwrap_or("");
            let id = attrs.attr("id").unwrap_or("");

            let combined = format!("{src} {alt} {class} {id}");
            if combined.to_lowercase().contains("logo") {
                let src = src.trim();
                (!src.is_empty()).then(|| src.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Find icon link elements.
fn find_icon_links(document: &Html) -> Vec<String> {
    let selector = Selector::parse("link[rel]").unwrap();

    document
        .select(&selector)
        .filter_map(|el| {
            let rel = el.value().attr("rel").unwrap_or("");
            if rel.to_lowercase().contains("icon") {
                el.value().attr("href").map(str::trim).filter(|s| !s.is_empty()).map(String::from)
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_og_image() {
        let html = r#"
            <html>
            <head>
                <meta property="og:image" content="https://example.com/og.png">
                <meta property="og:title" content="Title">
            </head>
            </html>
        "#;
        let document = Html::parse_document(html);
        let og_images = find_og_image(&document);

        assert_eq!(og_images.len(), 1);
        assert_eq!(og_images[0], "https://example.com/og.png");
    }

    #[test]
    fn test_find_logo_images() {
        let html = r#"
            <html>
            <body>
                <img src="/logo.png" alt="Company Logo">
                <img src="/header-logo.svg" class="site-logo">
                <img src="/photo.jpg" alt="Photo">
                <img src="/image.png" id="main-logo">
            </body>
            </html>
        "#;
        let document = Html::parse_document(html);
        let logos = find_logo_images(&document);

        assert_eq!(logos.len(), 3);
        assert!(logos.contains(&"/logo.png".to_string()));
        assert!(logos.contains(&"/header-logo.svg".to_string()));
        assert!(logos.contains(&"/image.png".to_string()));
    }

    #[test]
    fn test_find_logo_candidates_priority() {
        let html = r#"
            <html>
            <head>
                <meta property="og:image" content="/og.png">
            </head>
            <body>
                <img src="/logo.png" alt="Logo">
            </body>
            </html>
        "#;
        let document = Html::parse_document(html);
        let candidates = find_logo_candidates(&document);

        // OG image should come first, then logo image
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], "/og.png");
        assert_eq!(candidates[1], "/logo.png");
    }
}
