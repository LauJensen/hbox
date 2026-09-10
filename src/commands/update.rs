use std::{
    collections::HashSet,
    fs,
    io,
    num::NonZeroU32,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::assets::{
    generate_assets,
    install_assets,
    remove_staging_dir,
    safe_relative_path,
};

use crate::{
    ai::{chatgpt::ChatGptClient, AiConfig, AssetKind, AssetsManifest},
    config::{ResolvedSite},
    cli::UpdateDesignArgs,
    previews::remove_artifacts,
    commands::build::build_site,
    utils::{copy_all},
};

const UPDATE_SYSTEM_PROMPT: &str = include_str!("../../resources/prompts/update.txt");

#[derive(Debug, Serialize)]
struct CurrentFiles<'a> {
    page_html: &'a str,
    page_css: Option<&'a str>,
    design_css: Option<&'a str>,
    global_css: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteUpdate {
    page_html: String,
    page_css: Option<String>,
    design_css: Option<String>,
    global_css: Option<String>,
    assets_manifest: AssetsManifest,
}

struct SourceFiles {
    page_html: String,
    page_css: Option<String>,
    design_css: Option<String>,
    global_css: String,
}

/// Generates an updated copy of a site and builds it as a separately named preview.
pub async fn run(args: UpdateDesignArgs) -> Result<()> {
    validate_slug(&args.slug)?;

    let site = ResolvedSite::resolve(&args.site_name)?;
    let source_site = site.source_dir();
    ensure_site_exists(&source_site)?;

    let source = read_source_files(&source_site, &args.slug)?;
    let llm_prompt = update_prompt(&args.prompt, &source)?;

    let ai_config = AiConfig::from_env(args.threads)?;
    let client = ChatGptClient::new(ai_config)
        .context("Failed to initialize OpenAI client")?;

    println!("Generating update for {}...", args.slug);
    let update: SiteUpdate = client
        .structured_response(
            UPDATE_SYSTEM_PROMPT,
            &llm_prompt,
            "hbox_site_update",
            update_schema(),
        )
        .await?;

    validate_update(&update)?;
    cache_prompt(&source_site, &args.slug, &args.prompt)?;

    let resolved_source_site = ResolvedSite::resolve(&args.site_name)?;

    let (_preview_name, preview_site, preview_num) =
        create_preview_site_dir(&resolved_source_site)?;
    let result = create_and_build_preview(
        &client,
        &source_site,
        &preview_site,
        &args.slug,
        &update,
    )
    .await;

    if let Err(error) = result {
        remove_artifacts(&preview_site)?;
        return Err(error);
    }

    let site_name = &args.site_name.display();
    println!();
    println!("Update successfully generated.");
    println!();
    println!("To preview and/or accept use the following:");
    println!("- hbox preview {site_name} {preview_num}");
    println!("- hbox accept {site_name} {preview_num}");

    Ok(())
}

/// Copies the source site, applies the update, installs assets, and builds the preview.
async fn create_and_build_preview(
    client: &ChatGptClient,
    source_site: &Path,
    preview_site: &ResolvedSite,
    slug: &str,
    update: &SiteUpdate,
) -> Result<()> {
    copy_all(source_site, &preview_site.source_dir())?;
    apply_updated_files(&preview_site.source_dir(), slug, update)?;

    let staged_assets =
        preview_site.source_dir().join(".hbox/update-assets-tmp");

    generate_assets(
        client,
        &update.assets_manifest,
        &staged_assets,
    )
        .await?;

    install_assets(
        &preview_site.source_dir(),
        &staged_assets,
        &update.assets_manifest,
    )?;

    remove_staging_dir(&staged_assets).await?;

    build_site(preview_site).with_context(|| {
        format!("Failed to build preview site {}", &preview_site)
    })?;

    Ok(())
}

fn ensure_site_exists(site: &Path) -> Result<()> {
    if !site.is_dir() {
        bail!("Site does not exist: {}", site.display());
    }
    Ok(())
}

fn read_source_files(site: &Path, slug: &str) -> Result<SourceFiles> {
    Ok(SourceFiles {
        page_html: read_required(&site.join("pages").join(format!("{slug}.html")))?,
        page_css: read_optional(&site.join("pages").join(format!("{slug}.css")))?,
        design_css: read_optional(&site.join("design.css"))?,
        global_css: read_required(&site.join("global.css"))?,
    })
}

fn read_required(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Failed to read {}", path.display())),
    }
}

fn update_prompt(user_prompt: &str, source: &SourceFiles) -> Result<String> {
    let files = CurrentFiles {
        page_html: &source.page_html,
        page_css: source.page_css.as_deref(),
        design_css: source.design_css.as_deref(),
        global_css: &source.global_css,
    };

    Ok(format!(
        concat!(
            "Requested change:\n{}\n\n",
            "Current files:\n{}\n\n",
            "Return the complete updated page_html. For page_css, design_css, ",
            "and global_css, return the complete replacement only when that file ",
            "must change; otherwise return null. Return only newly created or ",
            "replaced assets in assets_manifest."
        ),
        user_prompt.trim(),
        serde_json::to_string_pretty(&files)?
    ))
}

