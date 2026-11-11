//! Collects a bounded, read-only page snapshot. Nothing downloaded is executed.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use kuchiki::traits::TendrilSink;
use regex::Regex;
use reqwest::{Client, Url};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    ai::{AssetKind, AssetManifestItem, AssetOrigin, AssetsManifest},
    assets::safe_relative_path,
};

const MAX_RESOURCES: usize = 96;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 48 * 1024 * 1024;

#[derive(Clone, Copy)]
enum ResourceKind {
    Stylesheet,
    Script,
    Asset,
}

struct Reference {
    url: Url,
    kind: ResourceKind,
}

#[derive(Serialize)]
pub(super) struct TextResource {
    pub url: String,
    pub content: String,
}

#[derive(Serialize)]
pub(super) struct Asset {
    pub source_url: String,
    pub public_url: String,
    filename: String,
    kind: AssetKind,
    #[serde(skip)]
    bytes: Vec<u8>,
}

/// A captured dependency graph. Source URLs and original bytes are owned here;
/// callers can relocate resources without refetching or regenerating them.
#[derive(Serialize)]
pub(super) struct PageSource {
    pub url: String,
    pub base_url: String,
    pub html: String,
    pub stylesheets: Vec<TextResource>,
    pub scripts: Vec<TextResource>,
    pub assets: Vec<Asset>,
    pub aliases: BTreeMap<String, String>,
    pub notes: Vec<String>,
}

struct Fetcher {
    client: Client,
    received_bytes: usize,
    text_bytes: usize,
    deadline: std::time::Instant,
}

struct Resource {
    url: Url,
    mime: String,
    bytes: Vec<u8>,
}

/// Accepts only HTTP(S) locations without embedded credentials.
pub(super) fn validate_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("URL import requires an HTTP(S) URL with a host and no embedded credentials");
    }
    Ok(())
}

impl PageSource {
    /// Follows static dependencies, including vendor runtimes. A missing required
    /// resource fails the import instead of producing a partially working page.
    pub async fn fetch(url: &Url, slug: &str) -> Result<Self> {
        validate_url(url)?;
        let mut fetcher = Fetcher::new()?;
        let page = fetcher.fetch(url, MAX_TEXT_BYTES).await?;
        if !page.mime.is_empty() && !page.mime.contains("html") {
            bail!("expected an HTML page at {url}, received {}", page.mime);
        }
        let document_url = page.url.clone();
        let html = fetcher.text(page)?;
        let (html, removed_host_bootstrap) = super::html::without_host_bootstrap(&html)?;
        let (base_url, mut queue) = html_references(&html, &document_url)?;
        let mut seen = HashSet::from([document_url.to_string()]);
        let mut source = Self {
            url: document_url.to_string(),
            base_url: base_url.to_string(),
            html,
            stylesheets: Vec::new(),
            scripts: Vec::new(),
            assets: Vec::new(),
            aliases: BTreeMap::new(),
            notes: if removed_host_bootstrap {
                vec!["Removed the source host's injected Cloudflare challenge bootstrap; origin-bound security middleware is not part of the frontend.".to_owned()]
            } else {
                Vec::new()
            },
        };

        while let Some(reference) = queue.pop_front() {
            if !seen.insert(reference.url.to_string()) {
                continue;
            }
            if seen.len() > MAX_RESOURCES {
                bail!("page exceeds the {MAX_RESOURCES}-resource import limit");
            }
            source
                .collect_reference(reference, &mut fetcher, &mut queue, slug)
                .await?;
        }
        if source.html.trim().is_empty() {
            bail!("remote page is empty");
        }
        Ok(source)
    }

    async fn collect_reference(
        &mut self,
        reference: Reference,
        fetcher: &mut Fetcher,
        queue: &mut VecDeque<Reference>,
        slug: &str,
    ) -> Result<()> {
        let limit = if matches!(reference.kind, ResourceKind::Asset) {
            MAX_FILE_BYTES
        } else {
            MAX_TEXT_BYTES
        };
        let resource = fetcher.fetch(&reference.url, limit).await?;
        if reference.url != resource.url {
            self.aliases
                .insert(reference.url.to_string(), resource.url.to_string());
        }
        match reference.kind {
            ResourceKind::Stylesheet | ResourceKind::Script => {
                if resource.mime.contains("html") {
                    bail!("received HTML instead of source code at {}", reference.url);
                }
                let resource_url = resource.url.clone();
                let captured = match reference.kind {
                    ResourceKind::Stylesheet => &self.stylesheets,
                    _ => &self.scripts,
                };
                if captured
                    .iter()
                    .any(|item| item.url == resource_url.as_str())
                {
                    return Ok(());
                }
                let content = fetcher.text(resource)?;
                if matches!(reference.kind, ResourceKind::Stylesheet) {
                    queue.extend(css_references(&content, &resource_url)?);
                    self.stylesheets.push(TextResource {
                        url: resource_url.to_string(),
                        content,
                    });
                } else {
                    queue.extend(script_references(&content, &resource_url)?);
                    self.scripts.push(TextResource {
                        url: resource_url.to_string(),
                        content,
                    });
                }
            }
            ResourceKind::Asset => {
                let url = resource.url.clone();
                if !self
                    .assets
                    .iter()
                    .any(|asset| asset.source_url == url.as_str())
                {
                    self.add_asset(&url, resource, slug)?;
                }
            }
        }
        Ok(())
    }

