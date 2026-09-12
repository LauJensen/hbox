use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use kuchiki::{traits::TendrilSink, NodeRef};
use regex::Regex;
use reqwest::{Client, Url};
use walkdir::WalkDir;

use crate::cli::ValidateArgs;
use crate::utils::resolve_within;

use crate::config::ResolvedSite;

pub struct ValidationReport {
    pub errors: usize,
    pub pages_inspected: usize,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        self.errors == 0
    }
}

#[derive(Debug)]
pub struct FileReport {
    path: PathBuf,
    issues: Vec<Issue>,
}

#[derive(Debug, Clone)]
struct Issue {
    line: Option<usize>,
    message: String,
}

#[derive(Debug, Clone)]
struct ExternalReference {
    file_index: usize,
    line: Option<usize>,
    url: String,
}

#[derive(Debug, Clone, Copy)]
enum ReferenceKind {
    Link,
    Image,
    Resource,
}

impl ReferenceKind {
    fn missing_message(self, target: &str) -> String {
        match self {
            Self::Link => format!("missing link target: {target}"),
            Self::Image => format!("missing image: {target}"),
            Self::Resource => format!("missing resource: {target}"),
        }
    }
}

/// Validates the generated output for a site and prints a per-file report.
pub async fn run(args: ValidateArgs) -> Result<ValidationReport> {
    let start_time = std::time::Instant::now();
    let site = ResolvedSite::resolve(&args.site)?;

    let report = validate_site(&site.output_dir(), args.check_external_links).await?;
    eprintln!("Site checked in: {:?}", start_time.elapsed());

    Ok(report)
}

/// Checks generated HTML and CSS files beneath `dist_dir` for broken references.
pub async fn validate_site(
    dist_dir: &Path,
    check_external_links: bool,
) -> Result<ValidationReport> {
    let dist_dir = fs::canonicalize(dist_dir)
        .with_context(|| format!("Failed to open generated site: {}", dist_dir.display()))?;

    let mut html_files = generated_files_with_extension(&dist_dir, "html")?;
    let mut css_files = generated_files_with_extension(&dist_dir, "css")?;
    html_files.sort();
    css_files.sort();

    if html_files.is_empty() {
        bail!("No generated HTML files found in {}", dist_dir.display());
    }

    let mut reports = Vec::with_capacity(html_files.len() + css_files.len());
    let mut external_references = Vec::new();
    let mut id_cache = HashMap::new();

    for file in &html_files {
        let file_index = reports.len();
        let (issues, external) = validate_html_file(file, &dist_dir, file_index, &mut id_cache)?;

        reports.push(FileReport {
            path: file.clone(),
            issues,
        });
        external_references.extend(external);
    }

    for file in &css_files {
        let file_index = reports.len();
        let (issues, external) = validate_css_file(file, &dist_dir, file_index)?;

        reports.push(FileReport {
            path: file.clone(),
            issues,
        });
        external_references.extend(external);
    }

    if check_external_links {
        validate_external_references(&mut reports, external_references).await?;
    }

    let errors = reports
        .iter()
        .map(|report| report.issues.len())
        .sum();

    let pages_inspected = html_files.len();
    print_report(&dist_dir, &reports, errors, pages_inspected);

    Ok(ValidationReport {
        errors,
        pages_inspected,
    })
}

fn generated_files_with_extension(root: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_file() && has_extension(entry.path(), extension) => {
                Some(Ok(entry.into_path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error).with_context(|| format!("Failed to walk {}", root.display()))),
        })
        .collect()
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}

