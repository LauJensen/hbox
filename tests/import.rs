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
const SCREENSHOT_NAME: &str = "reference.png";
const API_KEY: &str = "integration-test-key";
const TEXT_MODEL: &str = "test-text-model";
const IMAGE_MODEL: &str = "test-image-model";
const IMAGE_QUALITY: &str = "high";

const TINY_PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

const GENERATED_HEADER: &str =
    r#"<header id="generated-header">Generated header</header>"#;
const GENERATED_FOOTER: &str =
    r#"<footer id="generated-footer">Generated footer</footer>"#;
const EXISTING_HEADER: &str =
    r#"<header id="existing-header">Existing header</header>"#;
const EXISTING_FOOTER: &str =
    r#"<footer id="existing-footer">Existing footer</footer>"#;

const SVG_SOURCE: &str = concat!(
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">"#,
    r#"<circle cx="5" cy="5" r="4"/>"#,
    "</svg>",
);

#[test]
fn import_writes_a_complete_preview_build_and_cache() {
    let fixture = Fixture::initialized();
    let files = generated_files(
        imported_page_html("complete-import", true),
        Some(GENERATED_HEADER),
        Some(GENERATED_FOOTER),
        Some(".imported-page { display: grid; gap: 1rem; }"),
        Some(":root { --brand-accent: #d4a017; }"),
        complete_asset_manifest(),
    );

    let mock = MockOpenAi::start(vec![
        Stub::output("/v1/responses", design_brief()),
        Stub::output("/v1/responses", files),
        Stub::json(
            "/v1/images/generations",
            200,
            json!({ "data": [{ "b64_json": TINY_PNG_BASE64 }] }),
        ),
    ]);

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "Imported design into sites/.preview-integration.test-1"
        ),
        "unexpected stdout:\n{}",
        stdout(&output)
    );

    let requests = mock.finish();
    assert_eq!(requests.len(), 3);
    assert_common_request_headers(&requests);

    let analyze_request = requests[0].json_body();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(analyze_request["model"], json!(TEXT_MODEL));
    assert_eq!(analyze_request["store"], json!(false));
    assert_eq!(
        analyze_request
            .pointer("/text/format/name")
            .and_then(Value::as_str),
        Some("hbox_design_spec")
    );

    let expected_data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD
            .encode(Fixture::screenshot_bytes())
    );
    assert_eq!(
        analyze_request
            .pointer("/input/1/content/1/image_url")
            .and_then(Value::as_str),
        Some(expected_data_url.as_str())
    );

    let generate_request = requests[1].json_body();
    assert_eq!(requests[1].path, "/v1/responses");
    assert_eq!(generate_request["model"], json!(TEXT_MODEL));
    assert_eq!(generate_request["store"], json!(false));
    assert_eq!(
        generate_request
            .pointer("/text/format/name")
            .and_then(Value::as_str),
        Some("hbox_site_files")
    );

    let generation_prompt = generate_request
        .pointer("/input/1/content/0/text")
        .and_then(Value::as_str)
        .expect("site generation request should contain a text prompt");
    assert!(generation_prompt.contains("partials/header.html"));
    assert!(generation_prompt.contains("partials/footer.html"));
    assert!(generation_prompt.contains("Test Brand"));

    let image_request = requests[2].json_body();
    assert_eq!(requests[2].path, "/v1/images/generations");
    assert_eq!(image_request["model"], json!(IMAGE_MODEL));
    assert_eq!(image_request["prompt"], json!("A tiny gold test square"));
    assert_eq!(image_request["size"], json!("1024x1024"));
    assert_eq!(image_request["quality"], json!(IMAGE_QUALITY));
    assert_eq!(image_request["output_format"], json!("png"));
    assert_eq!(image_request["n"], json!(1));

    let original = fixture.site_source(SITE_NAME);
    assert!(original.join("pages/index.html").is_file());
    assert!(!original.join("pages/about.html").exists());
    assert!(!original.join("partials/header.html").exists());
    assert!(!original.join("partials/footer.html").exists());
    assert!(!original.join("public/images/hero.png").exists());
    assert!(read_text(original.join("design.css")).trim().is_empty());

    let preview = fixture.preview_source(SITE_NAME, 1);
    assert!(preview.join("pages/index.html").is_file());
    assert!(preview.join("blogposts/hello-hbox.md").is_file());
    assert!(preview.join("pages/about.html").is_file());
    assert_eq!(
        read_text(preview.join("partials/header.html")).trim(),
        GENERATED_HEADER
    );
    assert_eq!(
        read_text(preview.join("partials/footer.html")).trim(),
        GENERATED_FOOTER
    );
    assert_eq!(
        read_text(preview.join("pages/about.css")).trim(),
        ".imported-page { display: grid; gap: 1rem; }"
    );

    let design_css = read_text(preview.join("design.css"));
    assert!(design_css.contains(
        "/* hbox design import:start about */"
    ));
    assert!(design_css.contains(":root { --brand-accent: #d4a017; }"));
    assert!(design_css.contains(
        "/* hbox design import:end about */"
    ));

    let expected_png = base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG_BASE64)
        .expect("test PNG fixture should be valid base64");
    assert_eq!(
        fs::read(preview.join("public/images/hero.png"))
            .expect("generated PNG should be installed"),
        expected_png
    );
    assert_eq!(
        read_text(preview.join("public/images/brand/mark.svg")),
        SVG_SOURCE
    );
    assert!(
        !preview.join("public/images/generated-glow").exists(),
        "CSS-generated assets must not create files"
    );
    assert!(
        !preview.join(".hbox/import-assets-tmp").exists(),
        "asset staging directory should be removed after installation"
    );

    let cache = read_json(
        preview.join(".hbox/imports/about.import-result.json")
    );
    assert_eq!(
        cache
            .pointer("/design_spec/page/brand_name")
            .and_then(Value::as_str),
        Some("Test Brand")
    );
    assert_eq!(
        cache
            .pointer("/files/assets_manifest/assets")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3)
    );

    let design_spec =
        read_json(preview.join(".hbox/imports/about.design-spec.json"));
    assert_eq!(
        design_spec
            .pointer("/theme/accent")
            .and_then(Value::as_str),
        Some("#d4a017")
    );

    let manifest = read_json(
        preview.join(".hbox/imports/about.assets-manifest.json")
    );
    assert_eq!(
        manifest["assets"]
            .as_array()
            .expect("manifest assets should be an array")
            .len(),
        3
    );

    let output_dir = fixture.preview_output(SITE_NAME, 1);
    assert!(output_dir.join("index.html").is_file());
    assert!(
        output_dir
            .join("blog/en/hello-hbox/index.html")
            .is_file()
    );
    assert!(output_dir.join("images/hero.png").is_file());
    assert!(output_dir.join("images/brand/mark.svg").is_file());

    let built_page =
        read_text(output_dir.join("en/about/index.html"));
    assert!(built_page.contains(r#"id="generated-header""#));
    assert!(built_page.contains(r#"id="generated-footer""#));
    assert!(built_page.contains(r#"id="hbox-page-css""#));
    assert!(built_page.contains(r#"href="/global.css""#));
    assert!(built_page.contains(r#"href="/design."#));
    assert!(built_page.contains("complete-import"));

    let design_outputs = fs::read_dir(&output_dir)
        .expect("preview output should be readable")
        .map(|entry| entry.expect("directory entry should be readable"))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("design.") && name.ends_with(".css"))
        .collect::<Vec<_>>();
    assert_eq!(
        design_outputs.len(),
        1,
        "expected one content-hashed design stylesheet, got {design_outputs:?}"
    );

    assert_no_transient_output_artifacts(&fixture, SITE_NAME, 1);
}

#[test]
fn import_uses_the_next_available_preview_number() {
    let fixture = Fixture::initialized();

    let first_preview = fixture.preview_source(SITE_NAME, 1);
    write_file(first_preview.join("sentinel.txt"), "keep preview one");

    let first_output = fixture.preview_output(SITE_NAME, 1);
    write_file(first_output.join("sentinel.txt"), "keep output one");

    let mock = MockOpenAi::start(successful_text_stubs(generated_files(
        imported_page_html("second-preview", false),
        Some(GENERATED_HEADER),
        Some(GENERATED_FOOTER),
        None,
        None,
        Vec::new(),
    )));

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );

    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "Imported design into sites/.preview-integration.test-2"
        )
    );
    assert_eq!(mock.finish().len(), 2);

    assert_eq!(
        read_text(first_preview.join("sentinel.txt")),
        "keep preview one"
    );
    assert_eq!(
        read_text(first_output.join("sentinel.txt")),
        "keep output one"
    );
    assert!(
        fixture
            .preview_source(SITE_NAME, 2)
            .join("pages/about.html")
            .is_file()
    );
    assert!(
        fixture
            .preview_output(SITE_NAME, 2)
            .join("en/about/index.html")
            .is_file()
    );
    assert_no_transient_output_artifacts(&fixture, SITE_NAME, 2);
}

#[test]
fn reimport_preserves_existing_partials_and_replaces_only_owned_files() {
    let fixture = Fixture::initialized();
    let original = fixture.site_source(SITE_NAME);

    write_file(original.join("partials/header.html"), EXISTING_HEADER);
    write_file(original.join("partials/footer.html"), EXISTING_FOOTER);
    write_file(
        original.join("pages/about.html"),
        imported_page_html("old-page", false),
    );
    write_file(
        original.join("pages/about.css"),
        ".old-page { color: red; }",
    );
    write_file(
        original.join("design.css"),
        concat!(
            ".developer-owned { color: black; }\n\n",
            "/* hbox design import:start about */\n",
            ".old-import { color: red; }\n",
            "/* hbox design import:end about */\n\n",
            ".also-developer-owned { display: block; }\n",
        ),
    );

    let mock = MockOpenAi::start(successful_text_stubs(generated_files(
        imported_page_html("replacement-page", false),
        None,
        None,
        None,
        Some(".new-import { color: green; }"),
        Vec::new(),
    )));

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );

    assert_success(&output);
    let requests = mock.finish();
    assert_eq!(requests.len(), 2);

    let generate_body = requests[1].json_body();
    let prompt = generate_body
        .pointer("/input/1/content/0/text")
        .and_then(Value::as_str)
        .expect("generation prompt should exist");
    assert!(prompt.contains("already has partials/header.html"));
    assert!(prompt.contains("already has partials/footer.html"));

    let preview = fixture.preview_source(SITE_NAME, 1);
    assert_eq!(
        read_text(preview.join("partials/header.html")),
        EXISTING_HEADER
    );
    assert_eq!(
        read_text(preview.join("partials/footer.html")),
        EXISTING_FOOTER
    );
    assert!(
        read_text(preview.join("pages/about.html"))
            .contains("replacement-page")
    );
    assert!(
        !preview.join("pages/about.css").exists(),
        "null page_css should remove stale page CSS in the preview"
    );

    let design_css = read_text(preview.join("design.css"));
    assert!(design_css.contains(".developer-owned { color: black; }"));
    assert!(design_css.contains(
        ".also-developer-owned { display: block; }"
    ));
    assert!(design_css.contains(".new-import { color: green; }"));
    assert!(!design_css.contains(".old-import { color: red; }"));
    assert_eq!(
        design_css
            .matches("/* hbox design import:start about */")
            .count(),
        1
    );
    assert_eq!(
        design_css
            .matches("/* hbox design import:end about */")
            .count(),
        1
    );

    let built_page = read_text(
        fixture
            .preview_output(SITE_NAME, 1)
            .join("en/about/index.html"),
    );
    assert!(built_page.contains(r#"id="existing-header""#));
    assert!(built_page.contains(r#"id="existing-footer""#));
    assert!(!built_page.contains(r#"id="hbox-page-css""#));

    // Import always works in a preview. The accepted source remains untouched.
    assert!(
        read_text(original.join("pages/about.html")).contains("old-page")
    );
    assert!(original.join("pages/about.css").is_file());
    assert!(
        read_text(original.join("design.css"))
            .contains(".old-import { color: red; }")
    );
}

#[test]
fn reimport_handles_one_missing_partial_and_removes_an_obsolete_design_block() {
    let fixture = Fixture::initialized();
    let original = fixture.site_source(SITE_NAME);

    write_file(original.join("partials/header.html"), EXISTING_HEADER);
    write_file(
        original.join("design.css"),
        concat!(
            ".developer-owned { color: black; }\n\n",
            "/* hbox design import:start about */\n",
            ".obsolete-import { color: red; }\n",
            "/* hbox design import:end about */\n",
        ),
    );

    let mock = MockOpenAi::start(successful_text_stubs(generated_files(
        imported_page_html("no-design-css", false),
        None,
        Some(GENERATED_FOOTER),
        None,
        None,
        Vec::new(),
    )));

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );

    assert_success(&output);
    assert_eq!(mock.finish().len(), 2);

    let design_css = read_text(
        fixture
            .preview_source(SITE_NAME, 1)
            .join("design.css"),
    );
    assert_eq!(design_css, ".developer-owned { color: black; }\n");
    assert!(!design_css.contains("hbox design import:"));
    assert!(!design_css.contains("obsolete-import"));

    let preview = fixture.preview_source(SITE_NAME, 1);
    assert_eq!(
        read_text(preview.join("partials/header.html")),
        EXISTING_HEADER
    );
    assert_eq!(
        read_text(preview.join("partials/footer.html")).trim(),
        GENERATED_FOOTER
    );
}

#[test]
fn import_rejects_invalid_slugs_before_creating_a_preview() {
    let fixture = Fixture::initialized();
    let mock = MockOpenAi::start(Vec::new());

    for (slug, expected_error) in [
        ("", "page slug cannot be empty"),
        ("About", "invalid page slug"),
        ("about/us", "invalid page slug"),
        ("about.html", "page slug should not include .html"),
    ] {
        let output = run_import(
            &fixture,
            &mock,
            SITE_NAME,
            SCREENSHOT_NAME,
            slug,
        );

        assert_failure_contains(&output, expected_error);
        assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    }

    assert!(
        mock.finish().is_empty(),
        "invalid slugs must fail before an API request"
    );
}

#[test]
fn import_checks_site_and_configuration_before_creating_a_preview() {
    let uninitialized = Fixture::empty();
    uninitialized.write_screenshot(SCREENSHOT_NAME);
    let mock = MockOpenAi::start(Vec::new());

    let output = run_import(
        &uninitialized,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );
    assert_failure_contains(&output, "Site is not initialized");
    assert_no_preview_artifacts(&uninitialized, SITE_NAME, 1);

    let initialized = Fixture::initialized();
    let mut command = import_command(
        &initialized,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );
    command.env_remove("OPENAI_API_KEY");

    let output = command
        .output()
        .expect("hbox import process should start");
    assert_failure_contains(&output, "OPENAI_API_KEY is not set");
    assert_no_preview_artifacts(&initialized, SITE_NAME, 1);

    assert!(
        mock.finish().is_empty(),
        "preflight failures must not contact OpenAI"
    );
}

#[test]
fn import_cleans_up_when_the_screenshot_is_missing_or_unsupported() {
    let fixture = Fixture::initialized();
    let mock = MockOpenAi::start(Vec::new());

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        "missing.png",
        "about",
    );
    assert_failure_contains(&output, "failed to read screenshot");
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);

    write_file(fixture.root.join("reference.gif"), "not really a GIF");
    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        "reference.gif",
        "about",
    );
    assert_failure_contains(&output, "unsupported screenshot MIME type");
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);

    assert!(
        fixture.site_source(SITE_NAME).join("hbox.toml").is_file(),
        "cleanup must not remove the accepted site"
    );
    assert!(
        mock.finish().is_empty(),
        "invalid screenshots must fail before an API request"
    );
}

#[test]
fn import_cleans_up_when_the_analysis_request_fails() {
    let fixture = Fixture::initialized();
    let mock = MockOpenAi::start(vec![Stub::json(
        "/v1/responses",
        400,
        json!({
            "error": {
                "message": "deliberate analysis failure"
            }
        }),
    )]);

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );

    assert_failure_contains(&output, "deliberate analysis failure");
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    assert_original_has_no_import(&fixture);
    assert_eq!(mock.finish().len(), 1);
}

#[test]
fn import_cleans_up_when_generated_files_violate_partial_rules() {
    let fixture = Fixture::initialized();
    let invalid_files = generated_files(
        imported_page_html("invalid-partials", false),
        None,
        None,
        None,
        None,
        Vec::new(),
    );
    let mock = MockOpenAi::start(successful_text_stubs(invalid_files));

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );

    assert_failure_contains(
        &output,
        "header_html must contain the missing partial",
    );
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    assert_original_has_no_import(&fixture);
    assert_eq!(mock.finish().len(), 2);
}

#[test]
fn import_cleans_up_when_the_asset_manifest_is_unsafe() {
    let fixture = Fixture::initialized();
    let unsafe_asset = json!({
        "filename": "../escape.svg",
        "kind": "svg",
        "description": "Must be rejected",
        "generation_prompt": "An SVG fixture with an unsafe filename",
        "size": "auto",
        "svg_code": SVG_SOURCE
    });
    let files = generated_files(
        imported_page_html("unsafe-asset", false),
        Some(GENERATED_HEADER),
        Some(GENERATED_FOOTER),
        None,
        None,
        vec![unsafe_asset],
    );
    let mock = MockOpenAi::start(successful_text_stubs(files));

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );

    assert_failure_contains(
        &output,
        "assets[0] ('../escape.svg'): invalid filename",
    );
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    assert_original_has_no_import(&fixture);
    assert!(!fixture.root.join("escape.svg").exists());
    assert_eq!(mock.finish().len(), 2);
}

#[test]
fn import_cleans_up_when_image_generation_fails() {
    let fixture = Fixture::initialized();
    let image_asset = json!({
        "filename": "hero.png",
        "kind": "image",
        "description": "Test hero",
        "generation_prompt": "A tiny gold test square",
        "size": "1024x1024",
        "svg_code": ""
    });
    let files = generated_files(
        imported_page_html("image-failure", true),
        Some(GENERATED_HEADER),
        Some(GENERATED_FOOTER),
        None,
        None,
        vec![image_asset],
    );
    let mock = MockOpenAi::start(vec![
        Stub::output("/v1/responses", design_brief()),
        Stub::output("/v1/responses", files),
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

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );

    assert_failure_contains(&output, "deliberate image failure");
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    assert_original_has_no_import(&fixture);
    assert_eq!(mock.finish().len(), 3);
}

#[test]
fn import_cleans_up_source_and_staging_output_when_build_fails() {
    let fixture = Fixture::initialized();
    let broken_html = concat!(
        "<!doctype html><html lang=\"en\"><head><title>Broken</title>",
        "<body>",
        "{% include \"partials/header.html\" %}",
        "<main>Missing closing head</main>",
        "{% include \"partials/footer.html\" %}",
        "</body></html>",
    );
    let files = generated_files(
        broken_html.to_string(),
        Some(GENERATED_HEADER),
        Some(GENERATED_FOOTER),
        Some(".valid-css { color: black; }"),
        Some(".also-valid { color: white; }"),
        Vec::new(),
    );
    let mock = MockOpenAi::start(successful_text_stubs(files));

    let output = run_import(
        &fixture,
        &mock,
        SITE_NAME,
        SCREENSHOT_NAME,
        "about",
    );

    assert_failure_contains(&output, "Document is missing </head>");
    assert_no_preview_artifacts(&fixture, SITE_NAME, 1);
    assert_original_has_no_import(&fixture);
    assert_eq!(mock.finish().len(), 2);
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
        fixture.write_screenshot(SCREENSHOT_NAME);
        fixture
    }

    fn screenshot_bytes() -> &'static [u8] {
        b"deterministic fake screenshot bytes"
    }

    fn write_screenshot(&self, filename: &str) {
        fs::write(self.root.join(filename), Self::screenshot_bytes())
            .expect("screenshot fixture should be writable");
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

fn import_command(
    fixture: &Fixture,
    mock: &MockOpenAi,
    site_name: &str,
    screenshot: &str,
    slug: &str,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hbox"));

    command
        .current_dir(&fixture.root)
        .arg("import")
        .arg(site_name)
        .arg(screenshot)
        .arg(slug)
        .arg("--threads")
        .arg("1")
        .env("OPENAI_API_KEY", API_KEY)
        .env("OPENAI_BASE_URL", mock.base_url())
        .env("OPENAI_MODEL", TEXT_MODEL)
        .env("OPENAI_IMAGE_MODEL", IMAGE_MODEL)
        .env("OPENAI_IMAGE_QUALITY", IMAGE_QUALITY)
        // A developer or CI machine may have proxy variables set. The mock
        // server is local and should never be routed through those proxies.
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env("NO_PROXY", "127.0.0.1,localhost");

    command
}

fn run_import(
    fixture: &Fixture,
    mock: &MockOpenAi,
    site_name: &str,
    screenshot: &str,
    slug: &str,
) -> Output {
    import_command(fixture, mock, site_name, screenshot, slug)
        .output()
        .expect("hbox import process should start")
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
            "failed import left an artifact behind: {}",
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
            "successful import left a transient artifact behind: {}",
            path.display()
        );
    }
}

fn assert_original_has_no_import(fixture: &Fixture) {
    let original = fixture.site_source(SITE_NAME);
    assert!(original.join("hbox.toml").is_file());
    assert!(!original.join("pages/about.html").exists());
    assert!(
        !original
            .join(".hbox/imports/about.import-result.json")
            .exists()
    );
}

fn write_file(path: impl AsRef<Path>, contents: impl AsRef<str>) {
    let path = path.as_ref();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .expect("test fixture parent directory should be creatable");
    }

    fs::write(path, contents.as_ref().as_bytes())
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
}