fn validate_update(update: &SiteUpdate) -> Result<()> {
    if update.page_html.trim().is_empty() || !update.page_html.contains("<html") {
        bail!("updated page_html is empty or does not contain an html element");
    }

    for (name, contents) in [
        ("page_css", update.page_css.as_deref()),
        ("design_css", update.design_css.as_deref()),
        ("global_css", update.global_css.as_deref()),
    ] {
        if contents.is_some_and(|contents| contents.trim().is_empty()) {
            bail!("{name} must be null or a non-empty string");
        }
    }

    let mut filenames = HashSet::new();
    let mut paths = HashSet::new();

    for asset in &update.assets_manifest.assets {
        safe_relative_path(&asset.filename)
            .with_context(|| format!("Unsafe asset filename: {}", asset.filename))?;

        let relative = safe_relative_path(asset.path.trim_start_matches('/'))
            .with_context(|| format!("Unsafe asset path: {}", asset.path))?;
        if relative.components().next().and_then(normal_component) != Some("images") {
            bail!("Asset path must begin with /images/: {}", asset.path);
        }

        if !filenames.insert(asset.filename.clone()) {
            bail!("Duplicate asset filename: {}", asset.filename);
        }
        if !paths.insert(asset.path.clone()) {
            bail!("Duplicate asset path: {}", asset.path);
        }

        match &asset.kind {
            AssetKind::Svg if asset.svg_code.trim().is_empty() => {
                bail!("SVG asset has no svg_code: {}", asset.filename)
            }
            AssetKind::Image if asset.generation_prompt.trim().is_empty() => {
                bail!("Image asset has no generation_prompt: {}", asset.filename)
            }
            _ => {}
        }
    }

    Ok(())
}

/// Creates the first available sibling directory named `<site>-previewN`.
fn create_preview_site_dir(
    source_site: &ResolvedSite,
) -> Result<(String, ResolvedSite, NonZeroU32)> {
    let parent = source_site
        .source_dir()
        .parent()
        .with_context(|| {
            format!(
                "Site has no parent directory: {source_site}"
            )
        })?;

    for number in 1u32.. {
        let number = NonZeroU32::new(number).unwrap();

        let preview_name = &source_site.preview_name(number);

        let preview_source_dir =
            parent.join(&preview_name);

        let preview_site =
            ResolvedSite::resolve(&preview_source_dir)?;

        match fs::create_dir(&preview_site.source_dir()) {
            Ok(()) => {
                return Ok((preview_name.to_owned(), preview_site, number));
            }

            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists =>
            {
                continue;
            }

            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to create preview site {preview_site}"
                    )
                });
            }
        }
    }

    unreachable!("the preview number range is non-empty")
}

fn apply_updated_files(preview_site: &Path, slug: &str, update: &SiteUpdate) -> Result<()> {
    write_file(
        &preview_site.join("pages").join(format!("{slug}.html")),
        &update.page_html,
    )?;
    write_optional_file(
        &preview_site.join("pages").join(format!("{slug}.css")),
        update.page_css.as_deref(),
    )?;
    write_optional_file(
        &preview_site.join("design.css"),
        update.design_css.as_deref(),
    )?;
    write_optional_file(
        &preview_site.join("global.css"),
        update.global_css.as_deref(),
    )?;

    Ok(())
}

/// Stores only the user's update request in the update cache.
fn cache_prompt(site: &Path, slug: &str, prompt: &str) -> Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before the Unix epoch")?
        .as_millis();
    let cache_dir = site
        .join(".hbox/updates")
        .join(slug)
        .join(timestamp.to_string());
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Failed to create update cache {}", cache_dir.display()))?;
    write_file(&cache_dir.join("prompt.txt"), prompt)?;
    Ok(cache_dir)
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("Failed to write {}", path.display()))
}

/// Writes a replacement only when the model returned one; `None` means unchanged.
fn write_optional_file(path: &Path, contents: Option<&str>) -> Result<()> {
    if let Some(contents) = contents {
        write_file(path, contents)?;
    }
    Ok(())
}

fn validate_slug(slug: &str) -> Result<()> {
    let path = safe_relative_path(slug)?;
    if path.components().count() != 1 || slug.starts_with('.') {
        bail!("Page name must be a single, non-hidden path component");
    }
    Ok(())
}

fn normal_component<'a>(component: Component<'a>) -> Option<&'a str> {
    match component {
        Component::Normal(value) => value.to_str(),
        _ => None,
    }
}

fn update_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "page_html": { "type": "string" },
            "page_css": { "type": ["string", "null"] },
            "design_css": { "type": ["string", "null"] },
            "global_css": { "type": ["string", "null"] },
            "assets_manifest": {
                "type": "object",
                "properties": {
                    "assets": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "filename": { "type": "string" },
                                "path": { "type": "string" },
                                "kind": {
                                    "type": "string",
                                    "enum": ["image", "svg", "css_generated"]
                                },
                                "description": { "type": "string" },
                                "generation_prompt": { "type": "string" },
                                "size": {
                                    "type": ["string", "null"],
                                    "enum": [
                                        "1024x1024",
                                        "1024x1536",
                                        "1536x1024",
                                        "auto",
                                        null
                                    ]
                                },
                                "svg_code": { "type": "string" }
                            },
                            "required": [
                                "filename",
                                "path",
                                "kind",
                                "description",
                                "generation_prompt",
                                "size",
                                "svg_code"
                            ],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["assets"],
                "additionalProperties": false
            }
        },
        "required": [
            "page_html",
            "page_css",
            "design_css",
            "global_css",
            "assets_manifest"
        ],
        "additionalProperties": false
    })
}