fn validate_html_file(
    file: &Path,
    dist_dir: &Path,
    file_index: usize,
    id_cache: &mut HashMap<PathBuf, HashSet<String>>,
) -> Result<(Vec<Issue>, Vec<ExternalReference>)> {
    let source = fs::read_to_string(file)
        .with_context(|| format!("Failed to read {}", file.display()))?;
    let document = kuchiki::parse_html().one(source.clone());
    let mut issues = Vec::new();
    let mut external = Vec::new();

    validate_document_language(&document, &source, &mut issues);
    validate_images(&document, &source, &mut issues);
    validate_anchors(&document, &source, &mut issues);

    let references = [
        ("a[href]", "href", ReferenceKind::Link),
        ("img[src]", "src", ReferenceKind::Image),
        ("script[src]", "src", ReferenceKind::Resource),
        ("link[href]", "href", ReferenceKind::Resource),
        ("source[src]", "src", ReferenceKind::Resource),
        ("video[src]", "src", ReferenceKind::Resource),
        ("video[poster]", "poster", ReferenceKind::Image),
        ("audio[src]", "src", ReferenceKind::Resource),
        ("track[src]", "src", ReferenceKind::Resource),
        ("iframe[src]", "src", ReferenceKind::Resource),
        ("embed[src]", "src", ReferenceKind::Resource),
        ("object[data]", "data", ReferenceKind::Resource),
        ("input[src]", "src", ReferenceKind::Image),
        ("image[href]", "href", ReferenceKind::Image),
        ("use[href]", "href", ReferenceKind::Resource),
    ];

    for (selector, attribute, kind) in references {
        let nodes = document
            .select(selector)
            .map_err(|()| anyhow::anyhow!("Invalid internal CSS selector: {selector}"))?;
        for node in nodes {
            let attributes = node.attributes.borrow();
            let Some(target) = attributes.get(attribute) else {
                continue;
            };
            let line = line_containing(&source, target);

            validate_reference(
                file,
                dist_dir,
                target,
                kind,
                line,
                file_index,
                &mut issues,
                &mut external,
                id_cache,
            )?;
        }
    }

    for selector in ["img[srcset]", "source[srcset]"] {
        let nodes = document
            .select(selector)
            .map_err(|()| anyhow::anyhow!("Invalid internal CSS selector: {selector}"))?;
        for node in nodes {
            let attributes = node.attributes.borrow();
            let Some(srcset) = attributes.get("srcset") else {
                continue;
            };

            for target in parse_srcset(srcset) {
                validate_reference(
                    file,
                    dist_dir,
                    target,
                    ReferenceKind::Image,
                    line_containing(&source, target),
                    file_index,
                    &mut issues,
                    &mut external,
                    id_cache,
                )?;
            }
        }
    }

    let style_nodes = document
        .select("style")
        .map_err(|()| anyhow::anyhow!("Invalid internal CSS selector: style"))?;
    for node in style_nodes {
        validate_inline_css(
            file,
            &source,
            &node.text_contents(),
            dist_dir,
            file_index,
            &mut issues,
            &mut external,
            id_cache,
        )?;
    }

    let styled_nodes = document
        .select("[style]")
        .map_err(|()| anyhow::anyhow!("Invalid internal CSS selector: [style]"))?;
    for node in styled_nodes {
        let attributes = node.attributes.borrow();
        if let Some(style) = attributes.get("style") {
            validate_inline_css(
                file,
                &source,
                style,
                dist_dir,
                file_index,
                &mut issues,
                &mut external,
                id_cache,
            )?;
        }
    }

    Ok((issues, external))
}

#[allow(clippy::too_many_arguments)]
fn validate_inline_css(
    file: &Path,
    html_source: &str,
    css_source: &str,
    dist_dir: &Path,
    file_index: usize,
    issues: &mut Vec<Issue>,
    external: &mut Vec<ExternalReference>,
    id_cache: &mut HashMap<PathBuf, HashSet<String>>,
) -> Result<()> {
    for target in css_references(css_source)? {
        validate_reference(
            file,
            dist_dir,
            target,
            ReferenceKind::Resource,
            line_containing(html_source, target),
            file_index,
            issues,
            external,
            id_cache,
        )?;
    }

    Ok(())
}

fn validate_document_language(document: &NodeRef, source: &str, issues: &mut Vec<Issue>) {
    let Some(html_line) = nth_tag_line(source, "html", 0) else {
        issues.push(Issue {
            line: None,
            message: "missing html element".to_owned(),
        });
        return;
    };

    match document.select_first("html") {
        Ok(html) => {
            let attributes = html.attributes.borrow();
            if attributes
                .get("lang")
                .map_or(true, |language| language.trim().is_empty())
            {
                issues.push(Issue {
                    line: Some(html_line),
                    message: "missing html lang attribute".to_owned(),
                });
            }
        }
        Err(_) => unreachable!("the source contains an html element"),
    }
}

fn validate_images(document: &NodeRef, source: &str, issues: &mut Vec<Issue>) {
    for (index, image) in document
        .select("img")
        .into_iter()
        .flatten()
        .enumerate()
    {
        let attributes = image.attributes.borrow();
        if attributes.get("alt").is_none() {
            issues.push(Issue {
                line: nth_tag_line(source, "img", index),
                message: "missing alt".to_owned(),
            });
        }

        if attributes.get("src").is_none() && attributes.get("srcset").is_none() {
            issues.push(Issue {
                line: nth_tag_line(source, "img", index),
                message: "missing image source".to_owned(),
            });
        }
    }
}

