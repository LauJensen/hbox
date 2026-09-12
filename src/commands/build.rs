use std::{
    ffi::OsStr,
    collections::HashSet,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use chrono::NaiveDate;
use kuchiki::traits::TendrilSink;
use lightningcss::{
    printer::PrinterOptions,
    stylesheet::{MinifyOptions, ParserOptions, StyleSheet},
};
use minijinja::{context, Value};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{
    publish,
    config::{load_config, SiteConfig, ResolvedSite},
    cli::BuildArgs,
    models::{Document, PostSummary},
    rendering::markdown::{read_markdown_document,
                          render_markdown,
                          DocumentKind,
                          add_markdown_filter},
    templates::load_templates,
};

const SITE_CONF_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/nginx/site.conf"
));

#[derive(Debug)]
struct SiteStylesheets {
    global_href: String,
    design_href: Option<String>,
}

#[derive(Debug)]
pub struct BuildReport {
    pub output_dir: PathBuf,
    pub pages_built: usize,
    pub blogposts_built: usize,
}

pub async fn run(args: BuildArgs) -> Result<()> {
    let site = ResolvedSite::resolve(&args.site)?;
    let report = build_site(&site)?;

    println!(
        "Built {} pages and {} blogposts into {}",
        report.pages_built,
        report.blogposts_built,
        report.output_dir.display(),
    );

    Ok(())
}

/// Builds a configured site into `dist/<site-name>` and reports generated content counts.
pub fn build_site(site: &ResolvedSite) -> Result<BuildReport> {
    let start_time = std::time::Instant::now();

    let site_dir = &site.source_dir();
    let output_dir = site.output_staging_dir();

    validate_site_dir(site_dir)?;
    validate_html_sources(site_dir)?;

    let config = load_config(site_dir)?;

    clean_output_dir(&output_dir)?;

    let pages_dir     = site_dir.join("pages");
    let blogposts_dir = site_dir.join("blogposts");
    let public_dir    = site_dir.join("public");

    copy_public_assets(&public_dir, &output_dir)?;

    let stylesheets =
        build_site_stylesheets(site_dir, &output_dir)?;

    let mut env = load_templates(site_dir)?;

    add_markdown_filter(
        &mut env,
        config.site.code_theme
    );

    let (blogposts_built, blogpost_summaries) = build_blogposts(
        &env,
        site_dir,
        &blogposts_dir,
        &config,
        &output_dir,
        &stylesheets,
    )?;

    let pages_built = render_html_pages(
        &env,
        site_dir,
        &pages_dir,
        &config,
        &output_dir,
        &blogpost_summaries,
        &stylesheets,
    )?;

    if config.nginx.is_enabled() {
        write_nginx_conf(&site, &config)?;
    }

    publish::commit(site)?;

    eprintln!("Site built in: {:?}", start_time.elapsed());

    Ok(BuildReport {
        output_dir: site.output_dir().to_path_buf(),
        pages_built,
        blogposts_built,
    })
}

fn write_nginx_conf(
    site:   &ResolvedSite,
    config: &SiteConfig,
) -> Result<()> {
    let domains = config.nginx.domains_as_str();
    let access_log = config
        .nginx
        .access_log_directive(&site.site_name);

    let site_nginx_conf = SITE_CONF_TEMPLATE
        .replace("%DOMAINS%", &domains)
        .replace("%SITENAME%", &site.site_name)
        .replace("%ACCESS_LOG%", &access_log);

    let conf_file_path = site
        .output_staging_dir()
        .join("nginx.conf");

    std::fs::write(&conf_file_path, site_nginx_conf)
        .with_context(|| {
            format!(
                "failed to write nginx configuration {}",
                conf_file_path.display()
            )
        })
}

/// Verifies the site root and its required configuration and stylesheet inputs.
fn validate_site_dir(site_dir: &Path) -> Result<()> {
    if !site_dir.exists() {
        bail!("Site directory does not exist: {}", site_dir.display());
    }

    if !site_dir.is_dir() {
        bail!("Site path is not a directory: {}", site_dir.display());
    }

    let config_path = site_dir.join("hbox.toml");

    if !config_path.is_file() {
        bail!(
            "Site directory is missing hbox.toml: {}",
            site_dir.display()
        );
    }

    let global_css = site_dir.join("global.css");

    if !global_css.is_file() {
        bail!(
            "Site directory is missing required global.css: {}",
            site_dir.display()
        );
    }

    let design_css = site_dir.join("design.css");

    if design_css.exists() && !design_css.is_file() {
        bail!(
            "design.css exists but is not a file: {}",
            design_css.display()
        );
    }

    Ok(())
}

