use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use url::Url;

use crate::pathmap::{self, UrlKind};

fn relative_path(
    page_path: &str,
    target_url: &Url,
    base_host: &str,
    base_port: Option<u16>,
    target_kind: UrlKind,
) -> String {
    let target_path =
        pathmap::url_to_offline_path_with(target_url, base_host, base_port, target_kind);

    let page_dir = Path::new(page_path).parent().unwrap_or(Path::new(""));

    if page_dir.as_os_str().is_empty() && target_path == "index.html" {
        return target_path;
    }

    let dir_comps: Vec<&str> = page_dir
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    let target_comps: Vec<&str> = target_path.split('/').filter(|s| !s.is_empty()).collect();

    let common = dir_comps
        .iter()
        .zip(target_comps.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut result = String::new();
    for _ in common..dir_comps.len() {
        result.push_str("../");
    }

    for (i, comp) in target_comps.iter().enumerate().skip(common) {
        if i > common {
            result.push('/');
        }
        result.push_str(comp);
    }

    if result.is_empty() {
        ".".to_string()
    } else {
        result
    }
}

fn infer_kind(url: &Url) -> UrlKind {
    pathmap::infer_url_kind(url)
}

fn url_to_offline_path(
    url: &Url,
    base_host: &str,
    base_port: Option<u16>,
) -> (String, Option<String>) {
    let kind = infer_kind(url);
    let path = pathmap::url_to_offline_path_with(url, base_host, base_port, kind);
    let ext = Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_string());
    (path, ext)
}

fn attr_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)(\s+(?:href|src|action|poster|data-src|data-lazy-src)\s*=\s*)(?:"([^"]*?)"|'([^']*?)')"#,
        )
        .unwrap()
    })
}

fn srcset_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Match both srcset and data-srcset (lazy-loaded responsive images).
        Regex::new(r#"(?i)(\s+(?:srcset|data-srcset)\s*=\s*)(?:\"([^\"]*?)\"|'([^']*?)')"#).unwrap()
    })
}

fn style_attr_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)(\s+style\s*=\s*)\"([^\"]*?)\""#).unwrap())
}

fn meta_refresh_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // `<meta http-equiv="refresh" content="0;url=...">` — unrewritten,
        // they bounce offline readers back to the live site.
        Regex::new(
            r#"(?is)<meta\b[^>]*?\bhttp-equiv\s*=\s*[\"']refresh[\"'][^>]*?\bcontent\s*=\s*[\"']([^\"']*)[\"'][^>]*?>"#,
        )
        .unwrap()
    })
}

fn base_tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)<base\b[^>]*>"#).unwrap())
}

fn css_url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"url\(\s*['"]?([^'")]+)['"]?\s*\)"#).unwrap())
}

fn css_import_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"@import\s+['"]([^'"]+)['"]"#).unwrap())
}

fn extract_base_href(html: &str, page_url: &Url) -> Option<Url> {
    let base_re = regex::Regex::new(r#"(?i)<base\s+[^>]*href\s*=\s*["']([^"']+)["']"#).ok()?;
    let cap = base_re.captures(html)?;
    page_url.join(cap.get(1)?.as_str()).ok()
}

#[cfg(feature = "render")]
#[cfg(feature = "render")]
fn script_tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?si)<script\b[^>]*>.*?</script>").unwrap())
}

/// Check whether a `<script ...>` opening tag is a data/non-executable
/// script that should be kept in the offline copy (structured data,
/// templates, speculation rules).
#[cfg(feature = "render")]
fn is_data_script(open_tag: &str) -> bool {
    let re = regex::Regex::new(
        r#"(?i)\btype\s*=\s*[\"'](?:application/ld\+json|text/template|speculationrules|application/json)[\"']"#,
    );
    match re {
        Ok(re) => re.is_match(open_tag),
        Err(_) => false,
    }
}

/// Remove executable `<script>` tags from HTML. This is used for SPA pages
/// so the framework doesn't re-hydrate and wipe the DOM when API calls fail
/// offline. Data scripts (JSON-LD, templates) are kept.
#[cfg(feature = "render")]
pub fn strip_scripts(html: &str) -> String {
    if !html.to_lowercase().contains("<script") {
        return html.to_string();
    }
    let re = script_tag_regex();
    let mut result = String::with_capacity(html.len());
    let mut last_end = 0;
    for m in re.find_iter(html) {
        let open_tag_end = html[m.start()..m.end()]
            .find('>')
            .map(|i| m.start() + i)
            .unwrap_or(m.start());
        let open_tag = &html[m.start()..open_tag_end];
        if is_data_script(open_tag) {
            continue;
        }
        result.push_str(&html[last_end..m.start()]);
        last_end = m.end();
    }
    result.push_str(&html[last_end..]);
    result
}