    fn add_asset(&mut self, source_url: &Url, resource: Resource, slug: &str) -> Result<()> {
        let (kind, extension) = asset_type(&resource)?;
        let mut hash = Sha256::new();
        hash.update(source_url.as_str().as_bytes());
        hash.update(&resource.bytes);
        let filename = format!("imports/{slug}/{:x}.{extension}", hash.finalize());
        let item = AssetManifestItem {
            filename: filename.clone(),
            kind,
            description: source_url.to_string(),
            origin: AssetOrigin::Downloaded,
            generation_prompt: String::new(),
            size: None,
            svg_code: String::new(),
        };
        self.assets.push(Asset {
            source_url: source_url.to_string(),
            filename,
            kind,
            public_url: item
                .public_url()
                .context("downloaded asset has no public URL")?,
            bytes: resource.bytes,
        });
        Ok(())
    }

    /// Describes every captured binary using the existing asset installer contract.
    pub fn manifest(&self) -> AssetsManifest {
        AssetsManifest {
            assets: self
                .assets
                .iter()
                .map(|asset| AssetManifestItem {
                    filename: asset.filename.clone(),
                    kind: asset.kind,
                    description: asset.source_url.clone(),
                    origin: AssetOrigin::Downloaded,
                    generation_prompt: String::new(),
                    size: None,
                    svg_code: String::new(),
                })
                .collect(),
        }
    }

    /// Stages the bytes already fetched, using the manifest's validated names.
    pub async fn stage(&self, manifest: &AssetsManifest, directory: &Path) -> Result<()> {
        for item in &manifest.assets {
            let asset = self
                .assets
                .iter()
                .find(|asset| asset.filename == item.filename)
                .context("downloaded asset is missing from the source snapshot")?;
            let path = directory.join(safe_relative_path(&item.filename)?);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&path, &asset.bytes)
                .await
                .with_context(|| format!("failed to stage {}", path.display()))?;
        }
        Ok(())
    }
}

impl Fetcher {
    fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("hbox/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error("too many import redirects");
                }
                match validate_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(error) => attempt.error(error.to_string()),
                }
            }))
            .build()
            .context("failed to create URL import HTTP client")?;
        Ok(Self {
            client,
            received_bytes: 0,
            text_bytes: 0,
            deadline: std::time::Instant::now() + Duration::from_secs(120),
        })
    }

    /// Bounds both declared lengths and streamed bodies, including redirects.
    async fn fetch(&mut self, url: &Url, limit: usize) -> Result<Resource> {
        validate_url(url)?;
        let remaining = self
            .deadline
            .saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            bail!("URL import exceeded its two-minute download limit");
        }
        let mut response = self
            .client
            .get(url.clone())
            .timeout(remaining.min(Duration::from_secs(20)))
            .send()
            .await
            .with_context(|| format!("failed to fetch {url}"))?
            .error_for_status()
            .with_context(|| format!("failed to fetch {url}"))?;
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            bail!("resource exceeds the {limit}-byte import limit: {url}");
        }
        let final_url = response.url().clone();
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .with_context(|| format!("failed to read {url}"))?
        {
            if bytes.len() + chunk.len() > limit
                || self.received_bytes + chunk.len() > MAX_TOTAL_BYTES
            {
                bail!("download exceeds the per-file or total URL import limit: {url}");
            }
            self.received_bytes += chunk.len();
            bytes.extend_from_slice(&chunk);
        }
        Ok(Resource {
            url: final_url,
            mime,
            bytes,
        })
    }

    fn text(&mut self, resource: Resource) -> Result<String> {
        self.text_bytes += resource.bytes.len();
        if self.text_bytes > MAX_TEXT_BYTES {
            bail!("page source exceeds the {MAX_TEXT_BYTES}-byte capture limit");
        }
        String::from_utf8(resource.bytes)
            .with_context(|| format!("URL import requires UTF-8 text: {}", resource.url))
    }
}

