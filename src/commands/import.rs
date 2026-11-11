use std::{
    io::ErrorKind,
    path::Path,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Url;

use crate::{
    ai::{
        chatgpt::ChatGptClient,
        AiConfig,
        DesignImportResult,
        PartialStatus,
    },
    assets::{
        generate_assets,
        install_assets,
        remove_staging_dir,
        validate_asset_references,
        validate_manifest,
    },
    cli::ImportDesignArgs,
    commands::build::build_site,
    config::ResolvedSite,
    previews,
    url_import,
};

/// Imports into a preview using capture for URLs or model generation for images.
pub async fn run(args: ImportDesignArgs) -> Result<()> {
    let site = ResolvedSite::resolve(&args.site_name)?;

    if !site.is_initialized() {
        bail!(
            "Site is not initialized. Run:\n\n    hbox init {}\n",
            site.site_name(),
        );
    }

    // Do everything that can fail without creating a preview first.
    let page_slug = validate_page_slug(&args.slug)?;
    let source_url = parse_source_url(&args.source)?;
    if args.form_endpoint.is_some() && source_url.is_none() {
        bail!("--form-endpoint applies only to URL imports");
    }
    let client = if source_url.is_none() {
        Some(ChatGptClient::new(AiConfig::from_env(args.threads)?)?)
    } else {
        None
    };

    let preview_site = previews::create(&site)?;

    let import_result = import_into_preview(
        &preview_site,
        &args,
        source_url.as_ref(),
        &page_slug,
        client.as_ref(),
    )
    .await;

    if let Err(import_error) = import_result {
        return match previews::remove_artifacts(&preview_site) {
            Ok(()) => Err(import_error),
            Err(cleanup_error) => Err(import_error.context(format!(
                "import into {} failed, and cleanup also failed: {cleanup_error:#}",
                preview_site,
            ))),
        };
    }

    println!(
        "Imported design into {}",
        preview_site.source_dir().display()
    );

    Ok(())
}

async fn import_into_preview(
    preview_site: &ResolvedSite,
    args: &ImportDesignArgs,
    source_url: Option<&Url>,
    page_slug: &str,
    client: Option<&ChatGptClient>,
) -> Result<()> {
    let partial_status = PartialStatus::inspect(preview_site.source_dir());
    let spinner = import_spinner();
    let staging_dir = preview_site.source_dir().join(".hbox/import-assets-tmp");

    let cached_result_path = preview_site
        .source_dir()
        .join(".hbox")
        .join("imports")
        .join(format!("{page_slug}.import-result.json"));

    // Capture the result before applying `?` so the spinner is always cleared.
    let write_result: Result<DesignImportResult> = async {
        let result = if let Some(url) = source_url {
            spinner.set_message(format!("Preserving frontend from {url}"));
            url_import::prepare(
                preview_site.source_dir(), url, page_slug,
                args.form_endpoint.as_ref(), &staging_dir,
            ).await.context("failed to import remote page")?
        } else {
            spinner.set_message(format!(
                "Analyzing screenshot: {}",
                args.source.display()
            ));

            url_import::forget(preview_site.source_dir(), page_slug)?;
            client
                .context("screenshot import requires an AI client")?
                .import_design(&args.source, &partial_status, &spinner)
                .await
                .context("failed to import design")?
        };

        validate_import_result(&result, source_url.is_some())?;
        write_design_import_cache(&cached_result_path, &result).await?;

        spinner.set_message("Writing imported page");

        write_design_import_result(
            preview_site.source_dir(),
            page_slug,
            &result,
        )
        .await?;

        Ok(result)
    }
    .await;

    spinner.finish_and_clear();
    let result = write_result?;

    if source_url.is_none() {
        generate_assets(
            client.context("screenshot import requires an AI client")?,
            &result.files.assets_manifest,
            &staging_dir,
        ).await?;
    }
    install_assets(
        preview_site.source_dir(),
        &staging_dir,
        &result.files.assets_manifest,
    )?;
    remove_staging_dir(&staging_dir).await?;

    build_site(preview_site).with_context(|| {
        format!("failed to build imported preview {preview_site}")
    })?;

    for note in &result.notes { println!("Import note: {note}"); }

    Ok(())
}

fn import_spinner() -> ProgressBar {
    let spinner = ProgressBar::new_spinner();

    spinner.set_style(
        ProgressStyle::with_template("{spinner:.gold} {msg}")
            .expect("valid spinner template")
            .tick_strings(&[
                "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏",
            ]),
    );

    spinner.enable_steady_tick(Duration::from_millis(150));
    spinner
}

async fn write_design_import_result(
    site_dir: &Path,
    page_slug: &str,
    result: &DesignImportResult,
) -> Result<()> {
    let pages_dir = site_dir.join("pages");
    let partials_dir = site_dir.join("partials");
    let public_images_dir = site_dir.join("public").join("images");
    let imports_dir = site_dir.join(".hbox").join("imports");

    for directory in [
        &pages_dir,
        &partials_dir,
        &public_images_dir,
        &imports_dir,
    ] {
        tokio::fs::create_dir_all(directory)
            .await
            .with_context(|| {
                format!("failed to create directory {}", directory.display())
            })?;
    }

    let page_path = pages_dir.join(format!("{page_slug}.html"));
    let page_css_path = pages_dir.join(format!("{page_slug}.css"));
    let page_js_path = pages_dir.join(format!("{page_slug}.js"));
    let design_css_path = site_dir.join("design.css");

    let design_spec_path =
        imports_dir.join(format!("{page_slug}.design-spec.json"));

    let assets_manifest_path =
        imports_dir.join(format!("{page_slug}.assets-manifest.json"));

    write_optional_partial(
        &partials_dir.join("header.html"),
        result.files.header_html.as_deref(),
    )
    .await?;

    write_optional_partial(
        &partials_dir.join("footer.html"),
        result.files.footer_html.as_deref(),
    )
    .await?;

    write_required_text(
        &page_path,
        &result.files.page_html,
        "generated page HTML",
    )
    .await?;

    write_optional_page_file(
        &page_css_path,
        result.files.page_css.as_deref(),
        "generated page CSS",
    )
    .await?;

    write_optional_page_file(
        &page_js_path,
        result.files.page_js.as_deref(),
        "generated page JavaScript",
    )
    .await?;

    crate::design::upsert_block(
        &design_css_path,
        page_slug,
        result.files.design_css.as_deref(),
    )
    .await?;

    write_pretty_json(&design_spec_path, &result.design_spec)
        .await
        .with_context(|| {
            format!(
                "failed to write design spec {}",
                design_spec_path.display()
            )
        })?;

    write_pretty_json(
        &assets_manifest_path,
        &result.files.assets_manifest,
    )
    .await
    .with_context(|| {
        format!(
            "failed to write assets manifest {}",
            assets_manifest_path.display()
        )
    })?;

    Ok(())
}

async fn write_optional_partial(
    path: &Path,
    content: Option<&str>,
) -> Result<()> {
    let Some(content) = non_empty(content) else {
        return Ok(());
    };

    if path.exists() {
        bail!(
            "refusing to overwrite existing shared partial: {}",
            path.display()
        );
    }

    write_required_text(path, content, "generated partial").await
}

/// Replaces a page companion file, removing an old version when none is returned.
async fn write_optional_page_file(
    path: &Path,
    content: Option<&str>,
    description: &str,
) -> Result<()> {
    match non_empty(content) {
        Some(content) => write_required_text(path, content, description).await,
        None => remove_file_if_exists(path).await,
    }
}

async fn remove_file_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to remove stale file {}", path.display())),
    }
}