fn read_text(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn read_json(path: impl AsRef<Path>) -> Value {
    let path = path.as_ref();
    let text = read_text(path);

    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("failed to parse JSON from {}: {error}", path.display())
    })
}

fn imported_page_html(marker: &str, include_image: bool) -> String {
    let image = if include_image {
        r#"<img id="generated-image" src="/images/hero.png" alt="Test hero">"#
    } else {
        ""
    };

    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Imported page</title>
  </head>
  <body>
    {{% include "partials/header.html" %}}
    <main class="imported-page">
      <p id="import-marker">{marker}</p>
      {image}
    </main>
    {{% include "partials/footer.html" %}}
  </body>
</html>"#
    )
}

fn generated_files(
    page_html: String,
    header_html: Option<&str>,
    footer_html: Option<&str>,
    page_css: Option<&str>,
    design_css: Option<&str>,
    assets: Vec<Value>,
) -> Value {
    json!({
        "page_html": page_html,
        "page_css": page_css,
        "design_css": design_css,
        "assets_manifest": {
            "assets": assets
        },
        "header_html": header_html,
        "footer_html": footer_html
    })
}

fn complete_asset_manifest() -> Vec<Value> {
    vec![
        json!({
            "filename": "hero.png",
            "kind": "image",
            "description": "Test hero",
            "generation_prompt": "A tiny gold test square",
            "size": "1024x1024",
            "svg_code": ""
        }),
        json!({
            "filename": "brand/mark.svg",
            "kind": "svg",
            "description": "Test brand mark",
            "generation_prompt": "svg mark",
            "size": "auto",
            "svg_code": SVG_SOURCE
        }),
        json!({
            "filename": "generated-glow",
            "kind": "css_generated",
            "description": "Rendered by CSS",
            "generation_prompt": "css blob",
            "size": "auto",
            "svg_code": ""
        }),
    ]
}

