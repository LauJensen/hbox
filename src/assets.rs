use std::{
    collections::HashSet,
    io,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};

use crate::ai::{
    chatgpt::ChatGptClient,
    AssetKind,
    AssetsManifest,
};

/// Generates all currently supported file-backed assets in a clean staging
/// directory.
pub async fn generate_assets(
    client: &ChatGptClient,
    manifest: &AssetsManifest,
    staging_dir: &Path,
) -> Result<()> {
    validate_manifest(manifest)?;
    reset_staging_dir(staging_dir).await?;

    client
        .generate_asset_images(manifest, staging_dir)
        .await
        .context("failed to generate image assets")?;

    for asset in &manifest.assets {
        if asset.kind != AssetKind::Svg {
            continue;
        }

        let relative = safe_relative_path(&asset.filename)?;
        let destination = staging_dir.join(relative);

        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| {
                    format!(
                        "failed to create SVG staging directory {}",
                        parent.display()
                    )
                })?;
        }

        tokio::fs::write(&destination, asset.svg_code.trim())
            .await
            .with_context(|| {
                format!(
                    "failed to write staged SVG {}",
                    destination.display()
                )
            })?;
    }

    Ok(())
}

/// Installs every staged file-backed asset beneath the site's `public/`
/// directory. CSS-generated entries do not represent files and are skipped.
pub fn install_assets(
    site_dir: &Path,
    staging_dir: &Path,
    manifest: &AssetsManifest,
) -> Result<()> {
    validate_manifest(manifest)
        .context("refusing to install an invalid asset manifest")?;

    let mut installed_paths = HashSet::new();

    for asset in &manifest.assets {
        let Some(public_path) = asset.public_relative_path() else {
            continue;
        };

        // Validate before using either derived path in a filesystem join: an
        // absolute filename would otherwise replace the intended prefix.
        let filename = safe_relative_path(&asset.filename)
            .with_context(|| {
                format!(
                    "invalid asset filename '{}'",
                    asset.filename
                )
            })?;

        if !installed_paths.insert(public_path.clone()) {
            let public_url = asset
                .public_url()
                .context("file-backed asset has no public URL")?;

            bail!(
                "multiple assets resolve to public path {public_url}"
            );
        }

        let source = staging_dir.join(filename);

        if !source.is_file() {
            bail!(
                "staged asset does not exist: {}",
                source.display()
            );
        }

        let destination = site_dir
            .join("public")
            .join(public_path);

        copy_file(&source, &destination)?;
    }

    Ok(())
}

pub fn manifest_problems(manifest: &AssetsManifest) -> Vec<String> {
    let mut problems = Vec::new();
    let mut manifest_filenames = HashSet::<PathBuf>::new();
    let mut public_paths = HashSet::<PathBuf>::new();

    for (index, asset) in manifest.assets.iter().enumerate() {
        let label = format!(
            "assets[{index}] ('{}')",
            asset.filename
        );

        let filename = match safe_relative_path(&asset.filename) {
            Ok(filename) => {
                if !manifest_filenames.insert(filename.to_path_buf()) {
                    problems.push(format!(
                        "{label}: duplicate manifest filename '{}'",
                        asset.filename
                    ));
                }

                Some(filename)
            }

            Err(error) => {
                problems.push(format!(
                    "{label}: invalid filename: {error}"
                ));
                None
            }
        };

        if asset.description.trim().is_empty() {
            problems.push(format!(
                "{label}: description must not be empty"
            ));
        }

        match asset.kind {
            AssetKind::Image => {
                if asset.generation_prompt.trim().is_empty() {
                    problems.push(format!(
                        "{label}: image generation prompt must not be empty"
                    ));
                }

                if !asset.svg_code.trim().is_empty() {
                    problems.push(format!(
                        "{label}: image assets must have empty svg_code"
                    ));
                }

                if !has_extension(
                    &asset.filename,
                    &["png", "jpg", "jpeg", "webp"],
                ) {
                    problems.push(format!(
                        "{label}: image filename must end in .png, .jpg, \
                         .jpeg, or .webp"
                    ));
                }

                if !matches!(
                    asset.size.as_deref(),
                    Some(
                        "1024x1024"
                            | "1024x1536"
                            | "1536x1024"
                            | "auto"
                    )
                ) {
                    problems.push(format!(
                        "{label}: image size must be 1024x1024, \
                         1024x1536, 1536x1024, or auto"
                    ));
                }
            }

            AssetKind::Svg => {
                if asset.generation_prompt.trim().is_empty() {
                    problems.push(format!(
                        "{label}: SVG generation prompt must not be empty"
                    ));
                }

                if asset.svg_code.trim().is_empty() {
                    problems.push(format!(
                        "{label}: SVG source must not be empty"
                    ));
                }

                if !has_extension(&asset.filename, &["svg"]) {
                    problems.push(format!(
                        "{label}: SVG filename must end in .svg"
                    ));
                }

                if asset.size.as_deref() != Some("auto") {
                    problems.push(format!(
                        "{label}: SVG size must be auto"
                    ));
                }
            }

            AssetKind::CssGenerated => {
                if asset.generation_prompt.trim().is_empty() {
                    problems.push(format!(
                        "{label}: CSS generation prompt must not be empty"
                    ));
                }

                if !asset.svg_code.trim().is_empty() {
                    problems.push(format!(
                        "{label}: CSS-generated assets must have empty \
                         svg_code"
                    ));
                }

                if asset.size.is_some() {
                    problems.push(format!(
                        "{label}: CSS-generated asset size must be null"
                    ));
                }

                if filename.is_some_and(|filename| {
                    filename.components().count() != 1
                }) {
                    problems.push(format!(
                        "{label}: CSS-generated filename must be a \
                         single identifier, not a path"
                    ));
                }
            }

            AssetKind::Font => {
                problems.push(format!(
                    "{label}: font generation is not supported yet"
                ));
            }

            AssetKind::Video => {
                problems.push(format!(
                    "{label}: video generation is not supported yet"
                ));
            }
        }

        let Some(public_path) = asset.public_relative_path() else {
            continue;
        };

        let Some(filename) = filename else {
            continue;
        };

        if matches!(asset.kind, AssetKind::Image | AssetKind::Svg)
            && (filename.starts_with("images")
                || filename.starts_with("public/images"))
        {
            problems.push(format!(
                "{label}: filename must not begin with images/ or \
                 public/images/"
            ));
            continue;
        }

        if !public_paths.insert(public_path) {
            let public_url = asset
                .public_url()
                .unwrap_or_else(|| asset.filename.clone());

            problems.push(format!(
                "{label}: multiple assets resolve to public path \
                 {public_url}"
            ));
        }
    }

    problems
}

