use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        mpsc::{self, TryRecvError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use base64::Engine as _;
use serde_json::{json, Value};
use tempfile::TempDir;

const SITE_NAME: &str = "integration.test";
const PAGE_SLUG: &str = "index";
const API_KEY: &str = "integration-test-key";
const TEXT_MODEL: &str = "test-text-model";
const IMAGE_MODEL: &str = "test-image-model";
const IMAGE_QUALITY: &str = "high";

const ORIGINAL_PAGE_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Original page</title>
  </head>
  <body>
    <main id="original-page">Original page</main>
  </body>
</html>"#;

const ORIGINAL_PAGE_CSS: &str =
    "#original-page { color: navy; }";
const ORIGINAL_DESIGN_CSS: &str =
    ":root { --original-accent: navy; }";
const ORIGINAL_GLOBAL_CSS: &str =
    "*, *::before, *::after { box-sizing: border-box; }";

const UPDATED_PAGE_CSS: &str =
    ".updated-page { color: purple; }";
const UPDATED_DESIGN_CSS: &str =
    ":root { --updated-accent: gold; }";
const UPDATED_GLOBAL_CSS: &str =
    "*, *::before, *::after { box-sizing: border-box; }\nbody { margin: 0; }";

const TINY_PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

const SVG_SOURCE: &str = concat!(
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">"#,
    r#"<path d="M1 5h8M5 1v8"/>"#,
    "</svg>",
);

#[test]
fn update_replaces_files_assets_builds_preview_and_caches_prompt() {
    let fixture = Fixture::initialized();
    let raw_prompt = "  Make the page gold and add a generated hero  ";
    let update = update_value(
        updated_page_html("complete-update", true),
        Some(UPDATED_PAGE_CSS),
        Some(UPDATED_DESIGN_CSS),
        Some(UPDATED_GLOBAL_CSS),
        complete_asset_manifest(),
    );

    let mock = MockOpenAi::start(vec![
        Stub::output("/v1/responses", update),
        Stub::json(
            "/v1/images/generations",
            200,
            json!({ "data": [{ "b64_json": TINY_PNG_BASE64 }] }),
        ),
    ]);

    let output = run_update(
        &fixture,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        raw_prompt,
    );

    assert_success(&output);
    let output_text = stdout(&output);
    assert!(output_text.contains("Generating update for index..."));
    assert!(output_text.contains("Update successfully generated."));
    assert!(output_text.contains("- hbox preview integration.test 1"));
    assert!(output_text.contains("- hbox accept integration.test 1"));

    let requests = mock.finish();
    assert_eq!(requests.len(), 2);
    assert_common_request_headers(&requests);

    let update_request = requests[0].json_body();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(update_request["model"], json!(TEXT_MODEL));
    assert_eq!(update_request["store"], json!(false));
    assert_eq!(
        update_request
            .pointer("/text/format/name")
            .and_then(Value::as_str),
        Some("hbox_site_update")
    );
    assert_eq!(
        update_request
            .pointer("/text/format/strict")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        update_request
            .pointer("/text/format/schema/additionalProperties")
            .and_then(Value::as_bool),
        Some(false)
    );

    let system_prompt = update_request
        .pointer("/input/0/content/0/text")
        .and_then(Value::as_str)
        .expect("update request should contain the system prompt");
    assert!(system_prompt.contains(
        "Return complete replacement file contents"
    ));
    assert!(system_prompt.contains(
        "Do not introduce external fonts, scripts, stylesheets, or CDNs"
    ));

    let user_prompt = update_request
        .pointer("/input/1/content/0/text")
        .and_then(Value::as_str)
        .expect("update request should contain the user prompt");
    assert!(user_prompt.starts_with(
        "Requested change:\nMake the page gold and add a generated hero\n\n"
    ));

    let current_files = current_files_from_prompt(user_prompt);
    assert_eq!(
        current_files["page_html"],
        json!(ORIGINAL_PAGE_HTML)
    );
    assert_eq!(
        current_files["page_css"],
        json!(ORIGINAL_PAGE_CSS)
    );
    assert_eq!(
        current_files["design_css"],
        json!(ORIGINAL_DESIGN_CSS)
    );
    assert_eq!(
        current_files["global_css"],
        json!(ORIGINAL_GLOBAL_CSS)
    );

    let image_request = requests[1].json_body();
    assert_eq!(requests[1].path, "/v1/images/generations");
    assert_eq!(image_request["model"], json!(IMAGE_MODEL));
    assert_eq!(
        image_request["prompt"],
        json!("A wide deterministic golden hero")
    );
    assert_eq!(image_request["size"], json!("1536x1024"));
    assert_eq!(image_request["quality"], json!(IMAGE_QUALITY));
    assert_eq!(image_request["output_format"], json!("png"));
    assert_eq!(image_request["n"], json!(1));

    let original = fixture.site_source(SITE_NAME);
    assert_eq!(
        read_text(original.join("pages/index.html")),
        ORIGINAL_PAGE_HTML
    );
    assert_eq!(
        read_text(original.join("pages/index.css")),
        ORIGINAL_PAGE_CSS
    );
    assert_eq!(
        read_text(original.join("design.css")),
        ORIGINAL_DESIGN_CSS
    );
    assert_eq!(
        read_text(original.join("global.css")),
        ORIGINAL_GLOBAL_CSS
    );
    assert_eq!(
        fs::read(original.join("public/images/hero.png"))
            .expect("original hero fixture should be readable"),
        Fixture::original_hero_bytes().to_vec()
    );
    assert!(!original.join("public/images/icons/plus.svg").exists());

    let source_prompts = prompt_cache_files(&original, PAGE_SLUG);
    assert_eq!(source_prompts.len(), 1);
    assert_eq!(read_text(&source_prompts[0]), raw_prompt);
    assert_timestamp_directory(&source_prompts[0]);

    let preview = fixture.preview_source(SITE_NAME, 1);
    assert_eq!(
        read_text(preview.join("pages/index.html")),
        updated_page_html("complete-update", true)
    );
    assert_eq!(
        read_text(preview.join("pages/index.css")),
        UPDATED_PAGE_CSS
    );
    assert_eq!(
        read_text(preview.join("design.css")),
        UPDATED_DESIGN_CSS
    );
    assert_eq!(
        read_text(preview.join("global.css")),
        UPDATED_GLOBAL_CSS
    );

    let expected_png = base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG_BASE64)
        .expect("test PNG should be valid base64");
    assert_eq!(
        fs::read(preview.join("public/images/hero.png"))
            .expect("replacement hero should be installed"),
        expected_png
    );
    assert_eq!(
        read_text(preview.join("public/images/icons/plus.svg")),
        SVG_SOURCE
    );
    assert!(
        !preview.join("public/images/generated-glow").exists(),
        "CSS-generated assets must not create files"
    );
    assert!(
        !preview.join(".hbox/update-assets-tmp").exists(),
        "asset staging directory should be removed"
    );

    let preview_prompts = prompt_cache_files(&preview, PAGE_SLUG);
    assert_eq!(preview_prompts.len(), 1);
    assert_eq!(read_text(&preview_prompts[0]), raw_prompt);
    assert_eq!(
        source_prompts[0]
            .parent()
            .and_then(Path::file_name),
        preview_prompts[0]
            .parent()
            .and_then(Path::file_name),
        "the source cache is created before the site is copied"
    );

    let output_dir = fixture.preview_output(SITE_NAME, 1);
    let built_page = read_text(output_dir.join("index.html"));
    assert!(built_page.contains("complete-update"));
    assert!(built_page.contains(r#"id="hbox-page-css""#));
    assert!(built_page.contains(".updated-page{color:purple}"));
    assert!(built_page.contains(r#"href="/global.css""#));
    assert!(built_page.contains(r#"href="/design."#));
    assert!(output_dir.join("images/hero.png").is_file());
    assert!(output_dir.join("images/icons/plus.svg").is_file());

    let built_global_css = read_text(output_dir.join("global.css"));
    assert!(built_global_css.contains("body{margin:0}"));

    let design_outputs = fs::read_dir(&output_dir)
        .expect("preview output should be readable")
        .map(|entry| entry.expect("directory entry should be readable"))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("design.") && name.ends_with(".css"))
        .collect::<Vec<_>>();
    assert_eq!(design_outputs.len(), 1);

    assert_no_transient_output_artifacts(&fixture, SITE_NAME, 1);
}

#[test]
fn null_optional_replacements_preserve_existing_files() {
    let fixture = Fixture::initialized();
    let update = update_value(
        updated_page_html("html-only-update", false),
        None,
        None,
        None,
        Vec::new(),
    );
    let mock = MockOpenAi::start(vec![
        Stub::output("/v1/responses", update),
    ]);

    let output = run_update(
        &fixture,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        "Change only the heading",
    );

    assert_success(&output);
    assert_eq!(mock.finish().len(), 1);

    let original = fixture.site_source(SITE_NAME);
    let preview = fixture.preview_source(SITE_NAME, 1);

    assert_eq!(
        read_text(preview.join("pages/index.html")),
        updated_page_html("html-only-update", false)
    );
    assert_eq!(
        read_text(preview.join("pages/index.css")),
        ORIGINAL_PAGE_CSS
    );
    assert_eq!(
        read_text(preview.join("design.css")),
        ORIGINAL_DESIGN_CSS
    );
    assert_eq!(
        read_text(preview.join("global.css")),
        ORIGINAL_GLOBAL_CSS
    );
    assert_eq!(
        fs::read(preview.join("public/images/hero.png"))
            .expect("copied hero should be readable"),
        Fixture::original_hero_bytes().to_vec()
    );

    assert_eq!(
        read_text(original.join("pages/index.html")),
        ORIGINAL_PAGE_HTML
    );
    assert_eq!(prompt_cache_files(&original, PAGE_SLUG).len(), 1);

    let built_page =
        read_text(fixture.preview_output(SITE_NAME, 1).join("index.html"));
    assert!(built_page.contains("html-only-update"));
    assert!(built_page.contains("#original-page{color:navy}"));
    assert_no_transient_output_artifacts(&fixture, SITE_NAME, 1);
}

#[test]
fn update_uses_the_next_available_preview_number() {
    let fixture = Fixture::initialized();

    let first_preview = fixture.preview_source(SITE_NAME, 1);
    write_text(first_preview.join("sentinel.txt"), "keep source preview one");

    let first_output = fixture.preview_output(SITE_NAME, 1);
    write_text(first_output.join("sentinel.txt"), "keep output preview one");

    let update = update_value(
        updated_page_html("second-preview", false),
        None,
        None,
        None,
        Vec::new(),
    );
    let mock = MockOpenAi::start(vec![
        Stub::output("/v1/responses", update),
    ]);

    let output = run_update(
        &fixture,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        "Create the next preview",
    );

    assert_success(&output);
    assert_eq!(mock.finish().len(), 1);
    let output_text = stdout(&output);
    assert!(output_text.contains("- hbox preview integration.test 2"));
    assert!(output_text.contains("- hbox accept integration.test 2"));

    assert_eq!(
        read_text(first_preview.join("sentinel.txt")),
        "keep source preview one"
    );
    assert_eq!(
        read_text(first_output.join("sentinel.txt")),
        "keep output preview one"
    );
    assert!(
        fixture
            .preview_source(SITE_NAME, 2)
            .join("pages/index.html")
            .is_file()
    );
    assert!(
        fixture
            .preview_output(SITE_NAME, 2)
            .join("index.html")
            .is_file()
    );
    assert_no_transient_output_artifacts(&fixture, SITE_NAME, 2);
}

#[test]
fn update_rejects_invalid_slugs_and_missing_inputs_before_api_use() {
    let fixture = Fixture::initialized();
    let mock = MockOpenAi::start(Vec::new());

    for (slug, expected_error) in [
        ("", "unsafe relative path"),
        (".index", "Page name must be a single, non-hidden path component"),
        ("pages/index", "Page name must be a single, non-hidden path component"),
        ("../index", "unsafe relative path"),
    ] {
        let output = run_update(
            &fixture,
            &mock,
            SITE_NAME,
            slug,
            "This request must not reach the API",
        );
        assert_failure_contains(&output, expected_error);
        assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
        assert_no_prompt_cache(&fixture.site_source(SITE_NAME), PAGE_SLUG);
    }

    let missing_site = Fixture::empty();
    let output = run_update(
        &missing_site,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        "This request must not reach the API",
    );
    assert_failure_contains(&output, "Site does not exist");
    assert_no_preview_artifacts(&missing_site, SITE_NAME, 1);

    let missing_page = Fixture::initialized();
    fs::remove_file(
        missing_page
            .site_source(SITE_NAME)
            .join("pages/index.html"),
    )
    .expect("page fixture should be removable");
    let output = run_update(
        &missing_page,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        "This request must not reach the API",
    );
    assert_failure_contains(&output, "Failed to read sites/integration.test/pages/index.html");
    assert_no_preview_artifacts(&missing_page, SITE_NAME, 1);
    assert_no_prompt_cache(
        &missing_page.site_source(SITE_NAME),
        PAGE_SLUG,
    );

    let missing_global = Fixture::initialized();
    fs::remove_file(
        missing_global.site_source(SITE_NAME).join("global.css"),
    )
    .expect("global CSS fixture should be removable");
    let output = run_update(
        &missing_global,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        "This request must not reach the API",
    );
    assert_failure_contains(&output, "Failed to read sites/integration.test/global.css");
    assert_no_preview_artifacts(&missing_global, SITE_NAME, 1);
    assert_no_prompt_cache(
        &missing_global.site_source(SITE_NAME),
        PAGE_SLUG,
    );

    let missing_key = Fixture::initialized();
    let mut command = update_command(
        &missing_key,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        "This request must not reach the API",
    );
    command.env_remove("OPENAI_API_KEY");
    let output = command
        .output()
        .expect("hbox update process should start");
    assert_failure_contains(&output, "OPENAI_API_KEY is not set");
    assert_no_preview_artifacts(&missing_key, SITE_NAME, 1);
    assert_no_prompt_cache(
        &missing_key.site_source(SITE_NAME),
        PAGE_SLUG,
    );

    assert!(
        mock.finish().is_empty(),
        "preflight failures must not contact OpenAI"
    );
}

#[test]
fn api_and_deserialization_failures_leave_no_cache_or_preview() {
    let fixture = Fixture::initialized();
    let mock = MockOpenAi::start(vec![
        Stub::json(
            "/v1/responses",
            400,
            json!({
                "error": {
                    "message": "deliberate update failure"
                }
            }),
        ),
        Stub::output_text("/v1/responses", "not valid JSON"),
    ]);

    let output = run_update(
        &fixture,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        "API failure",
    );
    assert_failure_contains(&output, "deliberate update failure");
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    assert_no_prompt_cache(&fixture.site_source(SITE_NAME), PAGE_SLUG);

    let output = run_update(
        &fixture,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        "Malformed model response",
    );
    assert_failure_contains(
        &output,
        "OpenAI returned invalid structured output for hbox_site_update",
    );
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    assert_no_prompt_cache(&fixture.site_source(SITE_NAME), PAGE_SLUG);

    assert_eq!(mock.finish().len(), 2);
}

#[test]
fn invalid_updates_are_rejected_before_cache_or_preview_creation() {
    let fixture = Fixture::initialized();
    let cases = invalid_update_cases();
    let stubs = cases
        .iter()
        .map(|(update, _expected)| {
            Stub::output("/v1/responses", update.clone())
        })
        .collect();
    let mock = MockOpenAi::start(stubs);

    for (_update, expected_error) in &cases {
        let output = run_update(
            &fixture,
            &mock,
            SITE_NAME,
            PAGE_SLUG,
            "Return an invalid update",
        );

        assert_failure_contains(&output, expected_error);
        assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
        assert_no_prompt_cache(
            &fixture.site_source(SITE_NAME),
            PAGE_SLUG,
        );
    }

    assert_eq!(mock.finish().len(), cases.len());
}

#[test]
fn image_failure_removes_preview_but_keeps_the_used_prompt() {
    let fixture = Fixture::initialized();
    let prompt = "Generate an image that will fail";
    let image_asset = json!({
        "filename": "hero.png",
        "kind": "image",
        "description": "Replacement hero",
        "generation_prompt": "A deliberately failing image",
        "size": "1024x1024",
        "svg_code": ""
    });
    let update = update_value(
        updated_page_html("image-failure", true),
        None,
        None,
        None,
        vec![image_asset],
    );
    let mock = MockOpenAi::start(vec![
        Stub::output("/v1/responses", update),
        Stub::json(
            "/v1/images/generations",
            400,
            json!({
                "error": {
                    "message": "deliberate image failure"
                }
            }),
        ),
    ]);

    let output = run_update(
        &fixture,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        prompt,
    );

    assert_failure_contains(&output, "deliberate image failure");
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    assert_original_files_unchanged(&fixture);

    let prompts =
        prompt_cache_files(&fixture.site_source(SITE_NAME), PAGE_SLUG);
    assert_eq!(prompts.len(), 1);
    assert_eq!(read_text(&prompts[0]), prompt);
    assert_eq!(mock.finish().len(), 2);
}

#[test]
fn build_failure_removes_preview_and_staging_but_keeps_used_prompt() {
    let fixture = Fixture::initialized();
    let prompt = "Return a page that cannot build";
    let broken_page = concat!(
        "<!doctype html><html lang=\"en\"><head><title>Broken</title>",
        "<body><main>Missing closing head</main></body></html>",
    );
    let update = update_value(
        broken_page.to_string(),
        None,
        None,
        None,
        Vec::new(),
    );
    let mock = MockOpenAi::start(vec![
        Stub::output("/v1/responses", update),
    ]);

    let output = run_update(
        &fixture,
        &mock,
        SITE_NAME,
        PAGE_SLUG,
        prompt,
    );

    assert_failure_contains(&output, "Document is missing </head>");
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    assert_original_files_unchanged(&fixture);

    let prompts =
        prompt_cache_files(&fixture.site_source(SITE_NAME), PAGE_SLUG);
    assert_eq!(prompts.len(), 1);
    assert_eq!(read_text(&prompts[0]), prompt);
    assert_eq!(mock.finish().len(), 1);
}

struct Fixture {
    _temp_dir: TempDir,
    root: PathBuf,
}

impl Fixture {
    fn empty() -> Self {
        let temp_dir =
            tempfile::tempdir().expect("temporary test directory should exist");
        let root = temp_dir.path().to_path_buf();

        Self {
            _temp_dir: temp_dir,
            root,
        }
    }

    fn initialized() -> Self {
        let fixture = Self::empty();
        let output = Command::new(env!("CARGO_BIN_EXE_hbox"))
            .current_dir(&fixture.root)
            .arg("init")
            .arg(SITE_NAME)
            .output()
            .expect("hbox init process should start");

        assert_success(&output);

        let site = fixture.site_source(SITE_NAME);
        write_text(site.join("pages/index.html"), ORIGINAL_PAGE_HTML);
        write_text(site.join("pages/index.css"), ORIGINAL_PAGE_CSS);
        write_text(site.join("design.css"), ORIGINAL_DESIGN_CSS);
        write_text(site.join("global.css"), ORIGINAL_GLOBAL_CSS);
        write_bytes(
            site.join("public/images/hero.png"),
            Self::original_hero_bytes(),
        );

        fixture
    }

    fn original_hero_bytes() -> &'static [u8] {
        b"original hero bytes"
    }

    fn site_source(&self, site_name: &str) -> PathBuf {
        self.root.join("sites").join(site_name)
    }

    fn preview_source(&self, site_name: &str, index: u32) -> PathBuf {
        self.site_source(&format!(".preview-{site_name}-{index}"))
    }

    fn preview_output(&self, site_name: &str, index: u32) -> PathBuf {
        self.root
            .join("dist")
            .join(format!(".preview-{site_name}-{index}"))
    }

    fn preview_staging_output(
        &self,
        site_name: &str,
        index: u32,
    ) -> PathBuf {
        self.root
            .join("dist")
            .join(format!("..preview-{site_name}-{index}.staging"))
    }

    fn preview_backup_output(
        &self,
        site_name: &str,
        index: u32,
    ) -> PathBuf {
        self.root
            .join("dist")
            .join(format!("..preview-{site_name}-{index}.backup"))
    }
}

fn update_command(
    fixture: &Fixture,
    mock: &MockOpenAi,
    site_name: &str,
    slug: &str,
    prompt: &str,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hbox"));

    command
        .current_dir(&fixture.root)
        .arg("update")
        .arg(site_name)
        .arg(slug)
        .arg(prompt)
        .arg("--threads")
        .arg("1")
        .env("OPENAI_API_KEY", API_KEY)
        .env("OPENAI_BASE_URL", mock.base_url())
        .env("OPENAI_MODEL", TEXT_MODEL)
        .env("OPENAI_IMAGE_MODEL", IMAGE_MODEL)
        .env("OPENAI_IMAGE_QUALITY", IMAGE_QUALITY)
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env("NO_PROXY", "127.0.0.1,localhost");

    command
}

fn run_update(
    fixture: &Fixture,
    mock: &MockOpenAi,
    site_name: &str,
    slug: &str,
    prompt: &str,
) -> Output {
    update_command(fixture, mock, site_name, slug, prompt)
        .output()
        .expect("hbox update process should start")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command to succeed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout(output),
        stderr(output),
    );
}

fn assert_failure_contains(output: &Output, expected: &str) {
    assert!(
        !output.status.success(),
        "expected command to fail\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output),
    );

    let combined = format!("{}\n{}", stdout(output), stderr(output));
    assert!(
        combined.contains(expected),
        "expected output to contain {expected:?}\nactual output:\n{combined}",
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_no_preview_artifacts(
    fixture: &Fixture,
    site_name: &str,
    index: u32,
) {
    for path in [
        fixture.preview_source(site_name, index),
        fixture.preview_output(site_name, index),
        fixture.preview_staging_output(site_name, index),
        fixture.preview_backup_output(site_name, index),
    ] {
        assert!(
            !path.exists(),
            "failed update left an artifact behind: {}",
            path.display()
        );
    }
}

fn assert_no_transient_output_artifacts(
    fixture: &Fixture,
    site_name: &str,
    index: u32,
) {
    for path in [
        fixture.preview_staging_output(site_name, index),
        fixture.preview_backup_output(site_name, index),
    ] {
        assert!(
            !path.exists(),
            "successful update left a transient artifact behind: {}",
            path.display()
        );
    }
}

fn assert_original_files_unchanged(fixture: &Fixture) {
    let source = fixture.site_source(SITE_NAME);
    assert_eq!(
        read_text(source.join("pages/index.html")),
        ORIGINAL_PAGE_HTML
    );
    assert_eq!(
        read_text(source.join("pages/index.css")),
        ORIGINAL_PAGE_CSS
    );
    assert_eq!(
        read_text(source.join("design.css")),
        ORIGINAL_DESIGN_CSS
    );
    assert_eq!(
        read_text(source.join("global.css")),
        ORIGINAL_GLOBAL_CSS
    );
    assert_eq!(
        fs::read(source.join("public/images/hero.png"))
            .expect("original hero should remain readable"),
        Fixture::original_hero_bytes().to_vec()
    );
}

fn assert_no_prompt_cache(site: &Path, slug: &str) {
    let prompts = prompt_cache_files(site, slug);
    assert!(
        prompts.is_empty(),
        "unexpected cached prompts: {prompts:#?}"
    );
}

fn prompt_cache_files(site: &Path, slug: &str) -> Vec<PathBuf> {
    let cache_root = site.join(".hbox/updates").join(slug);
    let entries = match fs::read_dir(&cache_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Vec::new();
        }
        Err(error) => {
            panic!(
                "failed to inspect prompt cache {}: {error}",
                cache_root.display()
            );
        }
    };

    let mut prompts = entries
        .map(|entry| {
            entry
                .expect("prompt cache entry should be readable")
                .path()
                .join("prompt.txt")
        })
        .collect::<Vec<_>>();
    prompts.sort();
    prompts
}

fn assert_timestamp_directory(prompt_file: &Path) {
    let timestamp = prompt_file
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .expect("prompt cache should have a UTF-8 timestamp directory");

    assert!(
        timestamp.parse::<u128>().is_ok(),
        "cache directory should be a millisecond timestamp: {timestamp}"
    );
}

fn current_files_from_prompt(prompt: &str) -> Value {
    let (_, after_heading) = prompt
        .split_once("Current files:\n")
        .expect("update prompt should contain the current-files heading");
    let (json_text, _) = after_heading
        .split_once("\n\nReturn the complete updated page_html")
        .expect("update prompt should contain the return instructions");

    serde_json::from_str(json_text)
        .expect("current files in update prompt should be valid JSON")
}

fn write_text(path: impl AsRef<Path>, contents: impl AsRef<str>) {
    write_bytes(path, contents.as_ref().as_bytes());
}

fn write_bytes(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .expect("test fixture parent directory should be creatable");
    }

    fs::write(path, contents.as_ref())
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
}

fn read_text(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn updated_page_html(marker: &str, include_image: bool) -> String {
    let image = if include_image {
        r#"<img id="updated-hero" src="/images/hero.png" alt="Updated hero">"#
    } else {
        ""
    };

    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Updated page</title>
  </head>
  <body>
    <main class="updated-page">
      <h1 id="update-marker">{marker}</h1>
      {image}
    </main>
  </body>
</html>"#
    )
}

fn update_value(
    page_html: String,
    page_css: Option<&str>,
    design_css: Option<&str>,
    global_css: Option<&str>,
    assets: Vec<Value>,
) -> Value {
    json!({
        "page_html": page_html,
        "page_css": page_css,
        "design_css": design_css,
        "global_css": global_css,
        "assets_manifest": {
            "assets": assets
        }
    })
}

fn complete_asset_manifest() -> Vec<Value> {
    vec![
        json!({
            "filename": "hero.png",
            "kind": "image",
            "description": "Replacement hero",
            "generation_prompt": "A wide deterministic golden hero",
            "size": "1536x1024",
            "svg_code": ""
        }),
        json!({
            "filename": "icons/plus.svg",
            "kind": "svg",
            "description": "New plus icon",
            "generation_prompt": "A minimal plus icon rendered as an SVG",
            "size": null,
            "svg_code": SVG_SOURCE
        }),
        json!({
            "filename": "generated-glow",
            "kind": "css_generated",
            "description": "Rendered with CSS",
            "generation_prompt": "A decorative glow rendered entirely with CSS",
            "size": null,
            "svg_code": ""
        }),
    ]
}

fn valid_update(marker: &str) -> Value {
    update_value(
        updated_page_html(marker, false),
        None,
        None,
        None,
        Vec::new(),
    )
}

fn invalid_update_cases() -> Vec<(Value, &'static str)> {
    let mut empty_html = valid_update("empty-html");
    empty_html["page_html"] = json!("   ");

    let mut empty_page_css = valid_update("empty-page-css");
    empty_page_css["page_css"] = json!("   ");

    let mut empty_design_css = valid_update("empty-design-css");
    empty_design_css["design_css"] = json!("\n");

    let mut empty_global_css = valid_update("empty-global-css");
    empty_global_css["global_css"] = json!("\t");

    let mut unsafe_filename = valid_update("unsafe-filename");
    unsafe_filename["assets_manifest"]["assets"] = json!([
        svg_asset("../escape.svg", SVG_SOURCE)
    ]);

    let mut duplicate_filename = valid_update("duplicate-filename");
    duplicate_filename["assets_manifest"]["assets"] = json!([
        svg_asset("same.svg", SVG_SOURCE),
        svg_asset("same.svg", SVG_SOURCE)
    ]);

    let mut empty_svg = valid_update("empty-svg");
    empty_svg["assets_manifest"]["assets"] = json!([
        svg_asset("empty.svg", "   ")
    ]);

    let mut empty_image_prompt = valid_update("empty-image-prompt");
    empty_image_prompt["assets_manifest"]["assets"] = json!([{
        "filename": "empty.png",
        "kind": "image",
        "description": "Invalid image",
        "generation_prompt": "   ",
        "size": "1024x1024",
        "svg_code": ""
    }]);

    vec![
        (
            empty_html,
            "updated page_html is empty or does not contain an html element",
        ),
        (
            empty_page_css,
            "page_css must be null or a non-empty string",
        ),
        (
            empty_design_css,
            "design_css must be null or a non-empty string",
        ),
        (
            empty_global_css,
            "global_css must be null or a non-empty string",
        ),
        (unsafe_filename, "assets[0] ('../escape.svg'): invalid filename"),
        (
            duplicate_filename,
            "assets[1] ('same.svg'): duplicate staged filename",
        ),
        (empty_svg, "assets[0] ('empty.svg'): SVG source must not be empty"),
        (
            empty_image_prompt,
            "assets[0] ('empty.png'): image generation prompt must not be empty",
        ),
    ]
}

fn svg_asset(filename: &str, svg_code: &str) -> Value {
    json!({
        "filename": filename,
        "kind": "svg",
        "description": "Test SVG",
        "generation_prompt": "A deterministic SVG fixture",
        "size": null,
        "svg_code": svg_code
    })
}

#[derive(Clone, Debug)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

impl RecordedRequest {
    fn json_body(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|error| {
            panic!(
                "request body for {} was not JSON: {error}\nbody: {}",
                self.path, self.body
            )
        })
    }
}

fn assert_common_request_headers(requests: &[RecordedRequest]) {
    for request in requests {
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer integration-test-key")
        );
        assert!(
            request
                .headers
                .get("content-type")
                .is_some_and(|value| value.starts_with("application/json")),
            "request had no JSON content type: {request:#?}"
        );
    }
}

struct Stub {
    expected_path: String,
    response: MockResponse,
}

impl Stub {
    fn output(path: &str, value: Value) -> Self {
        let output_text = serde_json::to_string(&value)
            .expect("mock structured output should serialize");
        Self::output_text(path, &output_text)
    }

    fn output_text(path: &str, output_text: &str) -> Self {
        Self::json(
            path,
            200,
            json!({
                "output_text": output_text
            }),
        )
    }

    fn json(path: &str, status: u16, value: Value) -> Self {
        Self {
            expected_path: path.to_string(),
            response: MockResponse {
                status,
                body: value.to_string(),
            },
        }
    }
}

struct MockResponse {
    status: u16,
    body: String,
}

struct MockOpenAi {
    address: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stubs: Arc<Mutex<VecDeque<Stub>>>,
    errors: Arc<Mutex<Vec<String>>>,
    shutdown: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl MockOpenAi {
    fn start(stubs: Vec<Stub>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .expect("mock OpenAI server should bind");
        listener
            .set_nonblocking(true)
            .expect("mock listener should become nonblocking");
        let address = listener
            .local_addr()
            .expect("mock listener should have an address")
            .to_string();

        let requests = Arc::new(Mutex::new(Vec::new()));
        let stubs = Arc::new(Mutex::new(VecDeque::from(stubs)));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let (shutdown_sender, shutdown_receiver) = mpsc::channel();

        let worker_requests = Arc::clone(&requests);
        let worker_stubs = Arc::clone(&stubs);
        let worker_errors = Arc::clone(&errors);

        let worker = thread::spawn(move || loop {
            match shutdown_receiver.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }

            match listener.accept() {
                Ok((mut stream, _peer)) => {
                    if let Err(error) = serve_one_request(
                        &mut stream,
                        &worker_requests,
                        &worker_stubs,
                        &worker_errors,
                    ) {
                        worker_errors
                            .lock()
                            .expect("mock error list lock should not be poisoned")
                            .push(error);
                    }
                }

                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }

                Err(error) => {
                    worker_errors
                        .lock()
                        .expect("mock error list lock should not be poisoned")
                        .push(format!("mock listener failed: {error}"));
                    break;
                }
            }
        });

        Self {
            address,
            requests,
            stubs,
            errors,
            shutdown: Some(shutdown_sender),
            worker: Some(worker),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }

    fn finish(mut self) -> Vec<RecordedRequest> {
        self.stop();

        let errors = self
            .errors
            .lock()
            .expect("mock error list lock should not be poisoned")
            .clone();
        assert!(
            errors.is_empty(),
            "mock OpenAI server errors:\n{}",
            errors.join("\n")
        );

        let remaining = self
            .stubs
            .lock()
            .expect("mock stub lock should not be poisoned")
            .len();
        assert_eq!(
            remaining, 0,
            "{remaining} expected mock OpenAI response(s) were not used"
        );

        let requests = self
            .requests
            .lock()
            .expect("mock request lock should not be poisoned")
            .clone();

        requests
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }

        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .expect("mock OpenAI server thread should not panic");
        }
    }
}

