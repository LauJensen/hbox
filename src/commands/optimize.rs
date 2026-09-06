use crate::cli::OptimizeArgs;
use anyhow::{Result, Context};

use crate::{
    config::ResolvedSite,
    utils::{remove_directory_if_exists,copy_all},
    publish,
};

use std::path::Path;
use crate::optimization::images;

pub fn optimize(dist_dir: &Path) -> Result<()> {
    println!("Optimizing {:?}", &dist_dir);

    let html_files = images::find_dist_html_files(&dist_dir)?;

    for file in html_files {
        images::optimize_html_file(&dist_dir,
                                   &file)?;
    }

    Ok(())

}

pub async fn run(args: OptimizeArgs) -> Result<()> {
    let site = ResolvedSite::resolve(&args.site_name)?;
    let staging = site.output_staging_dir();

    remove_directory_if_exists(staging)?;

    let preparation = (|| -> Result<()> {
        copy_all(site.output_dir(), staging)
            .context("failed to stage site for optimization")?;

        optimize(staging)
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