fn validate_anchors(document: &NodeRef, source: &str, issues: &mut Vec<Issue>) {
    for (index, anchor) in document
        .select("a")
        .into_iter()
        .flatten()
        .enumerate()
    {
        let attributes = anchor.attributes.borrow();
        if attributes
            .get("href")
            .map_or(true, |target| target.trim().is_empty())
        {
            issues.push(Issue {
                line: nth_tag_line(source, "a", index),
                message: "missing link target".to_owned(),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_reference(
    source_file: &Path,
    dist_dir: &Path,
    raw_target: &str,
    kind: ReferenceKind,
    line: Option<usize>,
    file_index: usize,
    issues: &mut Vec<Issue>,
    external: &mut Vec<ExternalReference>,
    id_cache: &mut HashMap<PathBuf, HashSet<String>>,
) -> Result<()> {
    let target = raw_target.trim();
    if target.is_empty() {
        return Ok(());
    }

    if let Some(url) = external_url(target) {
        external.push(ExternalReference {
            file_index,
            line,
            url,
        });
        return Ok(());
    }

    if has_non_http_scheme(target) {
        return Ok(());
    }

    let (path_part, fragment) = split_target(target);
    let target_file = if path_part.is_empty() {
        source_file.to_path_buf()
    } else {
        let Some(path) = local_target_path(source_file, dist_dir, path_part) else {
            issues.push(Issue {
                line,
                message: format!("reference escapes site root: {target}"),
            });
            return Ok(());
        };

        match existing_target(path, kind) {
            Some(path) => path,
            None => {
                issues.push(Issue {
                    line,
                    message: kind.missing_message(target),
                });
                return Ok(());
            }
        }
    };

    if let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) {
        if has_extension(&target_file, "html") {
            let ids = document_ids(&target_file, id_cache)?;
            if !ids.contains(fragment) {
                issues.push(Issue {
                    line,
                    message: format!("missing fragment target: {target}"),
                });
            }
        }
    }

    Ok(())
}

fn external_url(target: &str) -> Option<String> {
    if target.starts_with("//") {
        return Some(format!("https:{target}"));
    }

    Url::parse(target)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.to_string())
}

fn has_non_http_scheme(target: &str) -> bool {
    Url::parse(target)
        .map(|url| !matches!(url.scheme(), "http" | "https"))
        .unwrap_or(false)
}

fn split_target(target: &str) -> (&str, Option<&str>) {
    let (before_fragment, fragment) = target
        .split_once('#')
        .map_or((target, None), |(path, fragment)| (path, Some(fragment)));
    let path = before_fragment
        .split_once('?')
        .map_or(before_fragment, |(path, _)| path);

    (path, fragment)
}

/// Resolves a URL path lexically while preventing `..` from escaping `dist_dir`.
fn local_target_path(
    source_file: &Path,
    dist_dir: &Path,
    target: &str,
) -> Option<PathBuf> {
    let base = if target.starts_with('/') {
        dist_dir
    } else {
        source_file.parent()?
    };

    resolve_within(
        dist_dir,
        base,
        target,
    )
    .ok()
}

fn existing_target(path: PathBuf, kind: ReferenceKind) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path);
    }

    if matches!(kind, ReferenceKind::Link) {
        let index = path.join("index.html");
        if index.is_file() {
            return Some(index);
        }

        if path.extension().is_none() {
            let html = path.with_extension("html");
            if html.is_file() {
                return Some(html);
            }
        }
    }

    None
}

fn document_ids<'a>(
    file: &Path,
    cache: &'a mut HashMap<PathBuf, HashSet<String>>,
) -> Result<&'a HashSet<String>> {
    if !cache.contains_key(file) {
        let source = fs::read_to_string(file)
            .with_context(|| format!("Failed to read fragment target {}", file.display()))?;
        let document = kuchiki::parse_html().one(source);
        let ids = document
            .select("[id]")
            .into_iter()
            .flatten()
            .filter_map(|node| node.attributes.borrow().get("id").map(str::to_owned))
            .collect();
        cache.insert(file.to_path_buf(), ids);
    }

    Ok(cache.get(file).expect("ID cache was populated"))
}

fn parse_srcset(srcset: &str) -> impl Iterator<Item = &str> {
    srcset
        .split(',')
        .filter_map(|candidate| candidate.split_whitespace().next())
}

