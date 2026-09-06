use anyhow::{Context, Result};
use html_escape::decode_html_entities;
use regex::{Captures, Regex};
use std::sync::LazyLock;
use syntect::{
    easy::HighlightLines,
    highlighting::{Theme, ThemeSet},
    html::{styled_line_to_highlighted_html, IncludeBackground},
    parsing::{SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};

use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodeTheme {
    Github,
    SolarizedDark,
    SolarizedLight,
    EightiesDark,
    MochaDark,

    #[default]
    OceanDark,

    OceanLight,
}

impl CodeTheme {
    pub fn syntect_name(self) -> &'static str {
        match self {
            Self::Github         => "InspiredGitHub",
            Self::SolarizedDark  => "Solarized (dark)",
            Self::SolarizedLight => "Solarized (light)",
            Self::EightiesDark   => "base16-eighties.dark",
            Self::MochaDark      => "base16-mocha.dark",
            Self::OceanDark      => "base16-ocean.dark",
            Self::OceanLight     => "base16-ocean.light",
        }
    }
}

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| {
    SyntaxSet::load_defaults_newlines()
});

static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(|| {
    ThemeSet::load_defaults()
});

fn syntect_theme(theme: CodeTheme) -> &'static Theme {
    &THEME_SET.themes[theme.syntect_name()]
}

static CODE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<pre[^>]*>\s*<code(?P<attrs>[^>]*)>(?P<code>.*?)</code>\s*</pre>"#,
    )
    .expect("valid code block regex")
});

static CLASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"class=["'][^"']*language-(?P<lang>[a-zA-Z0-9_+-]+)[^"']*["']"#)
        .expect("valid class regex")
});

static LANG_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"language=["'](?P<lang>[a-zA-Z0-9_+-]+)["']"#)
        .expect("valid language attr regex")
});

pub fn highlight_code_in_html(theme: CodeTheme, input: &str) -> Result<String> {
    let mut out = String::new();
    let mut last_end = 0;

    for caps in CODE_BLOCK_RE.captures_iter(input) {
        let whole = caps
            .get(0)
            .expect("whole match exists");

        out.push_str(&input[last_end..whole.start()]);

        let attrs = caps
            .name("attrs")
            .map(|m| m.as_str())
            .unwrap_or("");

        let raw_code = caps
            .name("code")
            .map(|m| m.as_str())
            .unwrap_or("");

        let language = language_from_attrs(attrs);
        let decoded_code = decode_html_entities(raw_code).to_string();

        let highlighted = highlight_code_block(
            theme,
            &decoded_code,
            language.as_deref(),
        )?;

        out.push_str(&highlighted);
        last_end = whole.end();
    }

    out.push_str(&input[last_end..]);

    Ok(out)
}

fn highlight_code_block(theme: CodeTheme, code: &str, language: Option<&str>) -> Result<String> {
    let syntax = syntax_for_language(language);
    let theme = syntect_theme(theme);

    let mut highlighter = HighlightLines::new(syntax, theme);

    let lang_class = language
        .map(normalize_language)
        .unwrap_or("text");

    let mut out = String::new();

    out.push_str(&format!(
        r#"<pre class="hbox-code language-{lang_class}"><code class="language-{lang_class}">"#
    ));

    for line in LinesWithEndings::from(code) {
        let ranges = highlighter
            .highlight_line(line, &SYNTAX_SET)
            .context("failed to highlight code line")?;

        let line_html = styled_line_to_highlighted_html(
            &ranges,
            IncludeBackground::No,
        )?;

        out.push_str(&line_html);
    }

    out.push_str("</code></pre>");

    Ok(out)
}

fn language_from_attrs(attrs: &str) -> Option<String> {
    CLASS_RE
        .captures(attrs)
        .and_then(lang_capture)
        .or_else(|| {
            LANG_ATTR_RE
                .captures(attrs)
                .and_then(lang_capture)
        })
}

fn lang_capture(caps: Captures<'_>) -> Option<String> {
    caps.name("lang")
        .map(|m| normalize_language(m.as_str()).to_string())
}

fn syntax_for_language(language: Option<&str>) -> &'static SyntaxReference {
    let syntax_set = &*SYNTAX_SET;

    language
        .map(normalize_language)
        .and_then(|lang| syntax_set.find_syntax_by_token(lang))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

fn normalize_language(language: &str) -> &str {
    match language {
        "rs"   => "rust",
        "clj"  |  "cljs" | "cljc" => "clojure",
        "js"   => "javascript",
        "ts"   => "typescript",
        "sh"   |  "shell" => "bash",
        "html" => "html",
        "css"  => "css",
        other  => other,
    }
}
