use std::fs;

use anyhow::{bail, Context, Result};

use crate::config::ResolvedSite;

/// Flips dist/foo          => dist/.foo.backup
///       dist/.foo.staging => dist/foo
///
/// Auto rollback on failure
pub(crate) fn commit(site: &ResolvedSite) -> Result<()> {
    let staging = site.output_staging_dir();
    let output  = site.output_dir();
    let backup  = site.output_backup_dir();

    if !staging.is_dir() {
        bail!(
            "output staging directory does not exist: {}",
            staging.display()
        );
    }

    if backup.exists() {
        fs::remove_dir_all(backup).with_context(|| {
            format!("removing stale backup {}", backup.display())
        })?;
    }

    if output.exists() {
        fs::rename(output, backup).with_context(|| {
            format!(
                "moving published output {} to {}",
                output.display(),
                backup.display()
            )
        })?;
    }

    if let Err(publish_error) = fs::rename(staging, output) {
        if backup.exists() {
            if let Err(rollback_error) = fs::rename(backup, output) {
                return Err(publish_error).with_context(|| {
                    format!(
                        "publishing {} failed, and restoring {} also failed: \
                         {rollback_error:#}",
                        staging.display(),
                        backup.display(),
                    )
                });
            }
        }

        return Err(publish_error).with_context(|| {
            format!(
                "publishing {} to {}",
                staging.display(),
                output.display()
            )
        });
    }

    if backup.exists() {
        fs::remove_dir_all(backup).with_context(|| {
            format!("removing old output backup {}", backup.display())
        })?;
    }

    Ok(())
}