fn split_fragment(value: &str) -> (String, Option<String>) {
    match value.find('#') {
        Some(i) => (value[..i].to_string(), Some(value[i..].to_string())),
        None => (value.to_string(), None),
    }
}

fn rewrite_url_value(
    value: &str,
    base_url: &Url,
    page_path: &str,
    base_host: &str,
    base_port: Option<u16>,
) -> Option<String> {
    if value.starts_with("javascript:")
        || value.starts_with("mailto:")
        || value.starts_with("tel:")
        || value.starts_with("data:")
        || value.starts_with("blob:")
    {
        return None;
    }

    if value.starts_with('#') {
        return None;
    }

    let (path_part, fragment) = split_fragment(value);
    let resolved = base_url.join(&path_part).ok()?;

    if resolved.scheme() != "http" && resolved.scheme() != "https" {
        return None;
    }

    if pathmap::is_dynamic_request(&resolved) {
        return None;
    }

    let kind = infer_kind(&resolved);
    if kind == UrlKind::Page && !crate::crawler::is_same_domain(&resolved, base_host) {
        // Keep external navigation links as-is for online use.
        return None;
    }
    if !pathmap::is_mirrorable_static(&resolved, base_host, base_port) {
        return None;
    }

    let new_path = relative_path(page_path, &resolved, base_host, base_port, kind);
    let with_fragment = match fragment {
        Some(f) => format!("{new_path}{f}"),
        None => new_path,
    };

    if with_fragment == value || with_fragment == "." || with_fragment == page_path {
        return None;
    }

    Some(with_fragment)
}

pub fn rewrite_html(html: &str, page_url: &Url, base_host: &str, base_port: Option<u16>) -> String {
    if !html.contains("href=")
        && !html.contains("src=")
        && !html.contains("srcset=")
        && !html.contains("data-src")
        && !html.contains("style=")
    {
        return strip_offline_breakers(html.to_string());
    }

    let base_url = extract_base_href(html, page_url).unwrap_or_else(|| page_url.clone());
    let page_path = {
        let (p, _) = url_to_offline_path(page_url, base_host, base_port);
        p
    };

    let attr_re = attr_regex();
    let srcset_re = srcset_regex();
    let style_re = style_attr_regex();

    let mut result = String::with_capacity(html.len() + 4096);
    let mut last_end = 0;

    #[derive(Clone, Copy, PartialEq)]
    enum SpanKind {
        Attr,
        Srcset,
        Style,
    }

    #[derive(Clone, Copy)]
    struct Span {
        start: usize,
        end: usize,
        kind: SpanKind,
    }

    let mut spans: Vec<Span> = Vec::new();
    for m in attr_re.find_iter(html) {
        spans.push(Span {
            start: m.start(),
            end: m.end(),
            kind: SpanKind::Attr,
        });
    }
    for m in srcset_re.find_iter(html) {
        spans.push(Span {
            start: m.start(),
            end: m.end(),
            kind: SpanKind::Srcset,
        });
    }
    for m in style_re.find_iter(html) {
        spans.push(Span {
            start: m.start(),
            end: m.end(),
            kind: SpanKind::Style,
        });
    }
    spans.sort_by_key(|s| s.start);

    for span in &spans {
        result.push_str(&html[last_end..span.start]);
        let matched = &html[span.start..span.end];

        let eq_pos = matched.find('=').unwrap();
        let attr_prefix = matched[..eq_pos].trim_end();
        let rest = &matched[eq_pos + 1..];
        let value = &rest[1..rest.len() - 1];

        match span.kind {
            SpanKind::Srcset => {
                if let Some(nv) = rewrite_srcset(value, &base_url, &page_path, base_host, base_port)
                {
                    result.push_str(&format!("{}=\"{}\"", attr_prefix, nv));
                } else {
                    result.push_str(matched);
                }
            }
            SpanKind::Style => {
                if let Some(nv) =
                    rewrite_inline_style(value, &base_url, &page_path, base_host, base_port)
                {
                    result.push_str(&format!("{}=\"{}\"", attr_prefix, nv));
                } else {
                    result.push_str(matched);
                }
            }
            SpanKind::Attr => {
                if let Some(new_path) =
                    rewrite_url_value(value, &base_url, &page_path, base_host, base_port)
                {
                    result.push_str(&format!("{}=\"{}\"", attr_prefix, new_path));
                } else {
                    result.push_str(matched);
                }
            }
        }

        last_end = span.end;
    }

    result.push_str(&html[last_end..]);
    let result = base_tag_regex().replace_all(&result, "").to_string();
    strip_offline_breakers(result)
}