fn design_brief() -> Value {
    json!({
        "page": {
            "kind": "landing_page",
            "brand_name": "Test Brand",
            "summary": "A deterministic integration-test design",
            "visual_style": "Minimal"
        },
        "theme": {
            "mode": "light",
            "background": "#ffffff",
            "surface": "#f8f8f8",
            "surface_alt": "#eeeeee",
            "text": "#111111",
            "muted": "#666666",
            "accent": "#d4a017",
            "accent_soft": "#fff3c4",
            "border": "#dddddd",
            "mood": "Confident"
        },
        "layout_system": {
            "design_system": "hbox",
            "container": "standard",
            "density": "normal",
            "section_style": "flat"
        },
        "component_system": {
            "backgrounds": "Solid",
            "buttons": "Simple",
            "cards": "Flat",
            "icons": "Line",
            "typography": "System sans",
            "media_panels": "Contained",
            "badges": "Compact",
            "navigation": "Horizontal",
            "links": "Underlined",
            "logo_clouds": "Monochrome",
            "forbidden_interpretations": []
        },
        "sections": [{
            "id": "hero",
            "type": "hero",
            "layout": "centered",
            "visual_role": "primary_hero",
            "visible_text": ["Imported page"],
            "items": [],
            "asset_roles": ["hero"],
            "notes": "Deterministic test section"
        }],
        "assets": []
    })
}

fn successful_text_stubs(files: Value) -> Vec<Stub> {
    vec![
        Stub::output("/v1/responses", design_brief()),
        Stub::output("/v1/responses", files),
    ]
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

            MockResponse {
                status: 400,
                body: json!({
                    "error": {
                        "message": "unexpected integration-test request path"
                    }
                })
                .to_string(),
            }
        }

        None => {
            errors
                .lock()
                .map_err(|_| "mock error lock was poisoned".to_string())?
                .push(format!(
                    "received unexpected extra request at {request_path}"
                ));

            MockResponse {
                status: 400,
                body: json!({
                    "error": {
                        "message": "unexpected extra integration-test request"
                    }
                })
                .to_string(),
            }
        }
    };

    write_http_response(stream, response)
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
