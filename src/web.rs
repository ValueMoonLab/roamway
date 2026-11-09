pub const INDEX: &str = include_str!("web/index.html");
pub const APP_JS: &str = include_str!("web/app.js");
pub const STYLE_CSS: &str = include_str!("web/style.css");

pub const CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'";

/// Map a `/ui/...` path suffix (empty means the index page) to
/// (content_type, body). Unknown files return `None`.
pub fn asset(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "" | "index.html" => Some(("text/html; charset=utf-8", INDEX)),
        "app.js" => Some(("application/javascript; charset=utf-8", APP_JS)),
        "style.css" => Some(("text/css; charset=utf-8", STYLE_CSS)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_are_non_empty() {
        assert!(!INDEX.trim().is_empty());
        assert!(!APP_JS.trim().is_empty());
        assert!(!STYLE_CSS.trim().is_empty());
    }

    #[test]
    fn asset_routes_content_types() {
        assert_eq!(asset("").map(|(ct, _)| ct), Some("text/html; charset=utf-8"));
        assert_eq!(asset("index.html").map(|(ct, _)| ct), Some("text/html; charset=utf-8"));
        assert_eq!(asset("app.js").map(|(ct, _)| ct), Some("application/javascript; charset=utf-8"));
        assert_eq!(asset("style.css").map(|(ct, _)| ct), Some("text/css; charset=utf-8"));
        assert!(asset("missing.js").is_none());
    }
}