async fn write_required_text(
    path: &Path,
    content: &str,
    description: &str,
) -> Result<()> {
    let content = content.trim();

    if content.is_empty() {
        bail!("{description} is empty");
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| {
                format!("failed to create directory {}", parent.display())
            })?;
    }

    tokio::fs::write(path, format!("{content}\n"))
        .await
        .with_context(|| {
            format!("failed to write {description} {}", path.display())
        })?;

    Ok(())
}

async fn write_pretty_json<T>(path: &Path, value: &T) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let json = serde_json::to_string_pretty(value)?;

    tokio::fs::write(path, format!("{json}\n"))
        .await
        .with_context(|| format!("failed to write JSON file {}", path.display()))
}

fn validate_import_result(result: &DesignImportResult, preserved: bool) -> Result<()> {
    let files = &result.files;

    if files.page_html.trim().is_empty() {
        bail!("generated page_html is empty");
    }

    if !files.page_html.contains("<html") {
        bail!("generated page_html does not contain an <html> element");
    }

    if let Some(header) = files.header_html.as_deref() {
        if header.trim().is_empty() {
            bail!("header_html must be null or a non-empty string");
        }
    }

    if let Some(footer) = files.footer_html.as_deref() {
        if footer.trim().is_empty() {
            bail!("footer_html must be null or a non-empty string");
        }
    }

    if let Some(page_css) = files.page_css.as_deref() {
        if page_css.trim().is_empty() {
            bail!("page_css must be null or a non-empty string");
        }
    }

    if let Some(design_css) = files.design_css.as_deref() {
        if design_css.trim().is_empty() {
            bail!("design_css must be null or a non-empty string");
        }
    }

    validate_manifest(&files.assets_manifest)
        .context("invalid imported assets manifest")?;

    // The preservation pipeline owns its complete resource graph, including
    // binaries referenced only from external modules rather than page fields.
    if preserved {
        return Ok(());
    }

    validate_asset_references(
        &files.assets_manifest,
        [
            files.page_html.as_str(),
            files.page_css.as_deref().unwrap_or_default(),
            files.design_css.as_deref().unwrap_or_default(),
            files.page_js.as_deref().unwrap_or_default(),
            files.header_html.as_deref().unwrap_or_default(),
            files.footer_html.as_deref().unwrap_or_default(),
        ],
    )
    .context("OpenAI returned invalid asset references")?;

    Ok(())
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// URLs use the existing positional source argument; local screenshot paths keep
/// their original PathBuf handling, including non-UTF-8 filesystem paths.
fn parse_source_url(source: &Path) -> Result<Option<Url>> {
    let Some(value) = source.to_str() else { return Ok(None); };
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") || value.contains("://") {
        let url = Url::parse(value).context("invalid import URL")?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("URL imports support only HTTP(S)");
        }
        Ok(Some(url))
    } else {
        Ok(None)
    }
}

fn validate_page_slug(slug: &str) -> Result<String> {
    let slug = slug.trim();

    if slug.is_empty() {
        bail!("page slug cannot be empty");
    }

    if slug.ends_with(".html") {
        bail!(
            "page slug should not include .html. Use '--slug about', not '--slug about.html'"
        );
    }

    let valid = slug.bytes().all(|byte| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || byte == b'-'
            || byte == b'_'
    });

    if !valid {
        bail!(
            "invalid page slug '{}'. Use lowercase ASCII letters, numbers, '-' or '_'",
            slug
        );
    }

    Ok(slug.to_string())
}

async fn write_design_import_cache(
    path: &Path,
    result: &DesignImportResult,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| {
                format!(
                    "failed to create cache directory {}",
                    parent.display()
                )
            })?;
    }

    dev_print!("Updating cache: {}", path.display());

    write_pretty_json(path, result)
        .await
        .with_context(|| {
            format!("failed to write cached import result {}", path.display())
        })
}
