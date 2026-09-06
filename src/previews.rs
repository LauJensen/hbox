// previews.rs

use std::{
    fs,
    path::{Path},
    io::ErrorKind,
    num::NonZeroU32,
};

use anyhow::{Context, Result,bail};

use crate::{
    config::ResolvedSite,
    utils::copy_all,
};

/// Copies `site` into the next available preview source directory.
pub(crate) fn create(site: &ResolvedSite) -> Result<ResolvedSite> {
    let preview = reserve_next_preview(site)?;

    if let Err(error) = copy_all(&site.source_dir(), &preview.source_dir()) {
        let cleanup_result = fs::remove_dir_all(&preview.source_dir());

        return match cleanup_result {
            Ok(()) => Err(error).with_context(|| {
                format!("failed to create preview {}", preview)
            }),
            Err(cleanup_error) => Err(error).with_context(|| {
                format!(
                    "failed to create preview {}, and failed to remove its \
                     incomplete directory: {cleanup_error}",
                    preview,
                )
            }),
        };
    }

    Ok(preview)
}

/// Atomically claims the next preview number by creating its directory.
fn reserve_next_preview(site: &ResolvedSite) -> Result<ResolvedSite> {
    for index in 1.. {
        let preview = site.preview(NonZeroU32::new(index)
            .expect("Preview indices start at one"));

        match fs::create_dir(&preview.source_dir()) {
            Ok(()) => return Ok(preview),

            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                continue;
            }

            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create preview directory {}",
                        preview.source_dir().display(),
                    )
                });
            }
        }
    }

    unreachable!("What on earth do you need this many previews for!?")
}

/// Removes every filesystem artifact owned by a preview.
///
/// Every path is attempted even if an earlier removal fails.
pub fn remove_artifacts(preview: &ResolvedSite) -> Result<()> {
    let mut failures = Vec::new();

    for path in [
        preview.output_staging_dir(),
        preview.output_backup_dir(),
        preview.output_dir(),
        preview.source_dir(),
    ] {
        if let Err(error) = remove_path_if_exists(path) {
            failures.push(format!("{}: {error:#}", path.display()));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        bail!(
            "failed to remove all preview artifacts:\n- {}",
            failures.join("\n- "),
        );
    }
}

pub fn remove_path_if_exists(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {}", path.display()));
        }
    };

    let file_type = metadata.file_type();

    if file_type.is_dir() && !file_type.is_symlink() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove directory {}", path.display()))
    } else {
        fs::remove_file(path)
            .with_context(|| format!("failed to remove path {}", path.display()))
    }
}