/// Rejects source HTML that attempts to reference Hbox-managed stylesheets.
fn validate_html_sources(site_dir: &Path) -> Result<()> {
    for entry in WalkDir::new(site_dir)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| entry.file_name() != OsStr::new(".hbox"))
    {
        let entry = entry.with_context(|| {
            format!(
                "Failed while walking site directory: {}",
                site_dir.display()
            )
        })?;

        if !entry.file_type().is_file() {
            continue;
        }

        let source = entry.path();

        if source.extension().and_then(|value| value.to_str()) != Some("html") {
            continue;
        }

        let html = fs::read_to_string(source)
            .with_context(|| format!("Failed to read HTML source {}", source.display()))?;

        validate_no_managed_stylesheets(&html)
            .with_context(|| format!("Invalid HTML source {}", source.display()))?;
    }

    Ok(())
}

/// Recreates the output directory so stale build artifacts cannot survive.
fn clean_output_dir(output_dir: &Path) -> Result<()> {
    if output_dir.exists() {
        fs::remove_dir_all(output_dir).with_context(|| {
            format!(
                "Failed to remove previous output directory: {}",
                output_dir.display()
            )
        })?;
    }

    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    Ok(())
}

/// Reads all Markdown documents of the requested kind from a content directory.
fn read_documents(
    dir: &Path,
    kind: DocumentKind,
    default_language: &str,
) -> Result<Vec<Document>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    if !dir.is_dir() {
        bail!(
            "Content path exists but is not a directory: {}",
            dir.display()
        );
    }

    let mut documents = Vec::new();

    for entry in WalkDir::new(dir).sort_by_file_name() {
        let entry = entry.with_context(|| {
            format!("Failed while walking content directory: {}", dir.display())
        })?;

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }

        if let Some(document) =
            read_markdown_document(path, kind, default_language)?
        {
            documents.push(document);
        }
    }

    Ok(documents)
}

/// Renders one Markdown document, injects managed CSS, and writes its output page.
fn render_document(
    env: &minijinja::Environment<'_>,
    config: &SiteConfig,
    document: &Document,
    posts: &[PostSummary],
    output_dir: &Path,
    stylesheets: &SiteStylesheets,
) -> Result<()> {
    let template = env.get_template(&document.template).with_context(|| {
        format!(
            "Template '{}' not found for {}",
            document.template,
            document.source_path.display()
        )
    })?;

    let formatted_posts: Vec<Value> = posts
        .iter()
        .map(|post| post.as_template_context(&config.site.date_format))
        .collect();

    let code_theme = document
        .code_theme
        .unwrap_or(config.site.code_theme);

    let content = render_markdown(&document.markdown, code_theme).with_context(|| {
        format!(
            "Failed to render Markdown from {}",
            document.source_path.display()
        )
    })?;

    let page = document_template_context(
        document,
        &config.site.date_format,
    );

    let rendered = template
        .render(context! {
            site        => config.site,
            page        => page,
            title       => document.meta.title,
            description => document.meta.description,
            language    => document.meta.language,
            slug        => document.meta.slug,
            url         => document.meta.url,
            content     => Value::from_safe_string(content),
            posts       => formatted_posts,
        })
        .with_context(|| {
            format!(
                "Failed to render {} using template '{}'",
                document.source_path.display(),
                document.template
            )
        })?;

    validate_html_document(&rendered).with_context(|| {
        format!(
            "Invalid rendered document generated from {}",
            document.source_path.display()
        )
    })?;

    let rendered = inject_site_stylesheets(&rendered, stylesheets)?;
    let rendered = add_externals(&rendered, &document.meta.externals)?;

    let output_file = output_file_for_post(output_dir, document.url())?;

    if let Some(parent) = output_file.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create output directory: {}", parent.display())
        })?;
    }

    fs::write(&output_file, rendered)
        .with_context(|| format!("Failed to write file: {}", output_file.display()))?;

    println!("Wrote blog post: {}", output_file.display());

    Ok(())
}