impl Drop for MockOpenAi {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve_one_request(
    stream: &mut TcpStream,
    requests: &Arc<Mutex<Vec<RecordedRequest>>>,
    stubs: &Arc<Mutex<VecDeque<Stub>>>,
    errors: &Arc<Mutex<Vec<String>>>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("failed to set mock read timeout: {error}"))?;

    let request = read_http_request(stream)?;
    let request_path = request.path.clone();

    requests
        .lock()
        .map_err(|_| "mock request lock was poisoned".to_string())?
        .push(request);

    let stub = stubs
        .lock()
        .map_err(|_| "mock stub lock was poisoned".to_string())?
        .pop_front();

    let response = match stub {
        Some(stub) if stub.expected_path == request_path => stub.response,

        Some(stub) => {
            errors
                .lock()
                .map_err(|_| "mock error lock was poisoned".to_string())?
                .push(format!(
                    "expected request path {}, received {}",
                    stub.expected_path, request_path
                ));

            unexpected_request_response()
        }

        None => {
            errors
                .lock()
                .map_err(|_| "mock error lock was poisoned".to_string())?
                .push(format!(
                    "received unexpected extra request at {request_path}"
                ));

            unexpected_request_response()
        }
    };

    write_http_response(stream, response)
}

fn unexpected_request_response() -> MockResponse {
    MockResponse {
        status: 400,
        body: json!({
            "error": {
                "message": "unexpected integration-test request"
            }
        })
        .to_string(),
    }
}

