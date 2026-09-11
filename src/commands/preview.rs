use std::{
    net::SocketAddr,
    path::PathBuf,
};

use crate::watch::watch_and_rebuild;

use reqwest::StatusCode;
use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{OriginalUri, State},
    http::{header, HeaderValue, Response, HeaderMap},
    response::{IntoResponse},
    routing::{get, post},
    Router,
};
use tokio::net::TcpListener;

use crate::{cli::PreviewArgs};

use crate::{
    config::{ResolvedSite},
};

const EDITOR_JS: &str = include_str!("../../resources/editor.js");

#[derive(Clone)]
struct AppState {
    output_dir: PathBuf,
}

pub async fn run(args: PreviewArgs) -> Result<()> {
    let site = ResolvedSite::resolve(&args.site)?;

    let site = match args.preview_index {
        Some(index) => site.preview(index),
        None        => site,
    };

    //eprintln!("Building site: {}", &args.site.display());
    //let report = build::build_site(&site)?;
    let watched_site = site.clone();

    //optimize::optimize(&site.output_dir);

    let _watcher_task = tokio::spawn({
        async move {
            watch_and_rebuild(watched_site).await
        }
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));

    let state = AppState {
        output_dir: PathBuf::from(site.output_dir()),
    };

    let app = Router::new()
        .route("/save", post(save_html))
        .route("/", get(serve_request))
        .route("/{*path}", get(serve_request))
        .with_state(state);

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| {
            format!("Failed to bind development server to http://{addr}")
        })?;

    println!("Serving {}", site.output_dir().display());
    println!("Open http://{addr}");

    axum::serve(listener, app)
        .await
        .context("Development server failed")?;

    Ok(())
}

fn inject_editor_script(html: &str) -> String {
    let script = format!(
        r#"<script class="hboxUtils" type="text/javascript">{EDITOR_JS}</script>"#
    );

    if html.contains("</body>") {
        html.replace("</body>", &format!("{script}\n</body>"))
    } else {
        format!("{html}\n{script}")
    }
}

/// TOOD: Slated for 1.1
async fn save_html(
    State(state): State<AppState>,
    headers: HeaderMap,
    html: String,
) -> Result<StatusCode, StatusCode> {
    let page_path = headers
        .get("x-hbox-path")
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;

    //let filename = request_path(page_path)?;
    let filename = html_path_from_uri(page_path)?;

    if filename.ends_with(".html") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let path = state.output_dir.join(filename);

    tokio::fs::write(path, html)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

fn html_path_from_uri(path: &str) -> Result<String, StatusCode> {
    let path = path
        .trim_start_matches('/')
        .trim_end_matches('/');

    if path.is_empty() {
        return Ok("index.html".to_string());
    }

    if path.contains('\\')
        || path.contains("..")
        || path.starts_with('/')
        || path.contains('.')
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    Ok(format!("{path}/index.html"))
}

async fn serve_request(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, StatusCode> {
    let rel_path = request_path(uri.path())?;
    let path = state.output_dir.join(&rel_path);

    if !path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    if rel_path.ends_with(".html") {
        let html = tokio::fs::read_to_string(path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let html = inject_editor_script(&html);

        return Ok(response_with_content_type(
            html,
            "text/html; charset=utf-8",
        ));
    }

    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let content_type = mime_guess::from_path(&rel_path)
        .first_or_octet_stream()
        .to_string();

    Ok(response_with_content_type(bytes, &content_type))
}

fn response_with_content_type(
    body: impl Into<Body>,
    content_type: &str,
) -> Response<Body> {
    let mut response = Response::new(body.into());

    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );

    response
}

fn request_path(path: &str) -> Result<String, StatusCode> {
    let path = path
        .trim_start_matches('/')
        .trim_end_matches('/');

    if path.is_empty() {
        return Ok("index.html".to_string());
    }

    if path.contains('\\')
        || path.contains("..")
        || path.starts_with('/')
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    if path.contains('.') {
        return Ok(path.to_string());
    }

    Ok(format!("{path}/index.html"))
}
