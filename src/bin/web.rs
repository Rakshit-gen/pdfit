//! pdfit-web: the same shrinking as the command line, as a local page.
//! Drop a PDF, see what's heavy, pick a limit, compare the pages before and
//! after, download. Files never leave the computer; the server listens on
//! 127.0.0.1 only and keeps one document in memory at a time.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use pdfit::doc::{self, ImageInfo};
use pdfit::fit;
use serde::Deserialize;
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};

#[derive(Parser)]
#[command(version, about = "Shrink a PDF to fit an upload limit, in the browser")]
struct Cli {
    /// Port on 127.0.0.1 to listen on.
    #[arg(long, default_value_t = 7474)]
    port: u16,
    /// Serve the UI from this directory instead of the copy built into the
    /// binary. Handy while working on the UI.
    #[arg(long)]
    static_dir: Option<PathBuf>,
}

struct Session {
    name: String,
    input: Arc<Vec<u8>>,
    output: Option<Vec<u8>>,
}

#[derive(Default)]
struct App {
    session: Mutex<Option<Session>>,
}

struct ApiError(StatusCode, &'static str, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "kind": self.1, "error": self.2 }))).into_response()
    }
}

impl From<doc::Error> for ApiError {
    fn from(e: doc::Error) -> Self {
        let status = match e {
            doc::Error::Write(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        ApiError(status, e.kind(), e.to_string())
    }
}

fn bad(kind: &'static str, msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, kind, msg.into())
}

type Api<T> = Result<T, ApiError>;

/// The file name from the X-Filename header, reduced to a safe base name.
fn file_name(headers: &HeaderMap) -> String {
    let raw = headers
        .get("x-filename")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("document.pdf");
    let decoded = percent_decode(raw);
    Path::new(&decoded)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "document.pdf".into())
}

/// Browsers can only send ASCII in headers, so the page percent-encodes.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read a dropped PDF and say what's in it. The bytes are kept for the fit.
async fn inspect(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    body: Bytes,
) -> Api<Json<serde_json::Value>> {
    let name = file_name(&headers);
    if body.is_empty() {
        return Err(bad("empty", "The file is empty."));
    }
    let input = Arc::new(body.to_vec());
    let bytes = input.clone();
    let summary = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, doc::Error> {
        let d = doc::load(&bytes)?;
        let inv = doc::inventory(&d);
        let skipped: HashSet<_> = inv
            .images
            .iter()
            .filter(|i| i.info.skip.is_some())
            .map(|i| i.id)
            .collect();
        let breakdown = fit::breakdown(&d, &skipped);
        let images: Vec<&ImageInfo> = inv.images.iter().map(|i| &i.info).collect();
        Ok(json!({
            "bytes": bytes.len(),
            "pages": inv.pages,
            "inline_images": inv.inline_images,
            "images": images,
            "breakdown": breakdown,
        }))
    })
    .await
    .map_err(|e| bad("broken", e.to_string()))??;
    *app.session.lock().unwrap() = Some(Session {
        name: name.clone(),
        input,
        output: None,
    });
    let mut summary = summary;
    summary["name"] = name.into();
    Ok(Json(summary))
}

#[derive(Deserialize)]
struct FitRequest {
    limit: usize,
}

async fn fit_now(
    State(app): State<Arc<App>>,
    Json(req): Json<FitRequest>,
) -> Api<Json<serde_json::Value>> {
    if req.limit < 1000 {
        return Err(bad("limit", "That limit is too small to hold any PDF."));
    }
    let input = app
        .session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.input.clone())
        .ok_or_else(|| bad("no_file", "Drop a PDF first."))?;
    let outcome = tokio::task::spawn_blocking(move || fit::fit(&input, req.limit))
        .await
        .map_err(|e| bad("broken", e.to_string()))??;
    let report = serde_json::to_value(&outcome).expect("plain data");
    if let Some(s) = app.session.lock().unwrap().as_mut() {
        s.output = Some(outcome.output);
    }
    Ok(Json(report))
}

/// The latest result, as a download named after the input.
async fn result(State(app): State<Arc<App>>) -> Api<Response> {
    let guard = app.session.lock().unwrap();
    let s = guard
        .as_ref()
        .ok_or_else(|| bad("no_file", "Drop a PDF first."))?;
    let out = s
        .output
        .clone()
        .ok_or_else(|| bad("no_result", "Shrink the file first."))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (header::CONTENT_DISPOSITION, attachment(&s.name)),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        out,
    )
        .into_response())
}

/// `attachment; filename=...` for NAME.small.pdf, with an ASCII fallback
/// and the real name in RFC 5987 form.
fn attachment(input: &str) -> String {
    let stem = Path::new(input)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".into());
    let name = format!("{stem}.small.pdf");
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist"]
#[allow_missing = true]
struct Ui;

async fn embedded_ui(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (file, path) = match Ui::get(path) {
        Some(f) if !path.is_empty() => (f, path),
        _ => match Ui::get("index.html") {
            Some(f) => (f, "index.html"),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    "This build has no UI. Run `pnpm --dir web build` and rebuild, or pass --static-dir.",
                )
                    .into_response();
            }
        },
    };
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
            (header::CACHE_CONTROL, cache.to_string()),
        ],
        file.data,
    )
        .into_response()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let app = Router::new()
        .route("/api/inspect", post(inspect))
        .route("/api/fit", post(fit_now))
        .route("/api/result", get(result))
        // Scans run to hundreds of MB; the server only listens locally.
        .layer(axum::extract::DefaultBodyLimit::max(1 << 30))
        .with_state(Arc::new(App::default()));
    let app = match &cli.static_dir {
        Some(dir) => app
            .fallback_service(ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")))),
        None => app.fallback(embedded_ui),
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], cli.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("pdfit is running at http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_names_are_reduced_to_a_base_name() {
        let mut h = HeaderMap::new();
        h.insert("x-filename", "..%2F..%2Fetc%2Fpasswd".parse().unwrap());
        assert_eq!(file_name(&h), "passwd");
        h.insert(
            "x-filename",
            "Lebenslauf%20M%C3%BCller.pdf".parse().unwrap(),
        );
        assert_eq!(file_name(&h), "Lebenslauf Müller.pdf");
        assert_eq!(file_name(&HeaderMap::new()), "document.pdf");
    }

    #[test]
    fn downloads_are_named_after_the_input() {
        assert_eq!(
            attachment("visa scan.pdf"),
            "attachment; filename=\"visa scan.small.pdf\"; filename*=UTF-8''visa%20scan.small.pdf"
        );
        assert!(
            attachment("Müller \"cv\".pdf")
                .starts_with("attachment; filename=\"M_ller _cv_.small.pdf\"")
        );
    }
}
