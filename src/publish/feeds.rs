use anyhow::Result;

use crate::config::ResolvedSite;

#[allow(dead_code)]
pub(super) fn generate(site: &ResolvedSite) -> Result<()> {
    let _output = site.output_staging_dir();

    // TODO: Generate RSS/Atom files inside `output`.

    Ok(())
}
