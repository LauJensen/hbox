use std::{
    path::Path,
    process::Command,
};

use crate::{
    cli::DeployArgs,
    config::{load_config,ResolvedSite},
};


use anyhow::{bail, Context, Result};

pub async fn run(args: DeployArgs) -> Result<()> {
    let site = ResolvedSite::resolve(&args.site_name)?;

    if !site.output_dir().exists() {
        bail!(format!("Cant deploy what isn't there. Run hbox build {} first",
                      &site.site_name));
    }

    let config = load_config(site.source_dir())?;

    let Some(deployment) = config.deployment.as_ref() else {
        bail!("The [deployment] section of hbox.toml is not filled out!");
    };

    rsync_deploy(
        &site.output_dir(),
        &deployment.ssh_user,
        &deployment.ssh_host,
        &deployment.deploy_path
    )
}

pub fn rsync_deploy(
    dist: &Path,
    user: &str,
    host: &str,
    deploy_path: &str,
) -> Result<()> {
    if !dist.is_dir() {
        bail!("deployment source does not exist: {}", dist.display());
    }

    let deploy_path = deploy_path.trim_end_matches('/');

    if deploy_path.is_empty() {
        bail!("deployment path cannot be empty or the filesystem root");
    }

    let destination = format!("{user}@{host}:{deploy_path}/");

    let status = Command::new("rsync")
        .arg("-az")
        .arg("--delete")
        .arg("--")
        .arg(dist) // Deliberately no trailing slash.
        .arg(&destination)
        .status()
        .context("failed to start rsync, make sure it's installed on PATH")?;

    if !status.success() {
        bail!("rsync failed with status {status}");
    }

    Ok(())
}