/// Maps a validated document URL to its directory-style `index.html` output path.
fn output_file_for_post(output_dir: &Path, url: &str) -> Result<PathBuf> {
    if url.contains(|character| matches!(character, '?' | '#' | '\\')) {
        bail!("Invalid document URL: {url}");
    }

    let clean = url.trim_matches('/');

    if clean.is_empty() {
        return Ok(output_dir.join("index.html"));
    }

    let mut relative = PathBuf::new();

    for segment in clean.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") {
            bail!("Invalid document URL: {url}");
        }

        relative.push(segment);
    }

    Ok(output_dir.join(relative).join("index.html"))
}

/// Copies public assets while protecting filenames reserved for generated CSS.
fn copy_public_assets(public_dir: &Path, output_dir: &Path) -> Result<()> {
    if !public_dir.exists() {
        return Ok(());
    }

    if !public_dir.is_dir() {
        bail!(
            "Public path exists but is not a directory: {}",
            public_dir.display()
        );
    }

    for entry in WalkDir::new(public_dir).sort_by_file_name() {
        let entry = entry.with_context(|| {
            format!(
                "Failed while walking public directory: {}",
                public_dir.display()
            )
        })?;

        let source = entry.path();

        let relative = source.strip_prefix(public_dir).with_context(|| {
            format!(
                "Failed to make public asset path relative: {}",
                source.display()
            )
        })?;

        if entry.file_type().is_file() {
            validate_public_asset_path(relative)?;
        }

        let target = output_dir.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).with_context(|| {
                format!("Failed to create asset directory: {}", target.display())
            })?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create asset directory: {}", parent.display())
                })?;
            }

            fs::copy(source, &target).with_context(|| {
                format!(
                    "Failed to copy asset {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
        }
    }

    Ok(())
}

/// Rejects public assets whose root filenames belong to Hbox's CSS pipeline.
fn validate_public_asset_path(relative: &Path) -> Result<()> {
    if relative == Path::new("global.css")
        || relative == Path::new("design.css")
    {
        bail!(
            "Public asset {} uses a filename managed by Hbox",
            relative.display()
        );
    }

    let is_root_file = relative.parent().is_some_and(|parent| parent.as_os_str().is_empty());

    let is_hashed_design_css = is_root_file
        && relative
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("design.") && name.ends_with(".css"));

    if is_hashed_design_css {
        bail!(
            "Public asset {} uses a filename reserved by Hbox",
            relative.display()
        );
    }

    Ok(())
}

/// Ensures no two documents declare the same output URL.
fn validate_unique_urls(documents: &[Document]) -> Result<()> {
    let mut urls = HashSet::new();

    for document in documents {
        if !urls.insert(document.meta.url.clone()) {
            bail!(
                "Duplicate output URL '{}' found at {}",
                document.meta.url,
                document.source_path.display()
            );
        }
    }

    Ok(())
}

