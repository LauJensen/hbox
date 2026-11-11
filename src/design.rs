//! Owns marked regions of design.css shared by generated and preserved pages.

use anyhow::{Context, Result, bail};
use std::{io::ErrorKind, path::Path};

const DESIGN_BLOCK_START_PREFIX: &str = "/* hbox design import:start ";
const DESIGN_BLOCK_END_PREFIX: &str = "/* hbox design import:end ";

/// Inserts or replaces only the block owned by this imported page.
///
/// Everything outside the markers remains developer-owned and untouched. If a
/// regenerated import no longer has reusable design CSS, its previous block is
/// removed.
pub async fn upsert_block(path: &Path, page_slug: &str, css: Option<&str>) -> Result<()> {
    let existing = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read design stylesheet {}", path.display()));
        }
    };

    let start_marker = format!("{DESIGN_BLOCK_START_PREFIX}{page_slug} */");
    let end_marker = format!("{DESIGN_BLOCK_END_PREFIX}{page_slug} */");

    let without_previous = remove_marked_block(&existing, &start_marker, &end_marker)?;

    let updated = match non_empty(css) {
        Some(css) => {
            let mut output = without_previous.trim_end().to_string();

            if !output.is_empty() {
                output.push_str("\n\n");
            }

            output.push_str(&start_marker);
            output.push('\n');
            output.push_str(css.trim());
            output.push('\n');
            output.push_str(&end_marker);
            output.push('\n');

            output
        }
        None => {
            let trimmed = without_previous.trim_end();

            if trimmed.is_empty() {
                String::new()
            } else {
                format!("{trimmed}\n")
            }
        }
    };

    tokio::fs::write(path, updated)
        .await
        .with_context(|| format!("failed to write design stylesheet {}", path.display()))?;

    Ok(())
}

fn remove_marked_block(input: &str, start_marker: &str, end_marker: &str) -> Result<String> {
    let Some(start) = input.find(start_marker) else {
        if input.contains(end_marker) {
            bail!(
                "found design CSS end marker without start marker: {}",
                end_marker
            );
        }

        return Ok(input.to_string());
    };

    let search_from = start + start_marker.len();

    let relative_end = input[search_from..].find(end_marker).with_context(|| {
        format!(
            "found design CSS start marker without end marker: {}",
            start_marker
        )
    })?;

    let end = search_from + relative_end + end_marker.len();

    if input[end..].contains(start_marker) {
        bail!(
            "multiple design CSS blocks found for the same page: {}",
            start_marker
        );
    }

    let mut output = String::with_capacity(input.len());
    output.push_str(&input[..start]);
    output.push_str(input[end..].trim_start_matches(['\r', '\n']));

    Ok(output)
}

/// Wraps original CSS without changing its bytes inside the markers.
pub fn block_text(key: &str, css: &str) -> String {
    format!("{DESIGN_BLOCK_START_PREFIX}{key} */\n{css}\n{DESIGN_BLOCK_END_PREFIX}{key} */\n")
}

/// Returns a block's content, excluding only the marker framing newlines.
pub fn block<'a>(input: &'a str, key: &str) -> Result<Option<&'a str>> {
    let start = format!("{DESIGN_BLOCK_START_PREFIX}{key} */");
    let end = format!("{DESIGN_BLOCK_END_PREFIX}{key} */");
    let Some(offset) = input.find(&start) else {
        if input.contains(&end) {
            bail!("design CSS end marker has no start: {key}");
        }
        return Ok(None);
    };
    let content = &input[offset + start.len()..];
    let finish = content
        .find(&end)
        .with_context(|| format!("missing design CSS end marker: {key}"))?;
    if content[finish + end.len()..].contains(&start) {
        bail!("duplicate design CSS block: {key}");
    }
    let text = &content[..finish];
    let text = text.strip_prefix("\n").unwrap_or(text);
    Ok(Some(text.strip_suffix("\n").unwrap_or(text)))
}

/// Removes one owned block, retaining unrelated developer and page styles.
pub fn without_block(input: &str, key: &str) -> Result<String> {
    remove_marked_block(
        input,
        &format!("{DESIGN_BLOCK_START_PREFIX}{key} */"),
        &format!("{DESIGN_BLOCK_END_PREFIX}{key} */"),
    )
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
