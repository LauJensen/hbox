use std::{fs, path::Path};

use anyhow::{anyhow, Context, Result};
use chrono::NaiveDate;
use gray_matter::{engine::YAML, Matter};
use pulldown_cmark::{html, Options, Parser};

use crate::models::{Document, FrontMatter, PostMeta};
use crate::rendering::highlighting::{
    highlight_code_in_html,
    CodeTheme,
};

use minijinja::{
    Value,
};

#[derive(Debug, Clone, Copy)]
pub enum DocumentKind {
// TOOD: Will pages benefit from Metadata?
//    Page,
    Blogpost,
}

pub fn read_markdown_document(
    path: &Path,
    kind: DocumentKind,
    default_language: &str,
) -> Result<Option<Document>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed to read markdown file: {}", path.display()))?;

    let matter = Matter::<YAML>::new();

    let parsed = matter
        .parse::<FrontMatter>(&raw)
        .with_context(|| format!("Failed to parse metadata in {}", path.display()))?;

    let frontmatter = parsed
        .data
        .clone()
        .ok_or_else(|| anyhow!("Missing metadata in {}", path.display()))?;

    if frontmatter.draft {
        return Ok(None);
    }

    let file_stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("Invalid markdown filename: {}", path.display()))?;

    let slug = frontmatter
        .slug
        .unwrap_or_else(|| file_stem.to_string());

    let language = frontmatter
        .language
        .unwrap_or_else(|| default_language.to_string());

    let description = frontmatter
        .description
        .unwrap_or_default();

    let date = parse_frontmatter_date(frontmatter.date.as_deref(), path)?;

    let template = frontmatter.template.unwrap_or_else(|| match kind {
//        DocumentKind::Page => "templates/page.html".to_string(),
        DocumentKind::Blogpost => "templates/blogpost.html".to_string(),
    });

    let url = match kind {
//        DocumentKind::Page => page_url(&slug),
        DocumentKind::Blogpost => PostMeta::build_url(
            &language,
            trim_slashes(&slug),
        ),
    };

    let meta = PostMeta {
        title: frontmatter.title,
        slug,
        language,
        description,
        url,
        externals: frontmatter.externals,
        date,
        featured_image: frontmatter.image,
    };

    Ok(Some(Document {
        meta,
        template,
        markdown: parsed.content,
        code_theme: frontmatter.code_theme,
        source_path: path.to_path_buf(),
    }))
}

fn parse_frontmatter_date(
    value: Option<&str>,
    path: &Path,
) -> Result<Option<NaiveDate>> {
    value
        .map(|date| {
            NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .with_context(|| {
                    format!(
                        "Invalid date in {}; expected YYYY-MM-DD, got '{}'",
                        path.display(),
                        date,
                    )
                })
        })
        .transpose()
}

pub fn render_markdown(
    markdown: &str,
    theme: CodeTheme,
) -> Result<String> {
    let html = markdown_to_html(markdown);
    highlight_code_in_html(theme, &html)
}

pub(crate) fn markdown_to_html(markdown: &str) -> String {
    let mut options = Options::empty();

    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(markdown, options);

    let mut output = String::new();
    html::push_html(&mut output, parser);

    output
}

//TODO: Re-enable DocumentKind::Page ?
//fn page_url(slug: &str) -> String {
//    let slug = trim_slashes(slug);

//    if slug.is_empty() || slug == "index" {
//        "/".to_string()
//    } else {
//        format!("/{slug}/")
//    }
//}

fn trim_slashes(value: &str) -> &str {
    value.trim_matches('/')
}

pub fn add_markdown_filter(
    env: &mut minijinja::Environment<'_>,
    theme: CodeTheme,
) {
    env.add_filter(
        "markdown",
        move |markdown: &str| -> Result<Value, minijinja::Error> {
            let markdown = textwrap::dedent(markdown);

            let html = render_markdown(markdown.trim(), theme)
                .map_err(|error| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        format!("failed to render Markdown: {error:#}"),
                    )
                })?;

            Ok(Value::from_safe_string(html))
        },
    );
}