/// Renders page templates, injects shared CSS, and appends optional page CSS.
fn render_html_pages(
    env: &minijinja::Environment<'_>,
    site_dir: &Path,
    pages_dir: &Path,
    config: &SiteConfig,
    output_dir: &Path,
    posts: &[PostSummary],
    stylesheets: &SiteStylesheets,
) -> Result<usize> {
    if !pages_dir.exists() {
        return Ok(0);
    }

    if !pages_dir.is_dir() {
        bail!(
            "Pages path exists but is not a directory: {}",
            pages_dir.display()
        );
    }

    let formatted_posts: Vec<Value> = posts
        .iter()
        .map(|post| post.as_template_context(&config.site.date_format))
        .collect();

    let mut pages_built = 0;

    for entry in WalkDir::new(pages_dir).sort_by_file_name() {
        let entry = entry.with_context(|| {
            format!(
                "Failed while walking pages directory: {}",
                pages_dir.display()
            )
        })?;

        if !entry.file_type().is_file() {
            continue;
        }

        let source = entry.path();

        if source.extension().and_then(|value| value.to_str()) != Some("html") {
            continue;
        }

        let template_name = source
            .strip_prefix(site_dir)
            .with_context(|| {
                format!(
                    "Failed to make page template path relative: {}",
                    source.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");

        let template = env.get_template(&template_name).with_context(|| {
            format!("Failed to load page template '{template_name}'")
        })?;

        let rendered = template
            .render(context! {
                site => &config.site,
                posts => formatted_posts,
            })
            .with_context(|| format!("Failed to render page {}", source.display()))?;

        validate_html_document(&rendered)
            .with_context(|| format!("Invalid rendered page {}", source.display()))?;

        let target =
            output_file_for_html_page(output_dir, pages_dir, source, &rendered)?;

        let rendered = inject_site_stylesheets(&rendered, stylesheets)?;
        let css_source = source.with_extension("css");

        let final_html = if css_source.is_file() {
            println!("Page {} has custom CSS", source.display());
            // fs::read_to_string(&css_source).with_context(|| {
            let page_css = minify_stylesheet_source(&css_source)?;

            inline_page_css(&rendered, &page_css)?
        } else if css_source.exists() {
            bail!(
                "Page CSS path exists but is not a file: {}",
                css_source.display()
            );
        } else {
            rendered
        };

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create page directory: {}", parent.display())
            })?;
        }

        fs::write(&target, final_html)
            .with_context(|| format!("Failed to write page {}", target.display()))?;

        pages_built += 1;
    }

    Ok(pages_built)
}

/// Minifies site CSS and emits the optional design stylesheet under a content hash.
fn build_site_stylesheets(
    site_dir: &Path,
    output_dir: &Path,
) -> Result<SiteStylesheets> {
    let global_source = site_dir.join("global.css");
    let global_target = output_dir.join("global.css");

    minify_stylesheet(&global_source, &global_target)?;

    let design_source = site_dir.join("design.css");

    let design_href = if design_source.is_file() {
        let minified = minify_stylesheet_source(&design_source)?;

        if minified.trim().is_empty() {
            None
        } else {
            let hash = content_hash(&minified);
            let filename = format!("design.{hash}.css");
            let target = output_dir.join(&filename);

            fs::write(&target, minified).with_context(|| {
                format!(
                    "Failed to write minified stylesheet {}",
                    target.display()
                )
            })?;

            Some(format!("/{filename}"))
        }
    } else {
        None
    };

    Ok(SiteStylesheets {
        global_href: "/global.css".to_string(),
        design_href,
    })
}

/// Minifies a stylesheet and writes it to the requested output path.
fn minify_stylesheet(
    source_path: &Path,
    target_path: &Path,
) -> Result<()> {
    let minified = minify_stylesheet_source(source_path)?;

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create stylesheet output directory {}",
                parent.display()
            )
        })?;
    }

    fs::write(target_path, minified).with_context(|| {
        format!(
            "Failed to write minified stylesheet {}",
            target_path.display()
        )
    })?;

    Ok(())
}

/// Parses and minifies a stylesheet, returning its production CSS.
fn minify_stylesheet_source(css_path: &Path) -> Result<String> {
    let source = fs::read_to_string(css_path)
        .with_context(|| format!("Failed to read stylesheet {}", css_path.display()))?;

    let mut stylesheet = StyleSheet::parse(
        &source,
        ParserOptions {
            filename: css_path.to_string_lossy().into_owned(),
            ..ParserOptions::default()
        },
    )
    .map_err(|error| {
        anyhow!(
            "Failed to parse stylesheet {}: {}",
            css_path.display(),
            error
        )
    })?;

    stylesheet
        .minify(MinifyOptions::default())
        .map_err(|error| {
            anyhow!(
                "Failed to minify stylesheet {}: {}",
                css_path.display(),
                error
            )
        })?;

    stylesheet
        .to_css(PrinterOptions {
            minify: true,
            ..PrinterOptions::default()
        })
        .map(|result| result.code)
        .map_err(|error| {
            anyhow!(
                "Failed to print minified stylesheet {}: {}",
                css_path.display(),
                error
            )
        })
}