fn manifest_link_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)<link\b[^>]*?\brel\s*=\s*["']manifest["'][^>]*?>"#).unwrap()
    })
}

fn modulepreload_link_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)<link\b[^>]*?\brel\s*=\s*["']modulepreload["'][^>]*?>"#).unwrap()
    })
}

fn script_preload_link_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)<link\b[^>]*?\brel\s*=\s*["']preload["'][^>]*?\bas\s*=\s*["']script["'][^>]*?>"#).unwrap()
    })
}

fn sw_external_script_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?is)<script\b[^>]*?\bsrc\s*=\s*["'][^"']*(?:registerSW|workbox|sw-?register|sw\.js)[^"']*["'][^>]*?>\s*</script>"#,
        ).unwrap()
    })
}

fn sw_inline_script_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)<script\b[^>]*?>([\s\S]*?)</script>"#).unwrap())
}

fn crossorigin_attr_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)\s+crossorigin(?:\s*=\s*["'][^"']*["'])?"#).unwrap())
}

fn strip_offline_breakers(html: String) -> String {
    let sw_ext_re = sw_external_script_regex();
    let manifest_re = manifest_link_regex();
    let modulepreload_re = modulepreload_link_regex();
    let script_preload_re = script_preload_link_regex();
    let sw_inline_re = sw_inline_script_regex();
    let crossorigin_re = crossorigin_attr_regex();

    let after_re = sw_ext_re.replace_all(&html, "");
    let after_re = manifest_re.replace_all(&after_re, "");
    let after_re = modulepreload_re.replace_all(&after_re, "");
    let after_re = script_preload_re.replace_all(&after_re, "");

    // Remove <meta http-equiv="refresh"> tags — they bounce offline readers
    // back to the live site. Same-site targets are mirrored as pages and
    // linked normally, so dropping the tag is lossless for site-internal
    // redirects; external targets must not be followed offline.
    let after_re = meta_refresh_regex().replace_all(&after_re, "");

    let mut result = String::with_capacity(after_re.len());
    let mut last_end = 0;
    for cap in sw_inline_re.captures_iter(&after_re) {
        let m = match cap.get(0) {
            Some(m) => m,
            None => continue,
        };
        let body = cap.get(1).map(|b| b.as_str()).unwrap_or("");
        if body.contains("serviceWorker") || body.contains("registerSW") || body.contains("workbox")
        {
            result.push_str(&after_re[last_end..m.start()]);
            last_end = m.end();
        }
    }
    result.push_str(&after_re[last_end..]);

    crossorigin_re.replace_all(&result, "").to_string()
}

fn rewrite_srcset(
    srcset: &str,
    base_url: &Url,
    page_path: &str,
    base_host: &str,
    base_port: Option<u16>,
) -> Option<String> {
    let parts: Vec<&str> = srcset.split(',').collect();
    let mut rewritten_parts = Vec::new();
    let mut changed = false;

    for part in &parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut tokens = trimmed.split_whitespace();
        let url_token = match tokens.next() {
            Some(t) => t,
            None => continue,
        };
        let descriptor: String = tokens.collect::<Vec<_>>().join(" ");

        if let Some(new_path) =
            rewrite_url_value(url_token, base_url, page_path, base_host, base_port)
        {
            if !descriptor.is_empty() {
                rewritten_parts.push(format!("{} {}", new_path, descriptor));
            } else {
                rewritten_parts.push(new_path);
            }
            changed = true;
        } else if !descriptor.is_empty() {
            rewritten_parts.push(format!("{} {}", url_token, descriptor));
        } else {
            rewritten_parts.push(url_token.to_string());
        }
    }

    if changed {
        Some(rewritten_parts.join(", "))
    } else {
        None
    }
}

/// Rewrite `url(...)` references inside an inline `style="..."` attribute.
fn rewrite_inline_style(
    value: &str,
    base_url: &Url,
    page_path: &str,
    base_host: &str,
    base_port: Option<u16>,
) -> Option<String> {
    let re = css_url_regex();
    let mut changed = false;
    let mut result = String::with_capacity(value.len());

    let mut last_end = 0;
    for cap in re.captures_iter(value) {
        let m = match cap.get(0) {
            Some(m) => m,
            None => continue,
        };
        let url_text = cap.get(1).map(|u| u.as_str()).unwrap_or("");
        if url_text.starts_with("data:") {
            continue;
        }
        if let Some(new_path) =
            rewrite_url_value(url_text, base_url, page_path, base_host, base_port)
        {
            result.push_str(&value[last_end..m.start()]);
            result.push_str(&format!("url({})", new_path));
            changed = true;
            last_end = m.end();
        }
    }
    result.push_str(&value[last_end..]);

    if changed {
        Some(result)
    } else {
        None
    }
}

