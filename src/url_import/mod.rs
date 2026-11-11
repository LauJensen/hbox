//! Imports a frontend without regenerating its source. Capture owns remote
//! resources, relocation owns URL changes, and this module owns the on-disk
//! contract shared by import, build, update and optimization.

mod html;
mod javascript;
mod relocate;
mod source;

use std::{fs, io::ErrorKind, path::Path};

use anyhow::{Context, Result, ensure};
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::{
    ai::{DesignImportResult, GeneratedSiteFiles},
    assets::{remove_staging_dir, safe_relative_path},
    design,
};
use relocate::RelocatedPage;
use source::PageSource;

const HTML_MARKER: &str = "<!-- hbox:preserved-page -->";
const METADATA_SUFFIX: &str = ".preserved.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StyleFile {
    key: String,
    path: String,
}

/// The build contract for one page that owns its original CSS/runtime baseline.
/// CSS lives in design.css; metadata records only its block and output names.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreservedPage {
    version: u32,
    slug: String,
    source_url: String,
    styles: Vec<StyleFile>,
}

/// Captures and stages a remote frontend through the normal import result and
/// binary asset contracts. This operation needs no model or API credentials.
pub async fn prepare(
    site_dir: &Path,
    url: &Url,
    slug: &str,
    endpoint: Option<&Url>,
    staging_dir: &Path,
) -> Result<DesignImportResult> {
    let source = PageSource::fetch(url, slug).await?;
    let relocated = RelocatedPage::new(&source, slug, endpoint)?;
    let (page_html, partials) = html::templates(&format!("{HTML_MARKER}{}", relocated.html), slug)?;
    for (path, text) in partials {
        write_text(&site_dir.join(safe_relative_path(&path)?), &text)?;
    }
    for (path, text) in relocated.scripts {
        write_text(
            &site_dir
                .join("public")
                .join(safe_relative_path(path.trim_start_matches('/'))?),
            &text,
        )?;
    }
    let mut css = String::new();
    let mut styles = Vec::new();
    for style in relocated.styles {
        css.push_str(&design::block_text(&style.key, &style.content));
        styles.push(StyleFile {
            key: style.key,
            path: style.path,
        });
    }
    let metadata = PreservedPage {
        version: 1,
        slug: slug.to_owned(),
        source_url: source.url.clone(),
        styles,
    };
    write_text(
        &metadata_path(site_dir, slug),
        &serde_json::to_string_pretty(&metadata)?,
    )?;
    let assets_manifest = source.manifest();
    remove_staging_dir(staging_dir).await?;
    source.stage(&assets_manifest, staging_dir).await?;
    let mut notes = source.notes.clone();
    if source.html.contains("<form") {
        notes.push(if endpoint.is_some() {
            "The original form UI and payload are preserved. The configured endpoint must implement the frontend's API contract."
        } else {
            "The original form UI is preserved. Server routes and data are not imported; provide a compatible backend/proxy or --form-endpoint."
        }.to_owned());
    }
    Ok(DesignImportResult {
        design_spec: serde_json::to_value(&source)?,
        files: GeneratedSiteFiles {
            page_html,
            page_css: None,
            page_js: None,
            design_css: (!css.is_empty()).then_some(css),
            header_html: None,
            footer_html: None,
            assets_manifest,
        },
        notes,
    })
}

impl PreservedPage {
    /// Looks up only top-level imported pages; ordinary Hbox pages need no
    /// metadata and continue through the existing stylesheet/script pipeline.
    pub fn load(site_dir: &Path, page: &Path) -> Result<Option<Self>> {
        let relative = page.strip_prefix(site_dir.join("pages"))?;
        if relative.components().count() != 1 {
            return Ok(None);
        }
        let slug = relative
            .file_stem()
            .and_then(|name| name.to_str())
            .context("invalid page slug")?;
        read_metadata(site_dir, slug)
    }

    /// Emits each source stylesheet separately and verbatim. Keeping the
    /// original graph prevents @import, layers and order from being flattened.
    pub fn emit_styles(&self, site_dir: &Path, output_dir: &Path) -> Result<()> {
        if self.styles.is_empty() {
            return Ok(());
        }
        let css = fs::read_to_string(site_dir.join("design.css"))?;
        let owned = design::block(&css, &self.slug)?
            .context("preserved page is missing its design.css block")?;
        for style in &self.styles {
            let text = design::block(owned, &style.key)?.with_context(|| {
                format!("missing preserved stylesheet {} in design.css", style.key)
            })?;
            let relative = safe_relative_path(style.path.trim_start_matches('/'))?;
            ensure!(
                style
                    .path
                    .starts_with(&format!("/css/imports/{}/", self.slug)),
                "invalid preserved stylesheet output path"
            );
            write_text(&output_dir.join(relative), text)?;
        }
        Ok(())
    }
}

/// Removes original-page baselines from the stylesheet used by ordinary Hbox
/// pages. Imported pages load their own linked CSS instead of global.css.
pub fn native_design_css(site_dir: &Path, css: &str) -> Result<String> {
    let mut text = css.to_owned();
    for page in preserved_pages(site_dir)? {
        text = design::without_block(&text, &page.slug)?;
    }
    Ok(text)
}

/// Keeps imported style blocks unchanged when AI edits a native page's shared
/// design.css. The model sees and owns only the native portion of that file.
pub fn merge_native_design_css(site_dir: &Path, updated: &str) -> Result<String> {
    let original = match fs::read_to_string(site_dir.join("design.css")) {
        Ok(css) => css,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut output = native_design_css(site_dir, updated)?;
    for page in preserved_pages(site_dir)? {
        if let Some(css) = design::block(&original, &page.slug)? {
            output.push('\n');
            output.push_str(&design::block_text(&page.slug, css));
        }
    }
    Ok(output)
}

/// Switching a page to a screenshot import restores the normal Hbox pipeline.
pub fn forget(site_dir: &Path, slug: &str) -> Result<()> {
    match fs::remove_file(metadata_path(site_dir, slug)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Optimizing a hydrated DOM can change the tree before its runtime attaches.
/// The marker travels with the built HTML, including deploy/optimize staging.
pub fn is_preserved_html(html: &str) -> bool {
    html.trim_start().starts_with(HTML_MARKER)
}

fn preserved_pages(site_dir: &Path) -> Result<Vec<PreservedPage>> {
    let directory = site_dir.join(".hbox/imports");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut pages = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(slug) = name
            .to_str()
            .and_then(|name| name.strip_suffix(METADATA_SUFFIX))
        else {
            continue;
        };
        if let Some(page) = read_metadata(site_dir, slug)? {
            pages.push(page);
        }
    }
    pages.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(pages)
}

fn read_metadata(site_dir: &Path, slug: &str) -> Result<Option<PreservedPage>> {
    let path = metadata_path(site_dir, slug);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let page: PreservedPage = serde_json::from_str(&text)
        .with_context(|| format!("invalid preservation metadata {}", path.display()))?;
    ensure!(
        page.version == 1 && page.slug == slug,
        "unsupported or mismatched preservation metadata {}",
        path.display()
    );
    Ok(Some(page))
}

fn metadata_path(site_dir: &Path, slug: &str) -> std::path::PathBuf {
    site_dir
        .join(".hbox/imports")
        .join(format!("{slug}{METADATA_SUFFIX}"))
}

fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
        .with_context(|| format!("failed to write preserved source {}", path.display()))
}
