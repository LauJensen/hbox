//! Relocates one captured dependency graph. URLs change; markup, CSS order,
//! executable code and hydration data otherwise retain their original contents.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use regex::Regex;
use reqwest::Url;
use sha2::{Digest, Sha256};

use super::{
    html, javascript,
    source::{self, PageSource},
};

pub(super) struct Stylesheet {
    pub key: String,
    pub path: String,
    pub content: String,
}

pub(super) struct RelocatedPage {
    pub html: String,
    pub styles: Vec<Stylesheet>,
    pub scripts: Vec<(String, String)>,
}

struct Relocator {
    urls: HashMap<String, String>,
    endpoint: Option<(Url, String)>,
}

impl RelocatedPage {
    /// Assigns all names before rewriting any contents, so cyclic module graphs
    /// and bundler preload tables resolve without content-hash recursion.
    pub fn new(source: &PageSource, slug: &str, endpoint: Option<&Url>) -> Result<Self> {
        let snapshot = hash(&serde_json::to_vec(source)?);
        let prefix = format!("imports/{slug}/{snapshot}");
        let mut urls = HashMap::new();
        for (resources, directory, extension) in [
            (&source.stylesheets, "css", "css"),
            (&source.scripts, "js", "js"),
        ] {
            for resource in resources {
                let path = format!(
                    "/{directory}/{prefix}/{}.{extension}",
                    hash(resource.url.as_bytes())
                );
                urls.insert(resource.url.clone(), path);
            }
        }
        for asset in &source.assets {
            urls.insert(asset.source_url.clone(), asset.public_url.clone());
        }
        for (alias, canonical) in &source.aliases {
            if let Some(path) = urls.get(canonical).cloned() {
                urls.insert(alias.clone(), path);
            }
        }
        let base = Url::parse(&source.base_url)?;
        let endpoint = endpoint
            .map(|endpoint| -> Result<_> {
                source::validate_url(endpoint)?;
                Ok((submission_url(source, &base)?, endpoint.to_string()))
            })
            .transpose()?;
        let relocator = Relocator { urls, endpoint };
        let mut styles = Vec::new();
        let mut scripts = Vec::new();
        let mut integrity = HashMap::new();
        for resource in &source.stylesheets {
            let path = relocator.urls[&resource.url].clone();
            let content = relocator.css(&resource.content, &Url::parse(&resource.url)?)?;
            integrity.insert(path.clone(), sri(&content));
            styles.push(Stylesheet {
                key: format!("source-{snapshot}-{}", hash(resource.url.as_bytes())),
                path,
                content,
            });
        }
        for resource in &source.scripts {
            let path = relocator.urls[&resource.url].clone();
            let content = relocator.script(&resource.content, &Url::parse(&resource.url)?);
            integrity.insert(path.clone(), sri(&content));
            scripts.push((path, content));
        }
        let html = relocator.html(&source.html, &base, &integrity)?;
        Ok(Self {
            html,
            styles,
            scripts,
        })
    }
}

impl Relocator {
    fn url(&self, value: &str, base: &Url) -> Option<String> {
        if value.is_empty() || value.starts_with('#') || value.starts_with("data:") {
            return None;
        }
        let mut url = base.join(value).ok()?;
        let fragment = url.fragment().map(str::to_owned);
        url.set_fragment(None);
        let mut target = self.urls.get(url.as_str())?.clone();
        if let Some(fragment) = fragment {
            target.push('#');
            target.push_str(&fragment);
        }
        Some(target)
    }

    fn script(&self, code: &str, base: &Url) -> String {
        let fetch_argument = Regex::new(r"\bfetch\s*\(\s*$").expect("fetch argument regex");
        javascript::rewrite(code, &|value, prefix| {
            if let Some(path) = value.strip_prefix("css:") {
                return self.url(path, base).map(|path| format!("css:{path}"));
            }
            if let Some((old, new)) = &self.endpoint
                && fetch_argument.is_match(prefix)
                && base.join(value).ok().as_ref() == Some(old)
            {
                return Some(new.clone());
            }
            let url = source::script_url(base, value)?;
            let target = self.urls.get(url.as_str())?;
            // Vite's root-relative preload hints are prefixed with '/' by the
            // runtime. ES imports and already absolute paths keep the slash.
            if !value.starts_with('.') && !value.starts_with('/') && !value.contains("://") {
                Some(target.trim_start_matches('/').to_owned())
            } else {
                Some(target.clone())
            }
        })
    }

    fn css(&self, code: &str, base: &Url) -> Result<String> {
        let references = Regex::new(
            r#"(?i)(?:url\(\s*(?:"([^"\r\n]*)"|'([^'\r\n]*)'|([^\s\)]+))|@import\s+(?:"([^"\r\n]*)"|'([^'\r\n]*)'))"#,
        )?;
        let edits = references
            .captures_iter(code)
            .filter_map(|capture| {
                let value = (1..=5).find_map(|index| capture.get(index))?;
                self.url(value.as_str(), base)
                    .map(|target| (value.range(), target))
            })
            .collect();
        html::edit(code, edits)
    }

