//! URL → offline filesystem path mapping and canonical URL identity.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use url::Url;

/// Whether a URL represents a navigable page or a static asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlKind {
    Page,
    Asset,
}

/// Sanitise a string for safe use in a file path component.
fn sanitize_component(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

/// Convert a URL path segment into a safe filesystem component.
fn safe_segment(seg: &str) -> String {
    if seg.is_empty() || seg == "." || seg == ".." {
        return "_".to_string();
    }
    let sane = sanitize_component(seg);
    if sane == "." || sane == ".." || sane.is_empty() {
        "_".to_string()
    } else {
        sane
    }
}

fn host_label(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    match url.port() {
        Some(port) if port != 80 && port != 443 => Some(format!("{host}:{port}")),
        _ => Some(host.to_string()),
    }
}

fn host_prefix(url: &Url, base_host: &str, base_port: Option<u16>) -> Option<String> {
    let host = url.host_str()?;
    let host_norm = host.strip_prefix("www.").unwrap_or(host);
    let base_norm = base_host.strip_prefix("www.").unwrap_or(base_host);
    let same_host = host_norm == base_norm;
    let same_port = url.port() == base_port || (url.port().is_none() && base_port.is_none());
    if same_host && same_port {
        None
    } else {
        let label = host_label(url)?;
        let label_norm = label.strip_prefix("www.").unwrap_or(&label);
        Some(format!("_external/{}", safe_segment(label_norm)))
    }
}

fn is_tracking_param(key: &str) -> bool {
    matches!(
        key,
        "_rsc"
            | "ref"
            | "utm_source"
            | "utm_medium"
            | "utm_campaign"
            | "utm_term"
            | "utm_content"
            | "gclid"
            | "fbclid"
            | "msclkid"
            | "mc_cid"
            | "mc_eid"
    ) || key.starts_with("utm_")
}

/// True when a URL looks like a Next.js RSC / flight request.
pub fn is_rsc_request(url: &Url) -> bool {
    url.query_pairs().any(|(k, _)| k == "_rsc")
        || url.path().contains("_next/static/chunks/") && url.path().contains('%')
}

/// True when a browser/network URL should never be downloaded as a static asset.
pub fn is_dynamic_request(url: &Url) -> bool {
    if is_rsc_request(url) {
        return true;
    }
    let path = url.path().to_lowercase();
    if path.starts_with("/api/")
        || path.starts_with("/cdn-cgi/")
        || path.contains("/__cf_")
        || path.ends_with("/rum")
        || path.contains("cloudflareinsights")
    {
        return true;
    }
    false
}

/// Normalize URL identity for dedup/manifest keys.
pub fn normalize_url(url: &Url, kind: UrlKind, _base_host: &str) -> Url {
    let mut u = url.clone();
    u.set_fragment(None);

    match kind {
        UrlKind::Page => {
            let pairs: Vec<(String, String)> = u
                .query_pairs()
                .filter(|(k, _)| !is_tracking_param(k))
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            u.set_query(None);
            if !pairs.is_empty() {
                let mut sorted = pairs;
                sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
                let q = sorted
                    .iter()
                    .map(|(k, v)| {
                        if v.is_empty() {
                            k.clone()
                        } else {
                            format!("{k}={v}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("&");
                u.set_query(Some(&q));
            }
        }
        UrlKind::Asset => {
            // Keep cache-busting query strings for assets.
        }
    }

    u
}

fn query_suffix(query: Option<&str>, kind: UrlKind) -> String {
    let q = match query {
        Some(q) if !q.is_empty() => q,
        _ => return String::new(),
    };

    match kind {
        UrlKind::Page => format!("@{}", sanitize_component(q)),
        UrlKind::Asset => {
            if q.len() <= 48 {
                format!("@{}", sanitize_component(q))
            } else {
                let mut hasher = Sha256::new();
                hasher.update(q.as_bytes());
                let hash = hex::encode(&hasher.finalize()[..8]);
                format!("@{hash}")
            }
        }
    }
}

fn infer_kind(url: &Url) -> UrlKind {
    classify_by_ext(url.path())
}

/// Backward-compatible wrapper: infer host and kind from the URL.
pub fn url_to_offline_path(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    url_to_offline_path_with(url, host, url.port(), infer_kind(url))
}

/// Convert URL to its offline filesystem relative path.
pub fn url_to_offline_path_with(
    url: &Url,
    base_host: &str,
    base_port: Option<u16>,
    kind: UrlKind,
) -> String {
    let prefix = host_prefix(url, base_host, base_port);
    let raw_path = url.path().trim_start_matches('/');
    let query_suffix = query_suffix(url.query(), kind);

    let segments: Vec<&str> = raw_path.split('/').filter(|s| !s.is_empty()).collect();

    let mut rel = if segments.is_empty() {
        if query_suffix.is_empty() {
            "index.html".to_string()
        } else {
            format!("{query_suffix}/index.html")
        }
    } else {
        let last = segments[segments.len() - 1];
        let has_ext = last.contains('.');
        let ends_with_slash = url.path().ends_with('/');

        let mut parts: Vec<String> = segments[..segments.len().saturating_sub(1)]
            .iter()
            .map(|s| safe_segment(s))
            .collect();

        if ends_with_slash || !has_ext {
            parts.push(if query_suffix.is_empty() {
                safe_segment(last)
            } else {
                format!("{}{}", safe_segment(last), query_suffix)
            });
            parts.push("index.html".to_string());
        } else {
            let safe_last = safe_segment(last);
            if query_suffix.is_empty() {
                parts.push(safe_last);
            } else {
                let dot_pos = safe_last.rfind('.').unwrap_or(safe_last.len());
                let (name, ext) = safe_last.split_at(dot_pos);
                parts.push(format!("{name}{query_suffix}{ext}"));
            }
        }
        parts.join("/")
    };

    if let Some(p) = prefix {
        rel = format!("{p}/{rel}");
    }
    rel
}

/// Backward-compatible wrapper.
pub fn url_to_path(url: &Url, output_base: &str) -> PathBuf {
    let host = url.host_str().unwrap_or("");
    url_to_path_with(url, output_base, host, url.port(), infer_kind(url))
}

/// Join `output_base` with the offline relative path, ensuring the result stays under `output_base`.
pub fn url_to_path_with(
    url: &Url,
    output_base: &str,
    base_host: &str,
    base_port: Option<u16>,
    kind: UrlKind,
) -> PathBuf {
    let rel = url_to_offline_path_with(url, base_host, base_port, kind);
    let base = PathBuf::from(output_base);
    let joined = base.join(&rel);

    let mut clean = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir | Component::CurDir => {}
            other => clean.push(other.as_os_str()),
        }
    }

    let result = if let Ok(stripped) = clean.strip_prefix(&base) {
        base.join(stripped)
    } else if clean.starts_with(&base) {
        clean
    } else {
        base.join(rel.replace("..", "_"))
    };

    if !is_under_output(&result, output_base) {
        base.join(sanitize_component(&rel))
    } else {
        result
    }
}

/// Returns true if `candidate` is inside `output_base`.
pub fn is_under_output(candidate: &Path, output_base: &str) -> bool {
    let base = PathBuf::from(output_base);
    let mut clean_base = PathBuf::new();
    for comp in base.components() {
        match comp {
            Component::ParentDir => {
                clean_base.pop();
            }
            Component::CurDir => {}
            other => clean_base.push(other.as_os_str()),
        }
    }
    let mut clean_cand = PathBuf::new();
    for comp in candidate.components() {
        match comp {
            Component::ParentDir => {
                clean_cand.pop();
            }
            Component::CurDir => {}
            other => clean_cand.push(other.as_os_str()),
        }
    }
    clean_cand.starts_with(&clean_base)
}

/// Whether a static asset from another host should be mirrored offline.
pub fn is_mirrorable_static(url: &Url, base_host: &str, base_port: Option<u16>) -> bool {
    if url.scheme() != "http" && url.scheme() != "https" {
        return false;
    }
    if is_dynamic_request(url) {
        return false;
    }
    let host = match url.host_str() {
        Some(h) => h,
        None => return false,
    };
    let host_norm = host.strip_prefix("www.").unwrap_or(host);
    let base_norm = base_host.strip_prefix("www.").unwrap_or(base_host);
    if host_norm == base_norm {
        let same_port = url.port() == base_port || (url.port().is_none() && base_port.is_none());
        if same_port {
            return true;
        }
    }
    infer_url_kind(url) != UrlKind::Page
}

fn classify_by_ext(path: &str) -> UrlKind {
    let path = path.to_lowercase();
    if path.ends_with(".css")
        || path.ends_with(".js")
        || path.ends_with(".mjs")
        || path.ends_with(".woff")
        || path.ends_with(".woff2")
        || path.ends_with(".ttf")
        || path.ends_with(".otf")
        || path.ends_with(".png")
        || path.ends_with(".jpg")
        || path.ends_with(".jpeg")
        || path.ends_with(".gif")
        || path.ends_with(".svg")
        || path.ends_with(".webp")
        || path.ends_with(".ico")
        || path.ends_with(".avif")
        || path.ends_with(".mp4")
        || path.ends_with(".webm")
        || path == "/css"
        || path == "/css2"
    {
        return UrlKind::Asset;
    }
    if path.ends_with(".html")
        || path.ends_with(".htm")
        || path.ends_with(".php")
        || path.ends_with("/")
        || path.is_empty()
        || !path.rsplit('/').next().unwrap_or("").contains('.')
    {
        UrlKind::Page
    } else {
        UrlKind::Asset
    }
}

/// Infer page vs asset kind from a full URL.
pub fn infer_url_kind(url: &Url) -> UrlKind {
    classify_by_ext(url.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_root_index() {
        let u = Url::parse("https://example.com/").unwrap();
        assert_eq!(
            url_to_offline_path_with(&u, "example.com", None, UrlKind::Page),
            "index.html"
        );
    }

    #[test]
    fn test_directory_page() {
        let u = Url::parse("https://example.com/about").unwrap();
        assert_eq!(
            url_to_offline_path_with(&u, "example.com", None, UrlKind::Page),
            "about/index.html"
        );
    }

    #[test]
    fn test_file_with_ext() {
        let u = Url::parse("https://example.com/img/a.png").unwrap();
        assert_eq!(
            url_to_offline_path_with(&u, "example.com", None, UrlKind::Asset),
            "img/a.png"
        );
    }

    #[test]
    fn test_query_on_page() {
        let u = Url::parse("https://example.com/article?id=1").unwrap();
        assert_eq!(
            url_to_offline_path_with(&u, "example.com", None, UrlKind::Page),
            "article@id=1/index.html"
        );
    }

    #[test]
    fn test_query_on_file() {
        let u = Url::parse("https://example.com/img/photo.png?v=2").unwrap();
        assert_eq!(
            url_to_offline_path_with(&u, "example.com", None, UrlKind::Asset),
            "img/photo@v=2.png"
        );
    }

    #[test]
    fn test_rejects_parent_dir_segments() {
        let u = Url::parse("https://example.com/../../etc/passwd").unwrap();
        let rel = url_to_offline_path_with(&u, "example.com", None, UrlKind::Page);
        assert!(!rel.contains(".."), "path must not contain '..': {rel}");
        let full = url_to_path_with(&u, "out", "example.com", None, UrlKind::Page);
        assert!(is_under_output(&full, "out"));
    }

    #[test]
    fn test_distinct_queries() {
        let u1 = Url::parse("https://example.com/post?id=1").unwrap();
        let u2 = Url::parse("https://example.com/post?id=2").unwrap();
        assert_ne!(
            url_to_offline_path_with(&u1, "example.com", None, UrlKind::Page),
            url_to_offline_path_with(&u2, "example.com", None, UrlKind::Page)
        );
    }

    #[test]
    fn test_tracking_params_stripped_for_pages() {
        let u = Url::parse("https://example.com/?ref=onepagelove&utm_source=x").unwrap();
        let norm = normalize_url(&u, UrlKind::Page, "example.com");
        assert!(norm.query().is_none());
        assert_eq!(norm.as_str(), "https://example.com/");
    }

    #[test]
    fn test_rsc_param_stripped_for_pages() {
        let u = Url::parse("https://example.com/articles?_rsc=abc").unwrap();
        let norm = normalize_url(&u, UrlKind::Page, "example.com");
        assert_eq!(norm.as_str(), "https://example.com/articles");
    }

    #[test]
    fn test_semantic_query_preserved_and_sorted() {
        let u = Url::parse("https://example.com/articles?cat=b&cat=a").unwrap();
        let norm = normalize_url(&u, UrlKind::Page, "example.com");
        assert_eq!(norm.query(), Some("cat=a&cat=b"));
    }

    #[test]
    fn test_external_asset_path() {
        let u = Url::parse("https://fonts.gstatic.com/s/roboto.woff2").unwrap();
        assert_eq!(
            url_to_offline_path_with(&u, "example.com", None, UrlKind::Asset),
            "_external/fonts.gstatic.com/s/roboto.woff2"
        );
    }

    #[test]
    fn test_is_mirrorable_static_external_font() {
        let u = Url::parse("https://fonts.gstatic.com/s/roboto.woff2").unwrap();
        assert!(is_mirrorable_static(&u, "example.com", None));
    }

    #[test]
    fn test_is_not_mirrorable_external_page() {
        let u = Url::parse("https://other.com/about").unwrap();
        assert!(!is_mirrorable_static(&u, "example.com", None));
    }

    #[test]
    fn test_cross_port_external_path() {
        let asset = Url::parse("http://127.0.0.1:2222/roboto.woff2").unwrap();
        let path = url_to_path_with(&asset, "out", "127.0.0.1", Some(1111), UrlKind::Asset);
        let s = path.to_string_lossy().replace('\\', "/");
        assert!(
            s.contains("_external/127.0.0.1_2222"),
            "unexpected path: {s}"
        );
    }

    #[test]
    fn test_is_dynamic_request_rsc() {
        let u = Url::parse("https://example.com/articles?_rsc=1").unwrap();
        assert!(is_dynamic_request(&u));
    }
}