/// Resolves dependencies against their own document or stylesheet URL. Inline
/// data and fragment-only SVG references remain inline and are never fetched.
fn reference(base: &Url, value: &str, kind: ResourceKind) -> Result<Option<Reference>> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('#') || value.starts_with("data:") {
        return Ok(None);
    }
    let mut url = base
        .join(value)
        .with_context(|| format!("invalid resource URL {value:?} in {base}"))?;
    validate_url(&url)?;
    url.set_fragment(None);
    Ok(Some(Reference { url, kind }))
}

fn html_references(html: &str, document_url: &Url) -> Result<(Url, VecDeque<Reference>)> {
    let document = kuchiki::parse_html().one(html);
    let base = document
        .select_first("base[href]")
        .ok()
        .and_then(|node| node.attributes.borrow().get("href").map(str::to_owned))
        .map(|href| document_url.join(&href))
        .transpose()?
        .unwrap_or_else(|| document_url.clone());
    validate_url(&base)?;
    let mut refs = VecDeque::new();
    for node in document
        .select("link[href]")
        .map_err(|()| anyhow!("invalid internal link selector"))?
    {
        let attrs = node.attributes.borrow();
        let rel = attrs.get("rel").unwrap_or_default().to_ascii_lowercase();
        let tokens: Vec<_> = rel.split_ascii_whitespace().collect();
        let kind = if tokens.contains(&"stylesheet")
            || (tokens.contains(&"preload") && attrs.get("as") == Some("style"))
        {
            Some(ResourceKind::Stylesheet)
        } else if tokens.contains(&"modulepreload")
            || (tokens.contains(&"preload") && attrs.get("as") == Some("script"))
        {
            Some(ResourceKind::Script)
        } else if tokens.contains(&"icon")
            || tokens.contains(&"apple-touch-icon")
            || (tokens.contains(&"preload") && matches!(attrs.get("as"), Some("font" | "image")))
        {
            Some(ResourceKind::Asset)
        } else {
            None
        };
        if let Some(kind) = kind {
            refs.extend(reference(
                &base,
                attrs.get("href").unwrap_or_default(),
                kind,
            )?);
        }
    }
    for (selector, attribute, kind) in [
        ("script[src]", "src", ResourceKind::Script),
        (
            "img[src], source[src], video[src], input[type=image][src]",
            "src",
            ResourceKind::Asset,
        ),
        ("video[poster]", "poster", ResourceKind::Asset),
        (
            "meta[property='og:image'], meta[name='twitter:image']",
            "content",
            ResourceKind::Asset,
        ),
        ("image[href], use[href]", "href", ResourceKind::Asset),
    ] {
        for node in document
            .select(selector)
            .map_err(|()| anyhow!("invalid internal resource selector"))?
        {
            let attrs = node.attributes.borrow();
            refs.extend(reference(
                &base,
                attrs.get(attribute).unwrap_or_default(),
                kind,
            )?);
        }
    }
    // A data URL can contain commas. Leave an all-data srcset inline rather than
    // interpreting parts of its payload as remote paths.
    for node in document
        .select("img[srcset], source[srcset]")
        .map_err(|()| anyhow!("invalid internal srcset selector"))?
    {
        let attrs = node.attributes.borrow();
        let srcset = attrs.get("srcset").unwrap_or_default();
        if srcset.trim_start().starts_with("data:") {
            continue;
        }
        for candidate in srcset
            .split(',')
            .filter_map(|part| part.split_whitespace().next())
        {
            refs.extend(reference(&base, candidate, ResourceKind::Asset)?);
        }
    }
    for node in document
        .select("style, [style]")
        .map_err(|()| anyhow!("invalid internal style selector"))?
    {
        let attrs = node.attributes.borrow();
        refs.extend(css_references(
            attrs.get("style").unwrap_or_default(),
            &base,
        )?);
        if node.name.local.as_ref() == "style" {
            refs.extend(css_references(&node.as_node().text_contents(), &base)?);
        }
    }
    for node in document
        .select("script:not([src])")
        .map_err(|()| anyhow!("invalid internal script selector"))?
    {
        refs.extend(script_references(&node.as_node().text_contents(), &base)?);
    }
    Ok((base, refs))
}