/// Validates a manifest at filesystem boundaries.
pub fn validate_manifest(manifest: &AssetsManifest) -> Result<()> {
    let problems = manifest_problems(manifest);

    if problems.is_empty() {
        return Ok(());
    }

    bail!(
        "invalid asset manifest:\n{}",
        format_manifest_problems(&problems)
    )
}

/// Ensures every generated file-backed asset is referenced through the public
/// URL derived from its manifest filename.
pub fn validate_asset_references<'a>(
    manifest: &AssetsManifest,
    source_files: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    let source_files = source_files.into_iter().collect::<Vec<_>>();
    let mut problems = Vec::new();

    for asset in &manifest.assets {
        let Some(public_url) = asset.public_url() else {
            continue;
        };

        if !source_files
            .iter()
            .any(|source| source.contains(&public_url))
        {
            problems.push(format!(
                "asset '{}' must be referenced as {public_url}",
                asset.filename
            ));
        }
    }

    if problems.is_empty() {
        return Ok(());
    }

    bail!(
        "invalid asset references:\n{}",
        format_manifest_problems(&problems)
    )
}

pub fn format_manifest_problems(problems: &[String]) -> String {
    problems
        .iter()
        .map(|problem| format!("- {problem}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Validates a path that must remain relative to a known parent.
pub fn safe_relative_path(value: &str) -> Result<&Path> {
    let path = Path::new(value);

    if value.is_empty()
        || value.trim() != value
        || value.contains('\\')
        || value.contains('\0')
        || value.contains('?')
        || value.contains('#')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("unsafe relative path: {value}");
    }

    Ok(path)
}

fn has_extension(filename: &str, allowed: &[&str]) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            allowed
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}

/// Removes a staging directory or staging symlink if it exists.
pub async fn remove_staging_dir(path: &Path) -> Result<()> {
    remove_path_if_exists(path).await
}

/// Recreates the staging directory without following a staging symlink.
async fn reset_staging_dir(path: &Path) -> Result<()> {
    remove_path_if_exists(path).await?;

    tokio::fs::create_dir_all(path)
        .await
        .with_context(|| {
            format!(
                "failed to create asset staging directory {}",
                path.display()
            )
        })
}

/// Removes either a directory, file, or symlink if present.
async fn remove_path_if_exists(path: &Path) -> Result<()> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect {}", path.display())
            });
        }
    };

    let file_type = metadata.file_type();

    if file_type.is_dir() && !file_type.is_symlink() {
        tokio::fs::remove_dir_all(path)
            .await
            .with_context(|| {
                format!(
                    "failed to remove staging directory {}",
                    path.display()
                )
            })
    } else {
        tokio::fs::remove_file(path)
            .await
            .with_context(|| {
                format!(
                    "failed to remove staging path {}",
                    path.display()
                )
            })
    }
}

/// Copies one staged asset, creating its destination directory first.
pub fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| {
                format!(
                    "failed to create asset directory {}",
                    parent.display()
                )
            })?;
    }

    std::fs::copy(source, destination)
        .with_context(|| {
            format!(
                "failed to install asset {} as {}",
                source.display(),
                destination.display()
            )
        })?;

    Ok(())
}
