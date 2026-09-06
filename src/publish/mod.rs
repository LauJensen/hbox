mod feeds;
mod transaction;

use anyhow::Result;

use crate::config::ResolvedSite;

#[allow(dead_code)]
pub(crate) fn publish(site: &ResolvedSite) -> Result<()> {
    feeds::generate(site)?;
    transaction::commit(site)
}

pub(crate) use transaction::commit;
