use crate::{
    assets::{safe_relative_path, validate_manifest},
    dev_print,
};

use std::path::Path;
use std::time::Duration;

use reqwest::{header::HeaderMap, StatusCode};
use tokio::time::sleep;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use anyhow::{Context, Result, bail};
use base64::Engine;
use schemars::{JsonSchema, schema_for};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Value, json};

use futures::stream::{self, StreamExt, TryStreamExt};

use image::ImageFormat;

use crate::ai::{
    AiConfig, AssetKind, AssetManifestItem, AssetsManifest,
    DesignImportResult, GeneratedSiteFiles, PartialStatus
};

const GENERAL_PROMPT: &str = include_str!("../../resources/prompts/general.txt");
const ANALYZE_PROMPT: &str = include_str!("../../resources/prompts/analyze.txt");
const GENERATE_PROMPT: &str = include_str!("../../resources/prompts/generate.txt");

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

const MAX_API_ATTEMPTS: usize = 6;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
const MAX_RETRY_JITTER_MS: u64 = 500;

#[derive(Debug)]
struct OpenAiResponse {
    status: StatusCode,
    body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignBrief {
    pub page: PageBrief,
    pub theme: ThemeBrief,
    pub layout_system: LayoutSystemBrief,
    pub component_system: ComponentSystemBrief,
    pub sections: Vec<SectionBrief>,
    pub assets: Vec<DesignAssetBrief>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageBrief {
    pub kind: PageKind,
    pub brand_name: String,
    pub summary: String,
    pub visual_style: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    LandingPage,
    AboutPage,
    ProductPage,
    ServicePage,
    BlogPage,
    ContactPage,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThemeBrief {
    pub mode: ThemeMode,
    pub background: String,
    pub surface: String,
    pub surface_alt: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    pub accent_soft: String,
    pub border: String,
    pub mood: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    Dark,
    Mixed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutSystemBrief {
    pub design_system: DesignSystem,
    pub container: ContainerWidth,
    pub density: Density,
    pub section_style: SectionStyle,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DesignSystem {
    Hbox,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContainerWidth {
    Narrow,
    Standard,
    Wide,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Compact,
    Normal,
    Spacious,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SectionStyle {
    Flat,
    Cards,
    Editorial,
    SplitPanels,
    Mixed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentSystemBrief {
    pub backgrounds: String,
    pub buttons: String,
    pub cards: String,
    pub icons: String,
    pub typography: String,
    pub media_panels: String,
    pub badges: String,
    pub navigation: String,
    pub links: String,
    pub logo_clouds: String,
    pub forbidden_interpretations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SectionBrief {
    pub id: String,
    #[serde(rename = "type")]
    pub section_type: SectionType,
    pub layout: SectionLayout,
    pub visual_role: VisualRole,
    pub visible_text: Vec<String>,
    pub items: Vec<SectionItemBrief>,
    pub asset_roles: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SectionType {
    Header,
    Hero,
    LogoCloud,
    Features,
    Services,
    CaseStudies,
    Testimonials,
    Stats,
    Content,
    Cta,
    Footer,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SectionLayout {
    SingleColumn,
    TwoColumn,
    ThreeColumnGrid,
    CardGrid,
    MediaLeft,
    MediaRight,
    BackgroundMedia,
    Centered,
    Split,
    StatsStrip,
    LogoCloud,
    Timeline,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisualRole {
    PrimaryHero,
    Supporting,
    Proof,
    Conversion,
    Navigation,
    Footer,
    Decorative,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SectionItemBrief {
    pub label: String,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignAssetBrief {
    pub filename: String,
    pub kind: DesignAssetKind,
    pub role: String,
    pub description: String,
    pub generation_prompt: String,
    pub treatment: AssetTreatment,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DesignAssetKind {
    Image,
    Svg,
    CssGenerated,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssetTreatment {
    BackgroundBlend,
    FullBleedBackground,
    HeroMediaBlended,
    LocalizedGlow,
    ContainedMedia,
    CardMedia,
    Logo,
    Icon,
    Decorative,
}

#[derive(Clone)]
pub struct ChatGptClient {
    http: reqwest::Client,
    api_key: String,
    responses_url: String,
    image_generations_url: String,
    model: String,
    image_model: String,
    image_quality: String,
    concurrency: usize,
}

impl ChatGptClient {
    pub fn new(config: AiConfig) -> Result<Self> {
        let api_base_url = config
            .api_base_url
            .trim()
            .trim_end_matches('/')
            .to_owned();

        if api_base_url.is_empty() {
            bail!("OPENAI_BASE_URL must not be empty");
        }

        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("failed to build OpenAI HTTP client")?;

        Ok(Self {
            http,
            api_key: config.api_key,
            responses_url: format!("{api_base_url}/responses"),
            image_generations_url: format!(
                "{api_base_url}/images/generations"
            ),
            model: config.model,
            image_model: config.image_model,
            image_quality: config.image_quality,
            concurrency: config.concurrency.get(),
        })
    }

    pub async fn import_design(
        &self,
        screenshot_path: &Path,
        partial_status: &PartialStatus,
        spinner: &ProgressBar,
    ) -> Result<DesignImportResult> {
        spinner.set_message("Devising design spec");
        let design_spec = self.analyze_screenshot(screenshot_path).await?;
        spinner.set_message("Generating site files");
        let files = self.generate_site_files(&design_spec, partial_status).await?;
        let design_spec = serde_json::to_value(design_spec)
            .context("failed to serialize typed design brief")?;

        Ok(DesignImportResult { design_spec, files })
    }

    pub async fn analyze_screenshot(
        &self,
        screenshot_path: &Path
    ) -> Result<DesignBrief> {
        let image_url = data_url_from_file(screenshot_path).await?;

        let body = json!({
            "model": self.model,
            "input": [
                {
                    "role": "system",
                    "content": [
                        {
                            "type": "input_text",
                            "text": GENERAL_PROMPT
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": ANALYZE_PROMPT
                        },
                        {
                            "type": "input_image",
                            "image_url": image_url
                        }
                    ]
                }
            ],
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "hbox_design_spec",
                    "strict": true,
                    "schema": design_brief_schema()
                }
            }
        });

        dev_print!(format!("Calling OpenAI model {}", &self.model));

        let value = self.responses_create(body).await?;
        let text = extract_output_text(&value)?;

        let parsed: DesignBrief = serde_json::from_str(&text).with_context(|| {
            format!(
                "OpenAI returned invalid JSON for design spec.\n\nRaw output:\n{}",
                text
            )
        })?;

        Ok(parsed)
    }

    pub async fn generate_site_files(
        &self,
        design_spec: &DesignBrief,
        partial_status: &PartialStatus
    ) -> Result<GeneratedSiteFiles> {
        let partial_rules = partial_prompt_block(partial_status);

        let user_prompt = format!(
            "{}\n\n{}\n\nDesign specification:\n{}",
            GENERATE_PROMPT,
            partial_rules,
            serde_json::to_string_pretty(design_spec)?
        );

        //        dev_print!("ChatGPT Prompt:");
        //        dev_print!(&user_prompt);

        let body = json!({
            "model": self.model,
            "input": [
                {
                    "role": "system",
                    "content": [
                        {
                            "type": "input_text",
                            "text": GENERAL_PROMPT
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": user_prompt
                        }
                    ]
                }
            ],
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "hbox_site_files",
                    "strict": true,
                    "schema": generated_files_schema()
                }
            }
        });

        let value = self.responses_create(body).await?;
        let text = extract_output_text(&value)?;

        let files: GeneratedSiteFiles =
            serde_json::from_str(&text)
            .with_context(|| {
                format!("OpenAI returned invalid JSON envelope.\n\nRaw output:\n{}",
                        text)
            })?;

        validate_generated_files(&files, partial_status)?;

        Ok(files)
    }

    async fn responses_create(&self, mut body: Value) -> Result<Value> {
        body.as_object_mut()
            .context("OpenAI Responses request body must be a JSON object")?
            .insert("store".to_owned(), Value::Bool(false));

        let response = self
            .post_json_with_retry(
                &self.responses_url,
                &body,
                "OpenAI Responses API request",
                |delay, attempt, max_attempts| {
                    dev_print!(format!(
                        "OpenAI Responses API failed transiently; \
                         retrying in {:.1}s ({attempt}/{max_attempts})",
                        delay.as_secs_f64(),
                    ));
                },
            )
            .await?;

        if !response.status.is_success() {
            return Err(openai_error(response.status, &response.body));
        }

        serde_json::from_str(&response.body)
            .context("failed to parse OpenAI response as JSON")
    }

    async fn post_json_with_retry<F>(
        &self,
        url: &str,
        body: &Value,
        operation: &str,
        mut on_retry: F,
    ) -> Result<OpenAiResponse>
    where
        F: FnMut(Duration, usize, usize),
    {
        for attempt in 1..=MAX_API_ATTEMPTS {
            let response = match self
                .http
                .post(url)
                .bearer_auth(&self.api_key)
                .json(body)
                .send()
                .await
            {
                Ok(response) => response,

                Err(error)
                    if error.is_connect()
                        && attempt < MAX_API_ATTEMPTS =>
                {
                    let delay = backoff_delay(attempt);
                    on_retry(delay, attempt, MAX_API_ATTEMPTS);
                    sleep(delay).await;
                    continue;
                }

                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to send {operation}"));
                }
            };

            let status = response.status();
            let headers = response.headers().clone();

            let response_body = response
                .text()
                .await
                .with_context(|| {
                    format!("failed to read {operation} response body")
                })?;

            if is_retryable_status(status)
                && attempt < MAX_API_ATTEMPTS
            {
                let delay = retry_delay(&headers, attempt);
                on_retry(delay, attempt, MAX_API_ATTEMPTS);
                sleep(delay).await;
                continue;
            }

            return Ok(OpenAiResponse {
                status,
                body: response_body,
            });
        }

        unreachable!("retry loop always returns on its final attempt")
    }

    pub async fn generate_asset_images(
        &self,
        manifest: &AssetsManifest,
        assets_dir: &Path,
    ) -> Result<()> {
        tokio::fs::create_dir_all(assets_dir)
            .await
            .with_context(|| {
                format!("Failed to create assets directory {}", assets_dir.display())
            })?;

        let assets = extract_image_assets(manifest);

        if assets.is_empty() {
            return Ok(());
        }

        let mp = MultiProgress::new();

        let overall = mp.add(ProgressBar::new(assets.len() as u64));

        overall.set_style(
            ProgressStyle::with_template(
                "{spinner} Generating image assets [{bar:30}] {pos}/{len} {msg}"
            )?
        );

        let spinner_style = ProgressStyle::with_template(
            "{spinner} Worker {prefix}: {wide_msg}")?;

        let worker_bars: Vec<ProgressBar> = (0..self.concurrency)
            .map(|i| {
                let pb = mp.add(ProgressBar::new_spinner());
                pb.set_style(spinner_style.clone());
                pb.set_prefix(format!("{}", i + 1));
                pb.set_message("idle");
                pb.enable_steady_tick(Duration::from_millis(100));
                pb
            })
            .collect();

        stream::iter(assets.into_iter().enumerate())
            .map(|(idx, asset)| {
                let client = self.clone();
                let assets_dir = assets_dir.to_path_buf();
                let overall = overall.clone();

                let pb = worker_bars[idx % self.concurrency].clone();

                async move {
                    pb.set_message(format!("Generating {}", asset.filename));

                    let result = client
                        .generate_one_asset_image(&asset, &assets_dir, &pb)
                        .await;

                    match &result {
                        Ok(_) => {
                            overall.inc(1);
                            overall.set_message(format!("Generated {}", asset.filename));
                            pb.set_message("idle");
                        }
                        Err(err) => {
                            overall.println(format!(
                                "Failed {}: {}",
                                asset.filename,
                                err
                            ));
                            pb.set_message(format!("Failed {}", asset.filename));
                        }
                    }

                    result
                }
            })
            .buffer_unordered(self.concurrency)
            .try_collect::<Vec<_>>()
            .await?;

        for pb in worker_bars {
            pb.finish_and_clear();
        }

        overall.finish_with_message("All image assets generated");

        Ok(())
    }

    async fn generate_one_asset_image(
        &self,
        asset: &AssetManifestItem,
        assets_dir: &Path,
        pb: &ProgressBar,
    ) -> Result<()> {
        let prompt = asset.generation_prompt.trim();

        if prompt.is_empty() {
            bail!("asset {} has no generation prompt", asset.filename);
        }

        let relative_path = safe_relative_path(&asset.filename)
            .with_context(|| {
                format!(
                    "invalid generated image filename '{}'",
                    asset.filename
                )
            })?;

        let output_path = assets_dir.join(relative_path);

        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| {
                    format!(
                        "Failed to create asset parent directory {}",
                        parent.display()
                    )
                })?;
        }

        let (output_format, expected_format) =
            image_format_for_filename(&asset.filename)?;

        let body = json!({
            "model": self.image_model,
            "prompt": prompt,
            "size": normalize_image_size(asset.size.as_deref()),
            "quality": self.image_quality,
            "output_format": output_format,
            "n": 1
        });

        let response = self
            .post_json_with_retry(
                &self.image_generations_url,
                &body,
                "OpenAI image generation request",
                |delay, attempt, max_attempts| {
                    pb.set_message(format!(
                        "Transient failure on {}. \
                         Retrying in {:.1}s ({attempt}/{max_attempts})",
                        asset.filename,
                        delay.as_secs_f64(),
                    ));
                },
            )
            .await?;

        if !response.status.is_success() {
            return Err(openai_error(response.status, &response.body));
        }

        let value: Value = serde_json::from_str(&response.body)
            .context("Failed to parse OpenAI image response as JSON")?;

        let b64 = value
            .get("data")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("b64_json"))
            .and_then(Value::as_str)
            .with_context(|| {
                format!(
                    "OpenAI image response did not contain \
                     data[0].b64_json for {}",
                    asset.filename,
                )
            })?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .with_context(|| {
                format!(
                    "Failed to decode base64 image for {}",
                    asset.filename,
                )
            })?;

        verify_image_format(
            &asset.filename,
            &expected_format,
            &bytes,
        )?;

        tokio::fs::write(&output_path, bytes)
            .await
            .with_context(|| {
                format!(
                    "Failed to write generated image to disk {}",
                    output_path.display(),
                )
            })?;

        Ok(())
    }

    /// Requests a strict JSON-schema response and deserializes its output.
    pub async fn structured_response<T>(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        schema_name: &str,
        schema: Value,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let body = json!({
            "model": self.model,
            "input": [
                {
                    "role": "system",
                    "content": [{
                        "type": "input_text",
                        "text": system_prompt
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": user_prompt
                    }]
                }
            ],
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": schema_name,
                    "strict": true,
                    "schema": schema
                }
            }
        });
        let value = self.responses_create(body).await?;
        let text = extract_output_text(&value)?;
        serde_json::from_str(&text).with_context(|| {
            format!(
                "OpenAI returned invalid structured output for {schema_name}.\n\nRaw output:\n{text}"
            )
        })
    }
}

async fn data_url_from_file(path: &Path) -> Result<String> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read screenshot: {}", path.display()))?;

    let mime = mime_guess::from_path(path)
        .first()
        .map(|m| m.essence_str().to_string())
        .unwrap_or_else(|| "image/png".to_string());

    if !matches!(mime.as_str(), "image/png" | "image/jpeg" | "image/webp") {
        anyhow::bail!(
            "unsupported screenshot MIME type '{}'. Use png, jpg, jpeg, or webp",
            mime
        );
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);

    Ok(format!("data:{};base64,{}", mime, encoded))
}

fn extract_output_text(response: &Value) -> Result<String> {
    if let Some(text) = response.get("output_text").and_then(Value::as_str) {
        return Ok(text.to_string());
    }

    let output = response
        .get("output")
        .and_then(Value::as_array)
        .context("OpenAI response missing output array")?;

    let mut collected = String::new();

    for item in output {
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };

        for content_item in content {
            let item_type = content_item.get("type").and_then(Value::as_str);

            if matches!(item_type, Some("output_text") | Some("text")) {
                if let Some(text) = content_item.get("text").and_then(Value::as_str) {
                    collected.push_str(text);
                }
            }
        }
    }

    if collected.trim().is_empty() {
        anyhow::bail!("OpenAI response did not contain output text");
    }

    Ok(collected)
}

fn validate_partial(
    field: &str,
    partial_exists: bool,
    generated: Option<&str>,
) -> Result<()> {
    match (partial_exists, generated.map(str::trim)) {
        (true, None) => Ok(()),

        (true, Some(_)) => {
            bail!("{field} must be null because the partial already exists")
        }

        (false, Some(value)) if !value.is_empty() => Ok(()),

        (false, _) => {
            bail!("{field} must contain the missing partial")
        }
    }
}

fn validate_generated_files(files: &GeneratedSiteFiles, partial_status: &PartialStatus)
                            -> Result<()> {
    if files.page_html.trim().is_empty() {
        bail!("generated page_html is empty");
    }

    if !files.page_html.contains("<html") {
        bail!("generated page_html does not look like HTML");
    }

    if let Some(page_css) = files.page_css.as_deref() {
        if page_css.trim().is_empty() {
            bail!("page_css must be null or a non-empty string");
        }
    }

    if let Some(design_css) = files.design_css.as_deref() {
        if design_css.trim().is_empty() {
            bail!("design_css must be null or a non-empty string");
        }
    }

    if let Some(header_html) = files.header_html.as_deref() {
        if header_html.trim().is_empty() {
            bail!("header_html must be null or a non-empty string");
        }
    }

    if let Some(footer_html) = files.footer_html.as_deref() {
        if footer_html.trim().is_empty() {
            bail!("footer_html must be null or a non-empty string");
        }
    }

    validate_partial(
        "header_html",
        partial_status.has_header,
        files.header_html.as_deref(),
    )?;

    validate_partial(
        "footer_html",
        partial_status.has_footer,
        files.footer_html.as_deref(),
    )?;

    validate_manifest(&files.assets_manifest)
        .context("OpenAI returned an invalid assets manifest")?;

    Ok(())
}

fn openai_error(status: StatusCode, body: &str) -> anyhow::Error {
    let parsed: Result<Value, _> = serde_json::from_str(body);

    if let Ok(value) = parsed {
        if let Some(message) = value
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
        {
            return anyhow::anyhow!("OpenAI API error {}: {}", status, message);
        }
    }

    anyhow::anyhow!("OpenAI API error {}: {}", status, body)
}

fn design_brief_schema() -> Value {
    serde_json::to_value(schema_for!(DesignBrief))
        .expect("DesignBrief JSON schema should serialize")
}

fn generated_files_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "page_html": {
                "type": "string"
            },
            "page_css": {
                "type": ["string", "null"]
            },
            "design_css": {
                "type": ["string", "null"]
            },
            "assets_manifest": {
                "type": "object",
                "properties": {
                    "assets": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "filename": { "type": "string" },
                                "kind": {
                                    "type": "string",
                                    "enum": [
                                        "image",
                                        "svg",
                                        "css_generated"
                                    ]
                                },
                                "description": { "type": "string" },
                                "generation_prompt": { "type": "string" },
                                "svg_code": { "type": "string" },
                                "size": {
                                    "type": ["string", "null"],
                                    "enum": [
                                        "1024x1024",
                                        "1024x1536",
                                        "1536x1024",
                                        "auto",
                                        null
                                    ]
                                }
                            },
                            "required": [
                                "filename",
                                "kind",
                                "description",
                                "generation_prompt",
                                "svg_code",
                                "size"
                            ],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["assets"],
                "additionalProperties": false
            },
            "header_html": {
                "type": ["string", "null"]
            },
            "footer_html": {
                "type": ["string", "null"]
            }
        },
        "required": [
            "page_html",
            "page_css",
            "design_css",
            "assets_manifest",
            "header_html",
            "footer_html"
        ],
        "additionalProperties": false
    })
}

