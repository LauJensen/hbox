pub mod chatgpt;

use std::{
    num::NonZeroUsize,
    path::{PathBuf,Path},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone)]
pub struct AiConfig {
    pub api_key: String,
    pub api_base_url: String,
    pub model: String,
    pub image_model: String,
    pub image_quality: String,
    pub concurrency: NonZeroUsize,
}

impl AiConfig {
    pub fn from_env(concurrency: NonZeroUsize) -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY is not set")?;

        let api_base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

        let model = std::env::var("OPENAI_MODEL")
            .unwrap_or_else(|_| "gpt-5.1".to_string());

        let image_model = std::env::var("OPENAI_IMAGE_MODEL")
            .unwrap_or_else(|_| "gpt-image-2".to_string());

        let image_quality = std::env::var("OPENAI_IMAGE_QUALITY")
            .unwrap_or_else(|_| "medium".to_string());

        Ok(Self {
            api_key,
            api_base_url,
            model,
            image_model,
            image_quality,
            concurrency,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedSiteFiles {
    pub page_html:       String,
    pub page_css:        Option<String>,
    pub design_css:      Option<String>,
    pub assets_manifest: AssetsManifest,
    pub header_html:     Option<String>,
    pub footer_html:     Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetsManifest {
    pub assets: Vec<AssetManifestItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetManifestItem {
    pub filename: String,
    pub kind: AssetKind,
    pub description: String,

    #[serde(default)]
    pub generation_prompt: String,

    pub size: Option<String>,
    #[serde(default)]
    pub svg_code: String,
}

impl AssetManifestItem {
    pub fn public_relative_path(&self) -> Option<PathBuf> {
        let directory = self.kind.public_directory()?;

        Some(
            Path::new(directory)
                .join(&self.filename)
        )
    }

    pub fn public_url(&self) -> Option<String> {
        let directory = self.kind.public_directory()?;

        Some(format!(
            "/{directory}/{}",
            self.filename
        ))
    }
}


impl AssetKind {
    fn public_directory(self) -> Option<&'static str> {
        match self {
            Self::Image | Self::Svg => Some("images"),
            Self::Font              => Some("fonts"),
            Self::Video             => Some("videos"),
            Self::CssGenerated      => None,
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Svg,
    CssGenerated,
    Font,
    Video,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DesignImportResult {
    pub design_spec: Value,
    pub files: GeneratedSiteFiles,
}

#[derive(Debug, Clone)]
pub struct PartialStatus {
    pub has_header: bool,
    pub has_footer: bool,
}

pub const HEADER_PARTIAL: &str = "header.html";
pub const FOOTER_PARTIAL: &str = "footer.html";

impl PartialStatus {
    pub fn inspect(site_dir: &Path) -> Self {
        let partials_dir = site_dir.join("partials");

        Self {
            has_header: partials_dir.join("header.html").exists(),
            has_footer: partials_dir.join("footer.html").exists(),
        }
    }

    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();

        if !self.has_header {
            missing.push(HEADER_PARTIAL);
        }

        if !self.has_footer {
            missing.push(FOOTER_PARTIAL);
        }

        missing
    }
}