fn validate_css_file(
    file: &Path,
    dist_dir: &Path,
    file_index: usize,
) -> Result<(Vec<Issue>, Vec<ExternalReference>)> {
    let source = fs::read_to_string(file)
        .with_context(|| format!("Failed to read {}", file.display()))?;
    let references = css_references(&source)?;
    let mut issues = Vec::new();
    let mut external = Vec::new();
    let mut id_cache = HashMap::new();

    for target in references {
        validate_reference(
            file,
            dist_dir,
            target,
            ReferenceKind::Resource,
            line_containing(&source, target),
            file_index,
            &mut issues,
            &mut external,
            &mut id_cache,
        )?;
    }

    Ok((issues, external))
}

fn css_references(source: &str) -> Result<Vec<&str>> {
    let url_pattern = Regex::new(r#"(?i)url\(\s*['\"]?([^'\")]+)['\"]?\s*\)"#)?;
    let import_pattern = Regex::new(r#"(?i)@import\s+['\"]([^'\"]+)['\"]"#)?;

    Ok(url_pattern
        .captures_iter(source)
        .chain(import_pattern.captures_iter(source))
        .filter_map(|captures| captures.get(1).map(|value| value.as_str().trim()))
        .filter(|target| !target.starts_with('#'))
        .collect())
}

const MIN_EXTERNAL_BODY_BYTES: usize = 4;

/// Checks each distinct external URL once and attaches failures to every
/// referring file.
async fn validate_external_references(
    reports: &mut [FileReport],
    references: Vec<ExternalReference>,
) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("hbox/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Failed to create external-link HTTP client")?;

    let requested_range =
        format!("bytes=0-{}", MIN_EXTERNAL_BODY_BYTES - 1);

    let mut results: HashMap<String, Option<String>> = HashMap::new();

    for reference in &references {
        if results.contains_key(&reference.url) {
            continue;
        }

        let failure = match client
            .get(&reference.url)
            .header(
                reqwest::header::RANGE,
                requested_range.as_str(),
            )
            .send()
            .await
        {
            Ok(response) if !response.status().is_success() => {
                Some(format!(
                    "external link returned HTTP {}: {}",
                    response.status(),
                    reference.url,
                ))
            }

            Ok(mut response) => {
                match response_has_at_least_bytes(
                    &mut response,
                    MIN_EXTERNAL_BODY_BYTES,
                )
                .await
                {
                    Ok(true) => None,

                    Ok(false) => Some(format!(
                        "external link returned 3 bytes or fewer: {}",
                        reference.url,
                    )),

                    Err(error) => Some(format!(
                        "failed to read external response: {} ({error})",
                        reference.url,
                    )),
                }
            }

            Err(error) => Some(format!(
                "external link request failed: {} ({error})",
                reference.url,
            )),
        };

        results.insert(reference.url.clone(), failure);
    }

    for reference in references {
        if let Some(message) = results
            .get(&reference.url)
            .and_then(|failure| failure.clone())
        {
            reports[reference.file_index].issues.push(Issue {
                line: reference.line,
                message,
            });
        }
    }

    Ok(())
}

async fn response_has_at_least_bytes(
    response: &mut reqwest::Response,
    minimum: usize,
) -> reqwest::Result<bool> {
    let mut received = 0usize;

    while received < minimum {
        let Some(chunk) = response.chunk().await? else {
            return Ok(false);
        };

        received = received.saturating_add(chunk.len());
    }

    Ok(true)
}

fn nth_tag_line(source: &str, tag: &str, ordinal: usize) -> Option<usize> {
    let pattern = Regex::new(&format!(r"(?i)<{}\b", regex::escape(tag))).ok()?;
    let offset = pattern.find_iter(source).nth(ordinal)?.start();
    Some(line_number(source, offset))
}

fn line_containing(source: &str, needle: &str) -> Option<usize> {
    source.find(needle).map(|offset| line_number(source, offset))
}

fn line_number(source: &str, offset: usize) -> usize {
    source[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1
}

pub fn print_report(dist_dir: &Path, reports: &[FileReport], errors: usize, pages: usize) {
    for report in reports {
        let path = report.path.strip_prefix(dist_dir).unwrap_or(&report.path);
        if report.issues.is_empty() {
            println!("[ OK  ] {}", path.display());
            continue;
        }

        println!("[ ERR ] {}", path.display());
        for issue in &report.issues {
            match issue.line {
                Some(line) => println!("-> {} (line {line})", issue.message),
                None => println!("-> {}", issue.message),
            }
        }
    }

    if errors == 0 {
        println!("✔️ SITE VALIDATE: 0 errors, {pages} pages inspected");
    } else {
        println!("🗙 SITE INVALID: {errors} errors, {pages} pages inspected");
    }
}
