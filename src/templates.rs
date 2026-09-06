use std::{fs, path::Path};

use anyhow::{Context, Result};
use minijinja::{AutoEscape, Environment};
use walkdir::WalkDir;

pub fn load_templates(templates_dir: &Path) -> Result<Environment<'static>> {
    let mut env = Environment::new();

    env.set_auto_escape_callback(|name| {
        if name.ends_with(".html") {
            AutoEscape::Html
        } else {
            AutoEscape::None
        }
    });

    for entry in WalkDir::new(templates_dir) {
        let entry = entry.with_context(|| {
            format!(
                "Failed while walking templates directory: {}",
                templates_dir.display()
            )
        })?;

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        if path.extension().and_then(|value| value.to_str()) != Some("html") {
            continue;
        }

        let name = path
            .strip_prefix(templates_dir)
            .with_context(|| format!("Failed to make template path relative: {}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");

        let source = fs::read_to_string(path)
            .with_context(|| format!("Failed to read template: {}", path.display()))?;

        env.add_template_owned(name, source)
            .with_context(|| format!("Failed to register template: {}", path.display()))?;
    }

    Ok(env)
}