fn read_http_request(stream: &mut TcpStream) -> Result<RecordedRequest, String> {
    let mut received = Vec::new();
    let mut chunk = [0_u8; 8192];
    let header_end;

    loop {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| format!("failed to read mock request: {error}"))?;

        if read == 0 {
            return Err(
                "client closed mock connection before sending headers".to_string()
            );
        }

        received.extend_from_slice(&chunk[..read]);

        if let Some(end) = find_header_end(&received) {
            header_end = end;
            break;
        }

        if received.len() > 1024 * 1024 {
            return Err("mock request headers exceeded 1 MiB".to_string());
        }
    }

    let header_text = std::str::from_utf8(&received[..header_end - 4])
        .map_err(|error| format!("mock request headers were not UTF-8: {error}"))?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "mock request had no request line".to_string())?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| "mock request had no method".to_string())?
        .to_string();
    let path = request_parts
        .next()
        .ok_or_else(|| "mock request had no path".to_string())?
        .to_string();

    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("malformed mock request header: {line}"))?;
        headers.insert(
            name.trim().to_ascii_lowercase(),
            value.trim().to_string(),
        );
    }

    let content_length = headers
        .get("content-length")
        .ok_or_else(|| "mock request had no Content-Length".to_string())?
        .parse::<usize>()
        .map_err(|error| format!("invalid mock Content-Length: {error}"))?;
    let total_length = header_end + content_length;

    while received.len() < total_length {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| format!("failed to read mock request body: {error}"))?;

        if read == 0 {
            return Err(
                "client closed mock connection before sending its body".to_string()
            );
        }

        received.extend_from_slice(&chunk[..read]);
    }

    let body = String::from_utf8(received[header_end..total_length].to_vec())
        .map_err(|error| format!("mock request body was not UTF-8: {error}"))?;

    Ok(RecordedRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn write_http_response(
    stream: &mut TcpStream,
    response: MockResponse,
) -> Result<(), String> {
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        status => {
            return Err(format!(
                "test mock has no reason phrase for HTTP status {status}"
            ));
        }
    };
    let body = response.body.as_bytes();
    let headers = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        response.status,
        reason,
        body.len(),
    );

    stream
        .write_all(headers.as_bytes())
        .and_then(|()| stream.write_all(body))
        .and_then(|()| stream.flush())
        .map_err(|error| format!("failed to write mock response: {error}"))
}
