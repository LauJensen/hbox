use anyhow::{bail, Context, Result};

use std::{
    fs,
    io,
    path::{Component,Path,PathBuf},
};

use walkdir::WalkDir;

/// Lexically removes `.` components and resolves `..` where possible.
///
/// This does not access the filesystem and therefore does not resolve symlinks.
pub fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}

            Component::ParentDir => {
                let can_pop = matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                );

                if can_pop {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push("..");
                }
            }

            component @ Component::Prefix(_)
            | component @ Component::RootDir
            | component @ Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }

    normalized
}

/// Resolves `relative` against `base` and requires the result to remain in `root`.
///
/// A leading `/` is treated as URL-root notation and removed. The caller
/// chooses whether `base` is the site root or the current document directory.
pub fn resolve_within(
    root: &Path,
    base: &Path,
    relative: &str,
) -> Result<PathBuf> {
    let root = normalize_path(root);
    let base = normalize_path(base);

    if !base.starts_with(&root) {
        bail!(
            "base path {} is outside root {}",
            base.display(),
            root.display()
        );
    }

    let relative = relative.trim_start_matches('/');
    let relative_path = Path::new(relative);

    if relative_path
        .components()
        .any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir
            )
        })
    {
        bail!("path is not relative: {relative}");
    }

    let resolved = normalize_path(base.join(relative_path));

    if !resolved.starts_with(&root) {
        bail!("path escapes root: {relative}");
    }

    Ok(resolved)
}

// COPY UTILS

/// Recursively copies a directory's contents into an empty destination.
///
/// Source symlinks are followed and materialized as regular files or
/// directories. `walkdir` detects dangling links and symlink loops.
pub fn copy_all(
    source: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<()> {
    let source = source.as_ref();
    let destination = destination.as_ref();

    if !source.is_dir() {
        bail!(
            "copy source is not a directory: {}",
            source.display(),
        );
    }

    if destination.exists() {
        if !destination.is_dir() {
            bail!(
                "copy destination is not a directory: {}",
                destination.display(),
            );
        }

        let mut entries = fs::read_dir(destination)
            .with_context(|| {
                format!(
                    "failed to inspect destination {}",
                    destination.display(),
                )
            })?;

        if entries.next().transpose()?.is_some() {
            bail!(
                "copy destination is not empty: {}",
                destination.display(),
            );
        }
    } else {
        fs::create_dir_all(destination).with_context(|| {
            format!(
                "failed to create destination {}",
                destination.display(),
            )
        })?;
    }

    let source = fs::canonicalize(source)
        .context("failed to resolve copy source")?;

    let destination = fs::canonicalize(destination)
        .context("failed to resolve copy destination")?;

    if destination == source || destination.starts_with(&source) {
        bail!(
            "copy destination must not be inside the source directory",
        );
    }

    for entry in WalkDir::new(&source)
        .follow_links(true)
        .min_depth(1)
    {
        let entry = entry.with_context(|| {
            format!(
                "failed while traversing {}",
                source.display(),
            )
        })?;

        let relative = entry
            .path()
            .strip_prefix(&source)
            .with_context(|| {
                format!(
                    "failed to resolve relative path for {}",
                    entry.path().display(),
                )
            })?;

        let target = destination.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).with_context(|| {
                format!(
                    "failed to create directory {}",
                    target.display(),
                )
            })?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "failed to create directory {}",
                        parent.display(),
                    )
                })?;
            }

            fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    entry.path().display(),
                    target.display(),
                )
            })?;
        } else {
            bail!(
                "unsupported filesystem entry: {}",
                entry.path().display(),
            );
        }
    }

    Ok(())
}

pub fn remove_directory_if_exists(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("Failed to remove directory {}", path.display()))
        }
    }
}
