//! Byte ranges over HTML, for edits that must not reserialize the source DOM.

use anyhow::{Context, Result, bail};
use std::ops::Range;

pub(super) struct Attribute {
    pub name: String,
    pub value: Range<usize>,
}

pub(super) struct Tag {
    pub name: String,
    pub range: Range<usize>,
    pub closing: bool,
    pub empty: bool,
    pub attributes: Vec<Attribute>,
    pub content: Option<Range<usize>>,
}

impl Tag {
    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name == name)
    }
}

/// Scans tag and attribute boundaries, treating raw script/style text and HTML
/// comments as opaque. The original whitespace, entities and SVG remain intact.
pub(super) fn tags(html: &str) -> Result<Vec<Tag>> {
    let bytes = html.as_bytes();
    let lower = html.to_ascii_lowercase();
    let mut output = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let start = i;
        if html[i..].starts_with("<!--") {
            i = html[i + 4..]
                .find("-->")
                .map_or(bytes.len(), |end| i + 4 + end + 3);
            continue;
        }
        i += 1;
        let closing = bytes.get(i) == Some(&b'/');
        if closing {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b':' | b'-'))
        {
            i += 1;
        }
        if i == name_start {
            continue;
        }
        let name = lower[name_start..i].to_owned();
        let mut attributes = Vec::new();
        while i < bytes.len() && bytes[i] != b'>' {
            if bytes[i].is_ascii_whitespace() || bytes[i] == b'/' {
                i += 1;
                continue;
            }
            let attribute_start = i;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b'=' | b'>' | b'/')
            {
                i += 1;
            }
            let attribute_name = lower[attribute_start..i].to_owned();
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if bytes.get(i) != Some(&b'=') {
                continue;
            }
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let quote = bytes
                .get(i)
                .copied()
                .filter(|byte| matches!(byte, b'\'' | b'\"'));
            if quote.is_some() {
                i += 1;
            }
            let value_start = i;
            while i < bytes.len()
                && match quote {
                    Some(quote) => bytes[i] != quote,
                    None => !bytes[i].is_ascii_whitespace() && bytes[i] != b'>',
                }
            {
                i += 1;
            }
            attributes.push(Attribute {
                name: attribute_name,
                value: value_start..i,
            });
            if quote.is_some() {
                i += 1;
            }
        }
        if i >= bytes.len() {
            bail!("unterminated <{name}> tag in imported HTML");
        }
        i += 1;
        let empty = html[start..i].ends_with("/>")
            || matches!(
                name.as_str(),
                "area"
                    | "base"
                    | "br"
                    | "col"
                    | "embed"
                    | "hr"
                    | "img"
                    | "input"
                    | "link"
                    | "meta"
                    | "param"
                    | "source"
                    | "track"
                    | "wbr"
            );
        let content = if !closing
            && !empty
            && matches!(name.as_str(), "script" | "style" | "textarea" | "title")
        {
            let end = lower[i..]
                .find(&format!("</{name}"))
                .with_context(|| format!("missing </{name}> in imported HTML"))?
                + i;
            let range = i..end;
            i = end;
            Some(range)
        } else {
            None
        };
        output.push(Tag {
            name,
            range: start..if let Some(content) = &content {
                content.start
            } else {
                i
            },
            closing,
            empty,
            attributes,
            content,
        });
    }
    Ok(output)
}

/// Applies disjoint byte edits. Overlap indicates an importer bug and is never
/// silently resolved by discarding one edit.
pub(super) fn edit(input: &str, mut edits: Vec<(Range<usize>, String)>) -> Result<String> {
    edits.sort_by_key(|(range, _)| range.start);
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    for (range, value) in edits {
        if range.start < cursor {
            bail!("overlapping remote source edits");
        }
        output.push_str(&input[cursor..range.start]);
        output.push_str(&value);
        cursor = range.end;
    }
    output.push_str(&input[cursor..]);
    Ok(output)
}

/// Source-host challenge middleware is not a frontend dependency. Cloudflare's
/// injected bootstrap creates a hidden iframe and calls an origin-bound route
/// that cannot be migrated. Remove only that identifiable host injection.
pub(super) fn without_host_bootstrap(html: &str) -> Result<(String, bool)> {
    let tokens = tags(html)?;
    let mut edits = Vec::new();
    for pair in tokens.windows(2) {
        let [tag, closing] = pair else { continue };
        if tag.name != "script" || tag.closing || !closing.closing || closing.name != "script" {
            continue;
        }
        let content = tag
            .content
            .as_ref()
            .map(|range| &html[range.clone()])
            .unwrap_or_default();
        let injected =
            content.contains("__CF$cv$params") && content.contains("/cdn-cgi/challenge-platform/");
        if injected {
            edits.push((tag.range.start..closing.range.end, String::new()));
        }
    }
    let changed = !edits.is_empty();
    Ok((edit(html, edits)?, changed))
}

/// Splits a site's header/footer into page-owned MiniJinja partials by byte
/// range. Article/section headers stay in place. All source text is literal to
/// MiniJinja, including JavaScript that happens to contain template delimiters.
pub(super) fn templates(html: &str, slug: &str) -> Result<(String, Vec<(String, String)>)> {
    let mut stack: Vec<(String, usize, bool)> = Vec::new();
    let mut ranges = Vec::new();
    for tag in tags(html)? {
        if tag.closing {
            if let Some(index) = stack.iter().rposition(|(name, _, _)| name == &tag.name) {
                let (name, start, extract) = stack[index].clone();
                stack.truncate(index);
                if extract {
                    ranges.push((start..tag.range.end, name));
                }
            }
        } else if !tag.empty {
            let extract = matches!(tag.name.as_str(), "header" | "footer")
                && !stack.iter().any(|(name, _, extracted)| {
                    *extracted || matches!(name.as_str(), "main" | "article" | "section" | "aside")
                });
            stack.push((tag.name, tag.range.start, extract));
        }
    }
    ranges.sort_by_key(|(range, _)| range.start);
    let mut page = String::new();
    let mut partials = Vec::new();
    let mut cursor = 0;
    for (index, (range, name)) in ranges.into_iter().enumerate() {
        let path = format!("partials/imports/{slug}/{name}-{index}.html");
        page.push_str(&literal(&html[cursor..range.start]));
        page.push_str(&format!("{{% include \"{path}\" %}}"));
        partials.push((path, literal(&html[range.clone()])));
        cursor = range.end;
    }
    page.push_str(&literal(&html[cursor..]));
    Ok((page, partials))
}

fn literal(text: &str) -> String {
    // Escape the only token recognized inside a raw block, including whitespace
    // control spellings. The expression emits exactly the original token.
    let endraw = regex::Regex::new(r"\{%[-+]?\s*endraw\s*[-+]?%\}").expect("raw token regex");
    let mut output = String::from("{% raw %}");
    let mut cursor = 0;
    for token in endraw.find_iter(text) {
        output.push_str(&text[cursor..token.start()]);
        output.push_str("{% endraw %}{{ ");
        output.push_str(&serde_json::to_string(token.as_str()).expect("string serialization"));
        output.push_str(" | safe }}{% raw %}");
        cursor = token.end();
    }
    output.push_str(&text[cursor..]);
    output.push_str("{% endraw %}");
    output
}
