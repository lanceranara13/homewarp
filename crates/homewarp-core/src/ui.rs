//! The web interface, built from `web/` and carried inside this binary.

use axum::{
    Json,
    http::{
        HeaderValue, StatusCode, Uri,
        header::{
            CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY,
            X_CONTENT_TYPE_OPTIONS,
        },
    },
    response::{IntoResponse, Response},
};
use rust_embed::{EmbeddedFile, RustEmbed};

/// `web/dist` as it was when this binary was compiled. A build without the web
/// interface (a `cargo check`, a test run) simply has no files.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web/dist"]
#[allow_missing = true]
struct Built;

/// Everything comes from this server: no third-party scripts, styles, fonts or
/// frames, and no page may frame this one.
const PAGE_POLICY: &str = "default-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// Serves a built file, or the app's page for any path that is not one: the
/// app does its own routing in the browser.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        let body = serde_json::json!({ "error": "There is no such endpoint." });
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    }
    match (Built::get(path), Built::get("index.html")) {
        // The bundler puts a content hash in these names, so a file never changes.
        (Some(file), _) if path.starts_with("assets/") => {
            respond(file, "public, max-age=31536000, immutable")
        }
        (Some(file), _) => respond(file, "no-cache"),
        (None, Some(page)) => respond(page, "no-cache"),
        (None, None) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "This build of Homewarp has no web interface in it.\n",
        )
            .into_response(),
    }
}

fn respond(file: EmbeddedFile, caching: &'static str) -> Response {
    let headers = [
        (
            CONTENT_TYPE,
            HeaderValue::from_str(file.metadata.mimetype())
                .unwrap_or(HeaderValue::from_static("application/octet-stream")),
        ),
        (CACHE_CONTROL, HeaderValue::from_static(caching)),
        (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        (REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
        (
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(PAGE_POLICY),
        ),
    ];
    (headers, file.data).into_response()
}
