use std::{
    io::ErrorKind,
    path::Path,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};

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
    },
    cli::ImportDesignArgs,
    commands::build::build_site,
    config::ResolvedSite,
    dev_print,
    previews,
};

const DESIGN_BLOCK_START_PREFIX: &str = "/* hbox design import:start ";
const DESIGN_BLOCK_END_PREFIX: &str = "/* hbox design import:end ";

pub async fn run(args: ImportDesignArgs) -> Result<()> {
    let skip_api_call = false;
    let skip_asset_gen = false;

    let site = ResolvedSite::resolve(&args.site_name)?;

    if !site.is_initialized() {
        bail!(
            "Site is not initialized. Run:\n\n    hbox init {}\n",
            site.site_name(),
        );
    }

    // Do everything that can fail without creating a preview first.
    let page_slug = validate_page_slug(&args.slug)?;
    let config = AiConfig::from_env(args.threads)?;
    let client = ChatGptClient::new(config)?;

    let preview_site = previews::create(&site)?;

    let import_result = import_into_preview(
        &preview_site,
        &args.screenshot_path,
        &page_slug,
        &client,
        skip_api_call,
        skip_asset_gen,
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
    screenshot_path: &Path,
    page_slug: &str,
    client: &ChatGptClient,
    skip_api_call: bool,
    skip_asset_gen: bool,
) -> Result<()> {
    let partial_status = PartialStatus::inspect(preview_site.source_dir());
    let spinner = import_spinner();

    let cached_result_path = preview_site
        .source_dir()
        .join(".hbox")
        .join("imports")
        .join(format!("{page_slug}.import-result.json"));

    // Capture the result before applying `?` so the spinner is always cleared.
    let write_result: Result<DesignImportResult> = async {
        let result = if skip_api_call {
            spinner.set_message("Reading cached import result");
            read_design_import_result(&cached_result_path).await?
        } else {
            spinner.set_message(format!(
                "Analyzing screenshot: {}",
                screenshot_path.display()
            ));

            client
                .import_design(screenshot_path, &partial_status, &spinner)
                .await
                .context("failed to import design")?
        };

        validate_import_result(&result)?;

        if !skip_api_call {
            write_design_import_cache(&cached_result_path, &result).await?;
        }

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

    if !skip_asset_gen {
        let staging_dir = preview_site
            .source_dir()
            .join(".hbox/import-assets-tmp");

        generate_assets(
            client,
            &result.files.assets_manifest,
            &staging_dir,
        )
        .await?;

        install_assets(
            preview_site.source_dir(),
            &staging_dir,
            &result.files.assets_manifest,
        )?;

        remove_staging_dir(&staging_dir).await?;
    }

    build_site(preview_site).with_context(|| {
        format!("failed to build imported preview {preview_site}")
    })?;

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

    write_optional_page_css(
        &page_css_path,
        result.files.page_css.as_deref(),
    )
    .await?;

    upsert_design_css_block(
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

async fn write_optional_page_css(
    path: &Path,
    css: Option<&str>,
) -> Result<()> {
    match non_empty(css) {
        Some(css) => write_required_text(path, css, "generated page CSS").await,
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

/// Inserts or replaces only the block owned by this imported page.
///
/// Everything outside the markers remains developer-owned and untouched. If a
/// regenerated import no longer has reusable design CSS, its previous block is
/// removed.
async fn upsert_design_css_block(
    path: &Path,
    page_slug: &str,
    css: Option<&str>,
) -> Result<()> {
    let existing = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read design stylesheet {}", path.display())
            });
        }
    };

    let start_marker = format!("{DESIGN_BLOCK_START_PREFIX}{page_slug} */");
    let end_marker = format!("{DESIGN_BLOCK_END_PREFIX}{page_slug} */");

    let without_previous =
        remove_marked_block(&existing, &start_marker, &end_marker)?;

    let updated = match non_empty(css) {
        Some(css) => {
            let mut output = without_previous.trim_end().to_string();

            if !output.is_empty() {
                output.push_str("\n\n");
            }

            output.push_str(&start_marker);
            output.push('\n');
            output.push_str(css.trim());
            output.push('\n');
            output.push_str(&end_marker);
            output.push('\n');

            output
        }
        None => {
            let trimmed = without_previous.trim_end();

            if trimmed.is_empty() {
                String::new()
            } else {
                format!("{trimmed}\n")
            }
        }
    };

    tokio::fs::write(path, updated)
        .await
        .with_context(|| {
            format!("failed to write design stylesheet {}", path.display())
        })?;

    Ok(())
}

fn remove_marked_block(
    input: &str,
    start_marker: &str,
    end_marker: &str,
) -> Result<String> {
    let Some(start) = input.find(start_marker) else {
        if input.contains(end_marker) {
            bail!(
                "found design CSS end marker without start marker: {}",
                end_marker
            );
        }

        return Ok(input.to_string());
    };

    let search_from = start + start_marker.len();

    let relative_end = input[search_from..]
        .find(end_marker)
        .with_context(|| {
            format!(
                "found design CSS start marker without end marker: {}",
                start_marker
            )
        })?;

    let end = search_from + relative_end + end_marker.len();

    if input[end..].contains(start_marker) {
        bail!(
            "multiple design CSS blocks found for the same page: {}",
            start_marker
        );
    }

    let mut output = String::with_capacity(input.len());
    output.push_str(&input[..start]);
    output.push_str(input[end..].trim_start_matches(['\r', '\n']));

    Ok(output)
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

fn validate_import_result(result: &DesignImportResult) -> Result<()> {
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

    Ok(())
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
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

async fn read_design_import_result(
    path: &Path,
) -> Result<DesignImportResult> {
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| {
            format!("failed to read cached import result {}", path.display())
        })?;

    serde_json::from_str(&text).with_context(|| {
        format!("failed to parse cached import result {}", path.display())
    })
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
