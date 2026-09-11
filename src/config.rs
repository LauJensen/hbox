use std::{
    fs,
    fmt,
    num::NonZeroU32,
    path::{Component,PathBuf,Path}
};

use chrono::format::{Item, StrftimeItems};

use crate::rendering::highlighting::CodeTheme;

use anyhow::{bail,Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_WEBP_QUALITY: u8 = 82;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteConfig {
    pub site: SiteInfo,

    #[serde(default)]
    pub nginx: NginxConfig,

    #[serde(default)]
    pub optimizations: OptimizationConfig,

    #[serde(default)]
    pub deployment: Option<DeployConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployConfig {
    pub ssh_user: String,
    pub ssh_host: String,
    pub deploy_path: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OptimizationConfig {
    pub webp_quality: u8,
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            webp_quality: DEFAULT_WEBP_QUALITY,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NginxConfig {
    pub domains: Vec<String>,
    pub access_log: bool,
}

impl NginxConfig {
    pub fn is_enabled(&self) -> bool {
        !self.domains.is_empty()
    }

    pub fn domains_as_str(&self) -> String {
        self.domains.join(" ")
    }

    pub fn access_log_directive(&self, site_name: &str) -> String {
        if self.access_log {
            format!(
                "access_log /var/log/nginx/{site_name}.access.log \
                 main buffer=64k flush=5s;"
                    )
        } else {
            "access_log off;".to_owned()
        }
    }
}


impl SiteConfig {
    fn validate(&self) -> Result<()> {
        let format = &self.site.date_format;

        if format.is_empty()
            || StrftimeItems::new(format)
                .any(|item| matches!(item, Item::Error))
        {
            bail!("invalid site.date_format: {format:?}");
        }

        if self.optimizations.webp_quality > 100 {
            bail!(
                "optimizations.webp_quality must be between 0 and 100, got {}",
                self.optimizations.webp_quality
            );
        }

        Ok(())
    }
}

const SITES_DIR: &str = "sites";
const DIST_DIR: &str = "dist";

/// All filesystem locations belonging to one resolved Hbox site.
/// Instances can only be constructed through `ResolvedSite::resolve`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSite {
    pub site_name:      String,
    source_dir:         PathBuf,
    output_dir:         PathBuf,
    output_backup_dir:  PathBuf,
    output_staging_dir: PathBuf,
}

impl ResolvedSite {
    /// Resolves either `foo` or `sites/foo` into the site's managed paths.
    ///
    /// The input must identify exactly one site. Absolute paths, parent
    /// traversal, nested paths and hidden names are rejected.
    pub fn resolve(input: impl AsRef<Path>) -> Result<Self> {
        let site_name = parse_site_name(input.as_ref())?;

        Ok(Self::from_validated_name(site_name))
    }

    /// Returns the validated site name without its `sites/` prefix.
    pub fn site_name(&self) -> &str {
        &self.site_name
    }

    /// Returns the canonical source directory, such as `sites/foo`.
    pub fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    /// Returns the published build directory, such as `dist/foo`.
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Returns the backup build directory, such as `dist/.foo.backup`.
    pub fn output_backup_dir(&self) -> &Path {
        &self.output_backup_dir
    }


    /// Returns the temporary build directory, such as `dist/.foo.staging`.
    ///
    /// A successful build can replace `output_dir` by renaming this directory.
    pub fn output_staging_dir(&self) -> &Path {
        &self.output_staging_dir
    }

    /// Returns the site name for a numbered preview.
    pub fn preview_name(&self, index: NonZeroU32) -> String {
        format!(".preview-{}-{index}", self.site_name)
    }

    /// Resolves a numbered preview belonging to this site.
    ///
    /// No validation can fail here because the original site name is already
    /// validated and the numeric suffix is generated internally.
    pub fn preview(&self, index: NonZeroU32) -> Self {
        Self::from_validated_name(self.preview_name(index))
    }

    /// Clojures allowance of ? in names is sorely missed
    pub fn is_initialized(&self) -> bool {
        self.source_dir.join("hbox.toml").is_file()
    }

    /// Constructs all paths from a previously validated site name.
    fn from_validated_name(site_name: String) -> Self {
        let source_dir = Path::new(SITES_DIR).join(&site_name);
        let output_dir = Path::new(DIST_DIR).join(&site_name);
        let output_backup_dir =
            Path::new(DIST_DIR).join(format!(".{site_name}.backup"));
        let output_staging_dir =
            Path::new(DIST_DIR).join(format!(".{site_name}.staging"));

        Self {
            site_name,
            source_dir,
            output_dir,
            output_backup_dir,
            output_staging_dir,
        }
    }
}

impl fmt::Display for ResolvedSite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source_dir.display().fmt(formatter)
    }
}

/// Extracts one safe site name from either `foo` or `sites/foo`.
fn parse_site_name(input: &Path) -> Result<String> {
    let relative = input
        .strip_prefix(SITES_DIR)
        .unwrap_or(input);

    let mut components = relative.components();

    let name = match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) => name,
        _ => {
            bail!(
                "invalid site '{}': expected a single site name or sites/<site-name>",
                input.display(),
            );
        }
    };

    let name = name
        .to_str()
        .context("site name is not valid UTF-8")?;

    if name.starts_with('.') {
        bail!("site names must not begin with '.': {name}");
    }

    if name.contains('\\') || name.contains('\0') {
        bail!("site name contains unsupported characters: {name}");
    }

    Ok(name.to_owned())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteInfo {
    pub name: String,

    #[serde(default)]
    pub base_url: String,

    #[serde(default = "default_language")]
    pub default_language: String,

    #[serde(default = "default_date_format")]
    pub date_format: String,

    #[serde(default)]
    pub code_theme: CodeTheme,

    pub default_title: Option<String>,
}

pub fn load_config(site_dir: &Path) -> Result<SiteConfig> {
    let path = site_dir.join("hbox.toml");

    let raw = fs::read_to_string(&path)
        .with_context(|| {
            format!("failed to read config file {}", path.display())
        })?;

    let config: SiteConfig = toml::from_str(&raw)
        .with_context(|| {
            format!("failed to parse config file {}", path.display())
        })?;

    config.validate()
        .with_context(|| {
            format!("invalid config file {}", path.display())
        })?;

    Ok(config)
}

fn default_date_format() -> String {
    "%d.%m.%Y".to_string()
}

fn default_language() -> String {
    "en".to_string()
}
