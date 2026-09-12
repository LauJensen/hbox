use std::path::PathBuf;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::rendering::highlighting::CodeTheme;

#[derive(Debug, Clone, Deserialize)]
pub struct FrontMatter {
    pub title: String,

    #[serde(default)]
    pub slug: Option<String>,

    #[serde(default)]
    pub language: Option<String>,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default)]
    pub date: Option<String>,

    #[serde(default)]
    pub template: Option<String>,

    #[serde(default)]
    pub image: Option<String>,

    #[serde(default)]
    pub draft: bool,

    #[serde(default)]
    pub externals: Vec<String>,

    #[serde(default)]
    pub code_theme: Option<CodeTheme>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PostMeta {
    pub title: String,
    pub slug: String,
    pub language: String,
    pub description: String,
    pub url: String,
    pub externals: Vec<String>,

    pub date: Option<NaiveDate>,

    pub featured_image: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Document {
    #[serde(flatten)]
    pub meta: PostMeta,

    pub template: String,

    #[serde(skip)]
    pub markdown: String,

    #[serde(skip)]
    pub code_theme: Option<CodeTheme>,

    #[serde(skip)]
    pub source_path: PathBuf,
}

pub type PostSummary = PostMeta;

impl PostMeta {
    pub fn build_url(language: &str, slug: &str) -> String {
        format!("/blog/{}/{}", language, slug)
    }
}

impl Document {
    pub fn url(&self) -> &str {
        &self.meta.url
    }
}

impl From<&Document> for PostSummary {
    fn from(doc: &Document) -> Self {
        doc.meta.clone()
    }
}