fn content_hash(content: &str) -> String {
    Sha256::digest(content.as_bytes())[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Loads, orders, summarizes, and renders all published blog posts.
fn build_blogposts(
    env: &minijinja::Environment<'_>,
    site_dir: &Path,
    blogposts_dir: &Path,
    config: &SiteConfig,
    output_dir: &Path,
    stylesheets: &SiteStylesheets,
) -> Result<(usize, Vec<PostSummary>)> {
    let templates_dir = site_dir.join("templates");

    if !blogposts_dir.exists() {
        return Ok((0, Vec::new()));
    }

    if !blogposts_dir.is_dir() {
        bail!(
            "Blogposts path exists but is not a directory: {}",
            blogposts_dir.display()
        );
    }

    if !templates_dir.is_dir() {
        bail!(
            "Blogposts exist, but templates directory is missing: {}",
            templates_dir.display()
        );
    }

    let blogposts = read_documents(
        blogposts_dir,
        DocumentKind::Blogpost,
        &config.site.default_language,
    )?;

    if blogposts.is_empty() {
        return Ok((0, Vec::new()));
    }

    validate_unique_urls(&blogposts)?;

    let mut post_summaries: Vec<PostSummary> =
        blogposts.iter().map(PostSummary::from).collect();

    post_summaries.sort_by(|left, right| match (left.date, right.date) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    for post in &blogposts {
        render_document(
            &env,
            config,
            post,
            &post_summaries,
            output_dir,
            stylesheets,
        )?;
    }

    Ok((blogposts.len(), post_summaries))
}

/// Maps a page source to its language-prefixed directory-style output path.
fn output_file_for_html_page(
    output_dir: &Path,
    pages_dir: &Path,
    source: &Path,
    rendered_html: &str,
) -> Result<PathBuf> {
    let relative = source.strip_prefix(pages_dir).with_context(|| {
        format!(
            "Failed to make page path relative: {}",
            source.display()
        )
    })?;

    let language = html_lang(rendered_html).with_context(|| {
        format!(
            "Page {} must contain an <html> element with a valid lang attribute",
            source.display()
        )
    })?;

    if relative == Path::new("index.html") {
        return Ok(output_dir.join("index.html"));
    }

    let relative_without_extension = relative.with_extension("");

    Ok(output_dir
        .join(language)
        .join(relative_without_extension)
        .join("index.html"))
}

/// Verifies the structural HTML requirements needed by the build pipeline.
fn validate_html_document(html: &str) -> Result<()> {
    html_lang(html)?;
    validate_no_managed_stylesheets(html)?;

    if head_end(html).is_none() {
        bail!("Document is missing </head>");
    }

    Ok(())
}

/// Extracts and validates the `lang` value from the document's `<html>` element.
fn html_lang(html: &str) -> Result<String> {
    let document = kuchiki::parse_html().one(html);
    let html_element = document
        .select_first("html")
        .map_err(|()| anyhow!("Document is missing an <html> element"))?;
    let attributes = html_element.attributes.borrow();
    let language = attributes
        .get("lang")
        .context("The <html> element is missing its lang attribute")?
        .to_string();

    validate_language_tag(&language)?;

    Ok(language)
}

/// Accepts path-safe language tags composed of alphanumeric subtags.
fn validate_language_tag(language: &str) -> Result<()> {
    if language.is_empty() {
        bail!("lang attribute cannot be empty");
    }

    if language.starts_with('-')
        || language.ends_with('-')
        || language.contains("--")
        || !language
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        bail!("Invalid language tag: {language}");
    }

    Ok(())
}

/// Inserts page-specific CSS at the end of `<head>` so it wins the cascade.
fn inline_page_css(html: &str, css: &str) -> Result<String> {
    let css = css.trim();

    if css.is_empty() {
        return Ok(html.to_string());
    }

    let head_end = head_end(html)
        .context("Cannot inject page CSS: missing </head>")?;

    let style = format!(
        "<style id=\"hbox-page-css\">{css}</style>\n"
    );

    let mut output = String::with_capacity(html.len() + style.len());
    output.push_str(&html[..head_end]);
    output.push_str(&style);
    output.push_str(&html[head_end..]);

    Ok(output)
}

/// Inserts global and optional design stylesheet links at the end of `<head>`.
fn inject_site_stylesheets(
    html: &str,
    stylesheets: &SiteStylesheets,
) -> Result<String> {
    let head_end = head_end(html)
        .context("Cannot inject stylesheets: missing </head>")?;

    let mut links = format!(
        "<link rel=\"stylesheet\" href=\"{}\">\n",
        stylesheets.global_href
    );

    if let Some(design_href) = &stylesheets.design_href {
        writeln!(
            links,
            "<link rel=\"stylesheet\" href=\"{design_href}\">"
        )
        .expect("writing to a String cannot fail");
    }

    let mut output = String::with_capacity(html.len() + links.len());
    output.push_str(&html[..head_end]);
    output.push_str(&links);
    output.push_str(&html[head_end..]);

    Ok(output)
}

/// Finds the closing head tag without imposing case rules beyond HTML itself.
fn head_end(html: &str) -> Option<usize> {
    html.as_bytes()
        .windows(b"</head>".len())
        .position(|window| window.eq_ignore_ascii_case(b"</head>"))
}

/// Rejects stylesheet links to `global.css` or `design.css` in user-owned HTML.
fn validate_no_managed_stylesheets(html: &str) -> Result<()> {
    let document = kuchiki::parse_html().one(html);
    let links = document
        .select("link")
        .map_err(|()| anyhow!("Failed to inspect <link> elements"))?;

    for link in links {
        let attributes = link.attributes.borrow();
        let is_stylesheet = attributes.get("rel").is_some_and(|value| {
            value
                .split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("stylesheet"))
        });

        if !is_stylesheet {
            continue;
        }

        let Some(href) = attributes.get("href") else {
            continue;
        };

        if let Some(filename) = managed_stylesheet_filename(href) {
            bail!(
                "HTML sources must not reference managed stylesheet \
                 '{filename}'; Hbox injects site stylesheet links"
            );
        }
    }

    Ok(())
}

/// Identifies managed stylesheet filenames after removing query and fragment suffixes.
fn managed_stylesheet_filename(href: &str) -> Option<&'static str> {
    let path = href
        .split_once('#')
        .map_or(href, |(path, _)| path);

    let path = path
        .split_once('?')
        .map_or(path, |(path, _)| path);

    let filename = path.rsplit('/').next()?;

    if filename.eq_ignore_ascii_case("global.css") {
        Some("global.css")
    } else if filename.eq_ignore_ascii_case("design.css") {
        Some("design.css")
    } else {
        None
    }
}