fn normalize_image_size(size: Option<&str>) -> &str {
    match size {
        Some("1024x1024") => "1024x1024",
        Some("1024x1536") => "1024x1536",
        Some("1536x1024") => "1536x1024",
        Some("auto")      => "auto",
        _ => "1024x1024",
    }
}

fn extract_image_assets(manifest: &AssetsManifest) -> Vec<AssetManifestItem> {
    manifest
        .assets
        .iter()
        .filter(|asset| matches!(&asset.kind, AssetKind::Image))
        .cloned()
        .collect()
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn retry_delay(
    headers: &HeaderMap,
    attempt: usize,
) -> Duration {
    let delay = retry_after_seconds(headers)
        .map(Duration::from_secs)
        .or_else(|| {
            retry_after_ms(headers).map(Duration::from_millis)
        })
        .unwrap_or_else(|| exponential_backoff(attempt));

    with_jitter(delay)
}

fn backoff_delay(attempt: usize) -> Duration {
    with_jitter(exponential_backoff(attempt))
}

fn exponential_backoff(attempt: usize) -> Duration {
    let exponent = attempt.saturating_sub(1).min(5) as u32;
    Duration::from_secs(1_u64 << exponent)
}

fn with_jitter(delay: Duration) -> Duration {
    let jitter = Duration::from_millis(
        fastrand::u64(0..=MAX_RETRY_JITTER_MS),
    );

    delay
        .saturating_add(jitter)
        .min(MAX_RETRY_DELAY)
}

fn retry_after_seconds(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn partial_prompt_block(status: &PartialStatus) -> String {
    let missing = status.missing();

    if missing.is_empty() {
        return concat!(
            "Shared partial rules:\n",
            "- The site already has partials/header.html.\n",
            "- The site already has partials/footer.html.\n",
            "- Do not generate header_html or footer_html.\n",
            "- The generated page_html must include the existing partials:\n",
            "  {% include \"partials/header.html\" %}\n",
            "  {% include \"partials/footer.html\" %}\n"
        )
        .to_string();
    }

    let missing_list = missing
        .iter()
        .map(|name| format!("- partials/{name}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        concat!(
            "Shared partial rules:\n",
            "- Some shared partials are missing.\n",
            "- Generate only the missing partial fields listed below.\n",
            "- Do not regenerate partials that already exist.\n",
            "- The CSS in partials must go into design.css like all other styles\n",
            "- Missing partial files:\n",
            "{}\n\n",
            "- The generated page_html must include both partials:\n",
            "  {{% include \"partials/header.html\" %}}\n",
            "  {{% include \"partials/footer.html\" %}}\n",
            "- header_html and footer_html are conditionally required fields.\n",
            "- If a partial is listed as missing, the corresponding field MUST be returned as a non-empty string.\n",
            "- If a partial is not listed as missing, the corresponding field MUST be null.\n",
        ),
        missing_list
    )
}

fn image_format_for_filename(
    filename: &str,
) -> Result<(&'static str, ImageFormat)> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .with_context(|| {
            format!("generated image has no extension: {filename}")
        })?;

    match extension.as_str() {
        "png" => Ok(("png", ImageFormat::Png)),
        "jpg" | "jpeg" => Ok(("jpeg", ImageFormat::Jpeg)),
        "webp" => Ok(("webp", ImageFormat::WebP)),
        _ => bail!(
            "unsupported generated image extension: {extension}"
        ),
    }
}

fn verify_image_format(
    filename: &str,
    expected: &ImageFormat,
    bytes: &[u8],
) -> Result<()> {
    let actual = image::guess_format(bytes)
        .with_context(|| {
            format!("could not detect image format for {filename}")
        })?;

    if &actual != expected {
        bail!(
            "generated image format mismatch for {filename}: \
             expected {expected:?}, received {actual:?}"
        );
    }

    Ok(())
}