pub fn rewrite_css(css: &str, css_url: &Url, base_host: &str, base_port: Option<u16>) -> String {
    let css_path = {
        let (p, _) = url_to_offline_path(css_url, base_host, base_port);
        p
    };

    let url_re = css_url_regex();
    let import_re = css_import_regex();

    #[derive(Clone, Copy)]
    struct CssSpan {
        start: usize,
        end: usize,
        url_text: usize,
        url_end: usize,
    }

    let mut spans: Vec<CssSpan> = Vec::new();
    for cap in url_re.captures_iter(css) {
        if let (Some(full), Some(url_match)) = (cap.get(0), cap.get(1)) {
            spans.push(CssSpan {
                start: full.start(),
                end: full.end(),
                url_text: url_match.start(),
                url_end: url_match.end(),
            });
        }
    }
    for cap in import_re.captures_iter(css) {
        if let (Some(full), Some(url_match)) = (cap.get(0), cap.get(1)) {
            spans.push(CssSpan {
                start: full.start(),
                end: full.end(),
                url_text: url_match.start(),
                url_end: url_match.end(),
            });
        }
    }

    spans.sort_by_key(|s| s.start);
    spans.dedup_by_key(|s| s.start);

    let mut result = String::with_capacity(css.len() + 256);
    let mut last_end = 0;

    for span in &spans {
        result.push_str(&css[last_end..span.start]);
        let url_text = &css[span.url_text..span.url_end];

        if url_text.starts_with("data:") {
            result.push_str(&css[span.start..span.end]);
            last_end = span.end;
            continue;
        }

        if let Some(new_path) =
            rewrite_url_value(url_text, css_url, &css_path, base_host, base_port)
        {
            result.push_str(&format!("url(\"{new_path}\")"));
        } else {
            result.push_str(&css[span.start..span.end]);
        }
        last_end = span.end;
    }

    result.push_str(&css[last_end..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn test_relative_path_same_dir() {
        let target = Url::parse("https://example.com/images/logo.png").unwrap();
        assert_eq!(
            relative_path("index.html", &target, "example.com", None, UrlKind::Asset),
            "images/logo.png"
        );
    }

    #[test]
    fn test_relative_path_parent() {
        let target = Url::parse("https://example.com/index.html").unwrap();
        assert_eq!(
            relative_path(
                "about/index.html",
                &target,
                "example.com",
                None,
                UrlKind::Page
            ),
            "../index.html"
        );
    }

    #[test]
    fn test_rewrite_html_same_domain() {
        let page = Url::parse("https://example.com/about/").unwrap();
        let html = r#"<a href="https://example.com/">Home</a>"#.to_string();
        let rewritten = rewrite_html(&html, &page, "example.com", None);
        assert!(rewritten.contains("../index.html"));
    }

    #[test]
    fn test_rewrite_html_external_font() {
        let page = Url::parse("https://example.com/").unwrap();
        let html =
            r#"<link href="https://fonts.googleapis.com/css2?family=Roboto" rel="stylesheet">"#
                .to_string();
        let rewritten = rewrite_html(&html, &page, "example.com", None);
        assert!(rewritten.contains("_external/fonts.googleapis.com"));
    }

    #[test]
    fn test_rewrite_html_keeps_external_nav() {
        let page = Url::parse("https://example.com/").unwrap();
        let html = r#"<a href="https://other.com/page">Other</a>"#.to_string();
        let rewritten = rewrite_html(&html, &page, "example.com", None);
        assert!(rewritten.contains("https://other.com/page"));
    }

    #[test]
    fn test_rewrite_css_url() {
        let css_url = Url::parse("https://example.com/css/style.css").unwrap();
        let css = "body { background: url('/images/bg.png'); }";
        let rewritten = rewrite_css(css, &css_url, "example.com", None);
        assert!(rewritten.contains("../images/bg.png"));
    }

    #[test]
    fn test_rewrite_css_external_font() {
        let css_url = Url::parse("https://example.com/css/style.css").unwrap();
        let css = "@font-face { src: url('https://fonts.gstatic.com/s/roboto.woff2'); }";
        let rewritten = rewrite_css(css, &css_url, "example.com", None);
        assert!(rewritten.contains("_external/fonts.gstatic.com"));
    }
}
