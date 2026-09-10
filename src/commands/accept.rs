use crate::{
    cli::AcceptPreviewArgs,
    utils::{copy_all},
    config::{ResolvedSite},
};

use std::{
    fs,
    num::NonZeroU32,
    path::{PathBuf},
};

use crate::commands::{
    build::build_site,
    validate::validate_site,
};

use crate::previews::remove_artifacts;

use anyhow::{Context, Result,bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub index: NonZeroU32,
    pub path: PathBuf,
}

pub async fn run(args: AcceptPreviewArgs) -> Result<()> {
    let site = ResolvedSite::resolve(&args.site_name)?;

    let site_dir = &site.source_dir();

    if !site_dir.is_dir() {
        bail!(
            "site '{}' does not exist in the sites directory",
            site.site_name()
        );
    }

    let previews = collect_previews(&site.site_name())?;

    let selected_preview = previews
        .iter()
        .find(|preview| preview.index == args.preview_num)
        .with_context(|| {
            format!(
                "preview index {} was not found for site '{}'",
                args.preview_num, site.site_name()
            )
        })?;

    let sites_path = PathBuf::from("sites");
    let staging_dir = sites_path.join(format!(".{}-accepting", site.site_name()));
    let backup_dir = sites_path.join(format!(".{}-backup", site.site_name()));

    let accepted_site = site.preview(args.preview_num);
    let build_report = build_site(&accepted_site)
        .context("failed to build accepted site")?;

    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir).with_context(|| {
            format!(
                "failed to remove stale staging directory {}",
                staging_dir.display()
            )
        })?;
    }

    if backup_dir.exists() {
        bail!(
            "backup directory {} already exists; refusing to overwrite it",
            backup_dir.display()
        );
    }

    copy_all(&selected_preview.path, &staging_dir).with_context(|| {
        format!(
            "failed to copy preview {} into staging directory",
            args.preview_num
        )
    })?;

    fs::rename(&site_dir, &backup_dir).with_context(|| {
        format!(
            "failed to move existing site {} to backup",
            site_dir.display()
        )
    })?;

    if let Err(error) = fs::rename(&staging_dir, &site_dir) {
        // Best-effort rollback to preserve the original site.
        let _ = fs::rename(&backup_dir, &site_dir);

        return Err(error).with_context(|| {
            format!(
                "failed to move accepted preview into {}",
                site_dir.display()
            )
        });
    }

    println!(
        "Accepted preview {} for site '{}'.",
        args.preview_num, site.site_name()
    );

    let site = ResolvedSite::resolve(&site_dir)?;

    let _build_report = build_site(&site)
        .context("failed to build accepted site after swap on disk")?;

    match validate_site(&build_report.output_dir, false)
        .await {
            Ok(_)      => {}
            Err(error) => eprintln!("Validation could not run: {error:#}"),
            }

    fs::remove_dir_all(&backup_dir).with_context(|| {
        format!("failed to remove backup {}", backup_dir.display())
    })?;

    for preview in &previews {
        println!("- removing all artifacts for {}", preview.path.display());

        let preview_site = site.preview(preview.index);

        remove_artifacts(&preview_site)
            .with_context(|| {
                format!(
                    "failed to remove artifacts for preview {}",
                    preview.path.display()
                )
            })?;
    }

    Ok(())
}

/// Extracts the numeric preview index from a folder named
/// `<site_name>-preview<N>`.
fn parse_preview_index(site_name: &str, folder_name: &str) -> Option<NonZeroU32> {
    let prefix = format!(".preview-{site_name}-");
    let suffix = folder_name.strip_prefix(&prefix)?;

    if suffix.is_empty() {
        return None;
    }

    suffix.parse().ok()
}

pub fn collect_previews(site_name: &str) ->
    Result<Vec<Preview>>
{
    let sites_path = PathBuf::from("sites");
    let mut previews = Vec::new();

    for entry in fs::read_dir(&sites_path)
        .with_context(|| format!("failed to read {}", sites_path.display()))?
    {
        let entry = entry.context("failed to read entry in sites directory")?;

        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}",
                                     entry.path().display()))?;

        if !file_type.is_dir() {
            continue;
        }

        let folder_name = entry.file_name();
        let Some(folder_name) = folder_name.to_str() else {
            continue
        };
        let Some(index) = parse_preview_index(site_name, folder_name) else {
            continue;
        };

        previews.push(Preview {
            index,
            path: entry.path(),
        });
    }

    previews.sort_unstable_by_key(|preview| preview.index);

    Ok(previews)
}
