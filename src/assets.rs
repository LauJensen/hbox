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

/// Generates all image and SVG assets into a clean staging directory.
///
/// Assets are staged by `filename`; their public `path` is used only when
/// installing them into the site.
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
        if let AssetKind::Svg = &asset.kind {
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
    }

    Ok(())
}

/// Installs staged image and SVG assets at their manifest paths in `public/`.
///
/// `CssGenerated` entries do not correspond to files and are skipped.
pub fn install_assets(
    site_dir: &Path,
    staging_dir: &Path,
    manifest: &AssetsManifest,
) -> Result<()> {
    let mut installed_paths = HashSet::new();

    for asset in &manifest.assets {
        if let AssetKind::CssGenerated = &asset.kind {
            continue;
        }

        let filename = safe_relative_path(&asset.filename)?;
        let public_path = public_asset_path(&asset.path)?;

        if !installed_paths.insert(public_path.to_path_buf()) {
            bail!(
                "multiple assets use the public path {}",
                asset.path
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

/// Removes a staging directory or staging symlink if it exists.
pub async fn remove_staging_dir(path: &Path) -> Result<()> {
    remove_path_if_exists(path).await
}

/// Validates a path that must remain relative to a known parent.
pub fn safe_relative_path(value: &str) -> Result<&Path> {
    let path = Path::new(value);

    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("unsafe relative path: {value}");
    }

    Ok(path)
}

/// Validates all file-producing entries before generation starts.
fn validate_manifest(manifest: &AssetsManifest) -> Result<()> {
    let mut filenames = HashSet::new();
    let mut public_paths = HashSet::<PathBuf>::new();

    for asset in &manifest.assets {
        match &asset.kind {
            AssetKind::CssGenerated => continue,

            AssetKind::Image => {
                if asset.generation_prompt.trim().is_empty() {
                    bail!(
                        "image asset '{}' has no generation prompt",
                        asset.filename
                    );
                }
            }

            AssetKind::Svg => {
                if asset.svg_code.trim().is_empty() {
                    bail!(
                        "SVG asset '{}' has no SVG source",
                        asset.filename
                    );
                }
            }
        }

        safe_relative_path(&asset.filename)
            .with_context(|| {
                format!(
                    "invalid asset filename '{}'",
                    asset.filename
                )
            })?;

        let public_path = public_asset_path(&asset.path)
            .with_context(|| {
                format!(
                    "invalid public asset path '{}'",
                    asset.path
                )
            })?;

        if !filenames.insert(asset.filename.as_str()) {
            bail!(
                "duplicate asset filename '{}'",
                asset.filename
            );
        }

        if !public_paths.insert(public_path.to_path_buf()) {
            bail!(
                "duplicate public asset path '{}'",
                asset.path
            );
        }
    }

    Ok(())
}

/// Converts `/images/foo.png` into the safe relative path `images/foo.png`.
fn public_asset_path(value: &str) -> Result<&Path> {
    let relative = value.trim_start_matches('/');
    let path = safe_relative_path(relative)?;

    let mut components = path.components();

    let begins_with_images = match components.next() {
        Some(Component::Normal(component)) => component == "images",
        _ => false,
    };

    if !begins_with_images || components.next().is_none() {
        bail!(
            "public asset path must be beneath /images/: {value}"
        );
    }

    Ok(path)
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