fn css_references(css: &str, base: &Url) -> Result<Vec<Reference>> {
    let imports = Regex::new(
        r#"(?i)@import\s+(?:url\(\s*(?:\"([^\"]+)\"|'([^']+)'|([^\s\)]+))\s*\)|\"([^\"]+)\"|'([^']+)')[^;]*;"#,
    )?;
    let urls = Regex::new(r#"(?i)url\(\s*(?:\"([^\"]*)\"|'([^']*)'|([^\s\)]+))\s*\)"#)?;
    let mut refs = Vec::new();
    // Mask @import rules so their URL isn't also interpreted as a media file.
    for capture in imports.captures_iter(css) {
        let value = (1..=5)
            .find_map(|index| capture.get(index))
            .context("CSS import has no URL")?;
        refs.extend(reference(base, value.as_str(), ResourceKind::Stylesheet)?);
    }
    let remainder = imports.replace_all(css, "");
    for capture in urls.captures_iter(&remainder) {
        if let Some(value) = capture
            .get(1)
            .or_else(|| capture.get(2))
            .or_else(|| capture.get(3))
        {
            refs.extend(reference(base, value.as_str(), ResourceKind::Asset)?);
        }
    }
    Ok(refs)
}

fn script_references(script: &str, base: &Url) -> Result<Vec<Reference>> {
    let mut refs = Vec::new();
    for value in super::javascript::strings(script) {
        let Some(url) = script_url(base, &value) else {
            continue;
        };
        let path = url.path().to_ascii_lowercase();
        let kind = if path.ends_with(".js") || path.ends_with(".mjs") {
            ResourceKind::Script
        } else if path.ends_with(".css") {
            ResourceKind::Stylesheet
        } else if [
            ".png", ".jpg", ".jpeg", ".webp", ".gif", ".svg", ".avif", ".ico", ".woff", ".woff2",
            ".ttf", ".otf", ".mp4", ".webm", ".ogv",
        ]
        .iter()
        .any(|extension| path.ends_with(extension))
        {
            ResourceKind::Asset
        } else {
            continue;
        };
        refs.push(Reference { url, kind });
    }
    Ok(refs)
}

/// Bundler preload tables use origin-relative paths without a slash, whereas
/// ES module imports use ./ or ../. Keep that distinction when relocating them.
pub(super) fn script_url(base: &Url, value: &str) -> Option<Url> {
    if value.contains([' ', '\n', '\r', '\t', '\"', '\'', '`', '{', '}', '\\'])
        || value.is_empty()
        || (value.contains(':') && !value.starts_with("http://") && !value.starts_with("https://"))
    {
        return None;
    }
    let mut url = if value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with('/')
        || value.contains("://")
    {
        base.join(value).ok()?
    } else if value.contains('/') {
        base.join(&format!("/{value}")).ok()?
    } else {
        return None;
    };
    if validate_url(&url).is_err() {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

/// Extensions are checked as well as MIME types: many hosts serve WOFF files
/// as application/octet-stream. Font signatures take precedence over the name.
fn asset_type(resource: &Resource) -> Result<(AssetKind, &'static str)> {
    let bytes = &resource.bytes;
    if resource.mime.contains("html") {
        bail!("received HTML instead of an asset: {}", resource.url);
    }
    for (signature, extension) in [
        (b"wOFF".as_slice(), "woff"),
        (b"wOF2".as_slice(), "woff2"),
        (b"OTTO".as_slice(), "otf"),
        (b"\0\x01\0\0".as_slice(), "ttf"),
    ] {
        if bytes.starts_with(signature) {
            return Ok((AssetKind::Font, extension));
        }
    }
    let extension = Path::new(resource.url.path())
        .extension()
        .and_then(|part| part.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let guessed = mime_guess::from_path(resource.url.path())
        .first_raw()
        .unwrap_or_default();
    let mime = if resource.mime.is_empty() || resource.mime == "application/octet-stream" {
        guessed
    } else {
        &resource.mime
    };
    let result = match (mime, extension.as_str()) {
        ("image/png", _) | (_, "png") => (AssetKind::Image, "png"),
        ("image/jpeg", _) | (_, "jpg" | "jpeg") => (AssetKind::Image, "jpg"),
        ("image/webp", _) | (_, "webp") => (AssetKind::Image, "webp"),
        ("image/gif", _) | (_, "gif") => (AssetKind::Image, "gif"),
        ("image/avif", _) | (_, "avif") => (AssetKind::Image, "avif"),
        ("image/x-icon" | "image/vnd.microsoft.icon", _) | (_, "ico") => (AssetKind::Image, "ico"),
        ("image/bmp", _) | (_, "bmp") => (AssetKind::Image, "bmp"),
        ("image/svg+xml", _) | (_, "svg") => (AssetKind::Svg, "svg"),
        ("font/woff", _) | (_, "woff") => (AssetKind::Font, "woff"),
        ("font/woff2", _) | (_, "woff2") => (AssetKind::Font, "woff2"),
        ("font/ttf", _) | (_, "ttf") => (AssetKind::Font, "ttf"),
        ("font/otf", _) | (_, "otf") => (AssetKind::Font, "otf"),
        ("video/mp4", _) | (_, "mp4") => (AssetKind::Video, "mp4"),
        ("video/webm", _) | (_, "webm") => (AssetKind::Video, "webm"),
        ("video/ogg", _) | (_, "ogv") => (AssetKind::Video, "ogv"),
        _ => bail!(
            "unsupported asset type {} at {}",
            resource.mime,
            resource.url
        ),
    };
    Ok(result)
}
