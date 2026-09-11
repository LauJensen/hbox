use crate::cli::OptimizeArgs;
use anyhow::{Result, Context};

use crate::{
    config::{load_config,ResolvedSite},
    utils::{remove_directory_if_exists,copy_all},
    publish,
};

use std::path::Path;
use crate::optimization::images;

pub fn optimize(
    dist_dir: &Path,
    webp_quality: u8,
) -> Result<()> {
    println!("Optimizing {:?}", dist_dir);

    let html_files =
        images::find_dist_html_files(dist_dir)?;

    let results_by_path =
        images::build_results_by_path(
            dist_dir,
            &html_files,
            webp_quality
        )?;

    for html_file in &html_files {
        images::optimize_html_file(
            dist_dir,
            html_file,
            &results_by_path,
        )?;
    }

    Ok(())
}

pub async fn run(args: OptimizeArgs) -> Result<()> {
    let site = ResolvedSite::resolve(&args.site_name)?;
    let staging = site.output_staging_dir();

    let config = load_config(site.source_dir())
        .context("Failed to load site configuration")?;

    remove_directory_if_exists(staging)?;

    println!("Converting all images to webp with quality {}%",
             config.optimizations.webp_quality);

    let preparation = (|| -> Result<()> {
        copy_all(site.output_dir(), staging)
            .context("failed to stage site for optimization")?;

        optimize(staging, config.optimizations.webp_quality)
            .context("failed to optimize staged site")?;

        Ok(())
    })();

    if let Err(error) = preparation {
        return match remove_directory_if_exists(staging) {
            Ok(()) => Err(error),

            Err(cleanup_error) => Err(error.context(format!(
                "optimization failed, and cleanup of {} also failed: \
                 {cleanup_error:#}",
                staging.display(),
            ))),
        };
    }

    publish::commit(&site)
        .context("failed to publish optimized site")?;

    Ok(())
}