impl PostSummary {
    /// Formats the optional publication date with the site's Chrono format.
    pub fn formatted_date(&self, date_format: &str) -> Option<String> {
        self.date
            .map(|date| format_site_date(date, date_format))
    }

    /// Converts a summary into the stable value exposed to MiniJinja templates.
    pub fn as_template_context(&self, date_format: &str) -> Value {
        context! {
            title          => self.title.clone(),
            slug           => self.slug.clone(),
            language       => self.language.clone(),
            description    => self.description.clone(),
            url            => self.url.clone(),
            date           => self.formatted_date(date_format),
            featured_image => self.featured_image.clone(),
        }
    }
}

fn format_site_date(date: NaiveDate, site_format: &str) -> String {
    date.format(site_format).to_string()
}

fn add_externals(html: &str, externals: &[String]) -> Result<String> {
    if externals.is_empty() {
        return Ok(html.to_owned());
    }

    let body_end = html
        .rfind("</body>")
        .context("document has no closing </body> tag")?;

    let mut scripts = String::new();

    for src in externals {
        write!(&mut scripts, r#"<script src="{src}"></script>"#)?;
    }

    let mut rendered = html.to_owned();
    rendered.insert_str(body_end, &scripts);

    Ok(rendered)
}

fn document_template_context(
    document: &Document,
    date_format: &str,
) -> Value {
    context! {
        title          => document.meta.title.clone(),
        slug           => document.meta.slug.clone(),
        language       => document.meta.language.clone(),
        description    => document.meta.description.clone(),
        url            => document.meta.url.clone(),
        date           => document.meta.date.map(
            |date| date.format(date_format).to_string()
        ),
        featured_image => document.meta.featured_image.clone(),
        externals      => document.meta.externals.clone(),
        template       => document.template.clone(),
    }
}