    fn html(&self, code: &str, base: &Url, integrity: &HashMap<String, String>) -> Result<String> {
        let mut edits = Vec::new();
        let paths = Regex::new(r"[^\s,]+")?;
        for tag in html::tags(code)? {
            if tag.closing {
                continue;
            }
            // An external <base> would redirect even /local/assets to the old
            // origin. Such pages need explicit navigation semantics first.
            ensure!(
                tag.name != "base",
                "URL import does not yet support <base>; remove it from the source page before importing"
            );
            if tag.name == "meta"
                && tag.attribute("http-equiv").is_some_and(|a| {
                    code[a.value.clone()].eq_ignore_ascii_case("content-security-policy")
                })
            {
                let policy = tag
                    .attribute("content")
                    .map(|a| &code[a.value.clone()])
                    .unwrap_or_default();
                ensure!(
                    !policy.contains("sha256-")
                        && !policy.contains("sha384-")
                        && !policy.contains("sha512-"),
                    "hash-based CSP in imported HTML needs a policy for the relocated scripts and styles"
                );
            }
            let mut resource = None;
            for attribute in &tag.attributes {
                let original = &code[attribute.value.clone()];
                let value = html_escape::decode_html_entities(original);
                let replacement = match attribute.name.as_str() {
                    "src" | "href" | "xlink:href" | "poster" | "data-rsc-css-href" | "content" => {
                        self.url(&value, base)
                    }
                    "srcset" if !value.trim_start().starts_with("data:") => {
                        let changes = paths
                            .find_iter(&value)
                            .filter_map(|path| {
                                self.url(path.as_str(), base).map(|url| (path.range(), url))
                            })
                            .collect();
                        Some(html::edit(&value, changes)?)
                    }
                    "style" => Some(self.css(&value, base)?),
                    "action" => self.endpoint.as_ref().and_then(|(old, new)| {
                        (base.join(&value).ok().as_ref() == Some(old)).then(|| new.clone())
                    }),
                    name if name.starts_with("on") => Some(self.script(&value, base)),
                    _ => None,
                };
                if let Some(replacement) =
                    replacement.filter(|replacement| replacement != value.as_ref())
                {
                    if matches!(attribute.name.as_str(), "src" | "href") {
                        resource = Some(replacement.clone());
                    }
                    edits.push((
                        attribute.value.clone(),
                        html_escape::encode_quoted_attribute(&replacement).into_owned(),
                    ));
                }
            }
            if let (Some(path), Some(attribute)) = (resource, tag.attribute("integrity"))
                && let Some(value) = integrity.get(&path)
            {
                edits.push((attribute.value.clone(), value.clone()));
            }
            if let Some(range) = tag.content {
                let content = &code[range.clone()];
                let replacement = match tag.name.as_str() {
                    "script" => self.script(content, base),
                    "style" => self.css(content, base)?,
                    _ => continue,
                };
                if replacement != content {
                    edits.push((range, replacement));
                }
            }
        }
        html::edit(code, edits)
    }
}

/// An override changes one existing submission destination; the frontend's
/// payload, validation and success/error handling remain owned by that frontend.
fn submission_url(source: &PageSource, base: &Url) -> Result<Url> {
    let mut candidates = HashSet::new();
    for tag in html::tags(&source.html)? {
        if tag.name == "form"
            && let Some(attribute) = tag.attribute("action")
        {
            let value = html_escape::decode_html_entities(&source.html[attribute.value.clone()]);
            candidates.insert(base.join(&value)?.to_string());
        }
    }
    let fetch = Regex::new(
        r#"\bfetch\s*\(\s*["'`]([^"'`\s]+)["'`]\s*,\s*\{[^}]{0,1024}?\bmethod\s*:\s*["'`](?:POST|PUT|PATCH)["'`]"#,
    )?;
    for script in std::iter::once(source.html.as_str())
        .chain(source.scripts.iter().map(|s| s.content.as_str()))
    {
        for capture in fetch.captures_iter(script) {
            candidates.insert(base.join(&capture[1])?.to_string());
        }
    }
    ensure!(
        candidates.len() == 1,
        "--form-endpoint requires exactly one static form action or fetch submission endpoint; found {}",
        candidates.len()
    );
    Url::parse(candidates.iter().next().context("missing form endpoint")?)
        .context("invalid form destination")
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sri(content: &str) -> String {
    format!(
        "sha256-{}",
        STANDARD.encode(Sha256::digest(content.as_bytes()))
    )
}
