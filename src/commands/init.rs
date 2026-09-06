use crate::{
    cli::InitArgs,
    config::ResolvedSite,
};

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};
use std::{fs, path::Path};

static STARTER_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/resources/starter");

const SITE_DIRECTORIES: &[&str] = &[
    ".hbox",
    "blogposts",
    "pages",
    "partials",
    "public",
    "public/images",
    "templates",
];

pub fn run(args: InitArgs) -> Result<()> {
    let site = ResolvedSite::resolve(&args.site_name)?;

    initialize(&site, args.force)?;

    println!("Initialized {}", site.source_dir().display());

    Ok(())
}

fn initialize(site: &ResolvedSite, force: bool) -> Result<()> {
    let source_dir = site.source_dir();

    if source_dir.exists() && !source_dir.is_dir() {
        bail!(
            "site path exists but is not a directory: {}",
            source_dir.display(),
        );
    }

    fs::create_dir_all(source_dir).with_context(|| {
        format!(
            "failed to create site directory {}",
            source_dir.display(),
        )
    })?;

    for relative in SITE_DIRECTORIES {
        let directory = source_dir.join(relative);

        fs::create_dir_all(&directory).with_context(|| {
            format!(
                "failed to create site directory {}",
                directory.display(),
            )
        })?;
    }

    copy_embedded_dir(&STARTER_DIR, source_dir, force)
}

fn copy_embedded_dir(dir: &Dir<'_>, target_root: &Path, force: bool) -> Result<()> {
    for file in dir.files() {
        let relative_path = file.path();
        let target_path = target_root.join(relative_path);

        write_embedded_file(&target_path, file.contents(), force)?;
    }

    for child_dir in dir.dirs() {
        copy_embedded_dir(child_dir, target_root, force)?;
    }

    Ok(())
}

fn write_embedded_file(path: &Path, contents: &[u8], force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "Refusing to overwrite existing file: {}\nUse --force to overwrite starter files.",
            path.display()
        );
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    fs::write(path, contents)
        .with_context(|| format!("Failed to write file: {}", path.display()))?;

    Ok(())
}
