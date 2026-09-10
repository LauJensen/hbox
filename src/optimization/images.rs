use crate::dev_print;
use reqwest::Url;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use image::{
    imageops::FilterType,
    DynamicImage,
    GenericImageView,
    ImageFormat,
};
use kuchiki::traits::*;
use walkdir::WalkDir;

const WEBP_QUALITY: f32 = 82.0;

pub fn find_dist_html_files(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.is_dir() {
        bail!(
            "Dist site directory does not exist: {}",
            path.display()
        );
    }

    let mut html_files = Vec::new();

    for entry in WalkDir::new(path).sort_by_file_name() {
        let entry = entry.with_context(|| {
            format!("Failed while walking {}", path.display())
        })?;

        if !entry.file_type().is_file() {
            continue;
        }

        let file_path = entry.path();

        if file_path.extension().is_some_and(|ext| ext == "html") {
            html_files.push(file_path.to_path_buf());
        }
    }

    Ok(html_files)
}

pub fn optimize_html_file(
    dist_site_dir: &Path,
    html_path: &Path,
) -> Result<()> {
    dev_print!("Optimizing {}", html_path.display());

    let html = fs::read_to_string(html_path).with_context(|| {
        format!(
            "Failed to read HTML file: {}",
            html_path.display()
        )
    })?;

    let document = kuchiki::parse_html().one(html);

    optimize_img_tags(dist_site_dir, html_path, &document).with_context(|| {
        format!(
            "Failed to optimize image tags in: {}",
            html_path.display()
        )
    })?;

    let mut output = Vec::new();

    document.serialize(&mut output).with_context(|| {
        format!(
            "Failed to serialize optimized HTML file: {}",
            html_path.display()
        )
    })?;

    fs::write(html_path, output).with_context(|| {
        format!(
            "Failed to write optimized HTML file: {}",
            html_path.display()
        )
    })?;

    Ok(())
}

fn optimize_img_tags(
    dist_site_dir: &Path,
    html_path: &Path,
    document: &kuchiki::NodeRef,
) -> Result<()> {
    let img_tags = document
        .select("img")
        .map_err(|_| anyhow::anyhow!("Failed to select img tags"))?;

    for img in img_tags {
        optimize_img_tag(dist_site_dir, html_path, img.as_node())?;
    }

    Ok(())
}

fn optimize_img_tag(
    dist_site_dir: &Path,
    html_path: &Path,
    img: &kuchiki::NodeRef,
) -> Result<()> {
    let element = img
        .as_element()
        .context("img selector returned a non-element node")?;

    let (src, existing_srcset) = {
        let attrs = element.attributes.borrow();

        let Some(src) = attrs.get("src") else {
            return Ok(());
        };

        let existing_srcset = attrs
            .get("srcset")
            .map(|value| value.trim().to_owned());

        (src.to_owned(), existing_srcset)
    };

    if existing_srcset.is_some_and(|srcset| !srcset.is_empty()) {
        return Ok(());
    }

    let Some(image_path) = image_path_for_src(
        dist_site_dir,
        html_path,
        &src,
    )? else {
        return Ok(());
    };

    if !is_optimizable_image(&image_path) {
        return Ok(());
    }

    if !image_path.is_file() {
        bail!(
            "Image referenced by {} does not exist on disk: {} -> {}",
            html_path.display(),
            src,
            image_path.display()
        );
    }

    let (image, source_format) = decode_image(&image_path)?;

    let (original_width, original_height) = image.dimensions();
    let webp_path = webp_path_for_image(&image_path)?;
    let webp_src = webp_src_for_image(&src)?;

    if source_format != ImageFormat::WebP {
        write_webp(&image, &webp_path)?;
    } else if webp_path != image_path && !webp_path.is_file() {
        fs::copy(&image_path, &webp_path).with_context(|| {
            format!(
                "Failed to copy WebP image {} to {}",
                image_path.display(),
                webp_path.display()
            )
        })?;
    }

    let mut srcset_entries = vec![
        format!("{webp_src} {original_width}w")
    ];

    for width in responsive_widths(original_width) {
        let height = scaled_height(
            original_width,
            original_height,
            width,
        );

        let variant_path =
            variant_path_for_width(&image_path, width)?;

        if !variant_path.is_file() {
            let resized = image.resize_exact(
                width,
                height,
                FilterType::Lanczos3,
            );

            write_webp(&resized, &variant_path)?;
        }

        let variant_src = variant_src_for_width(&src, width)?;
        srcset_entries.push(format!("{variant_src} {width}w"));
    }

    let mut attrs = element.attributes.borrow_mut();

    attrs.insert("src", webp_src);
    attrs.insert("srcset", srcset_entries.join(", "));
    // TODO: Rules should determine laziness.
    // attrs.insert("loading", "lazy".to_owned());
    attrs.insert("decoding", "async".to_owned());

    Ok(())
}

fn decode_image(image_path: &Path) -> Result<(DynamicImage, ImageFormat)> {
    let bytes = fs::read(image_path).with_context(|| {
        format!("Failed to read image: {}", image_path.display())
    })?;

    let format = image::guess_format(&bytes).with_context(|| {
        format!(
            "Failed to detect actual image format: {}",
            image_path.display()
        )
    })?;

    let image = if format == ImageFormat::WebP {
        let decoded = webp::Decoder::new(&bytes)
            .decode()
            .with_context(|| {
                format!(
                    "libwebp could not decode image: {}",
                    image_path.display()
                )
            })?;

        decoded.to_image()
    } else {
        image::load_from_memory_with_format(&bytes, format).with_context(|| {
            format!(
                "Failed to decode {format:?} image: {}",
                image_path.display()
            )
        })?
    };

    Ok((image, format))
}

fn write_webp(image: &DynamicImage, output_path: &Path) -> Result<()> {
    let rgba = image.to_rgba8();

    let encoder = webp::Encoder::from_rgba(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    );

    let encoded = encoder.encode(WEBP_QUALITY);

    fs::write(output_path, &*encoded).with_context(|| {
        format!(
            "Failed to write WebP image: {}",
            output_path.display()
        )
    })
}

fn is_non_local_reference(source: &str) -> bool {
    let source = source.trim();

    source.starts_with('#')
        || source.starts_with("//")
        || Url::parse(source).is_ok()
}

fn image_path_for_src(
    dist_site_dir: &Path,
    html_path: &Path,
    src: &str,
) -> Result<Option<PathBuf>> {
    let src = src.trim();

    if src.is_empty() || is_non_local_reference(src) {
        return Ok(None);
    }

    let clean_src = clean_src_for_srcset(src);

    if clean_src.is_empty() {
        return Ok(None);
    }

    let base = if clean_src.starts_with('/') {
        dist_site_dir
    } else {
        html_path.parent().with_context(|| {
            format!(
                "HTML file has no parent directory: {}",
                html_path.display()
            )
        })?
    };

    let image_path = crate::utils::resolve_within(
        dist_site_dir,
        base,
        &clean_src,
    )?;

    Ok(Some(image_path))
}

fn is_optimizable_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp"
            )
        })
}

fn is_webp(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("webp")
        })
}

fn responsive_widths(original_width: u32) -> Vec<u32> {
    const MIN_WIDTH: u32 = 100;

    let mut widths = Vec::new();
    let mut width = ((original_width as f32) * 0.9).round() as u32;

    while width >= MIN_WIDTH {
        widths.push(width);
        width = ((width as f32) * 0.8).round() as u32;
    }

    widths
}

fn scaled_height(
    original_width: u32,
    original_height: u32,
    target_width: u32,
) -> u32 {
    ((original_height as f64)
        * (target_width as f64 / original_width as f64))
        .round()
        .max(1.0) as u32
}

fn webp_path_for_image(image_path: &Path) -> Result<PathBuf> {
    if is_webp(image_path) {
        return Ok(image_path.to_path_buf());
    }

    let filename = image_path
        .file_name()
        .and_then(|filename| filename.to_str())
        .with_context(|| {
            format!(
                "Image path has invalid filename: {}",
                image_path.display()
            )
        })?;

    Ok(image_path.with_file_name(format!("{filename}.webp")))
}

fn webp_src_for_image(src: &str) -> Result<String> {
    let clean_src = clean_src_for_srcset(src);
    let path = Path::new(&clean_src);

    if is_webp(path) {
        return Ok(clean_src);
    }

    let filename = path
        .file_name()
        .and_then(|filename| filename.to_str())
        .with_context(|| {
            format!("Image src has invalid filename: {src}")
        })?;

    Ok(replace_src_filename(
        path,
        format!("{filename}.webp"),
    ))
}

fn variant_path_for_width(
    image_path: &Path,
    width: u32,
) -> Result<PathBuf> {
    let filename = image_path
        .file_name()
        .and_then(|filename| filename.to_str())
        .with_context(|| {
            format!(
                "Image path has invalid filename: {}",
                image_path.display()
            )
        })?;

    Ok(image_path.with_file_name(
        format!("{filename}-{width}w.webp")
    ))
}

fn variant_src_for_width(
    src: &str,
    width: u32,
) -> Result<String> {
    let clean_src = clean_src_for_srcset(src);
    let path = Path::new(&clean_src);

    let filename = path
        .file_name()
        .and_then(|filename| filename.to_str())
        .with_context(|| {
            format!("Image src has invalid filename: {src}")
        })?;

    Ok(replace_src_filename(
        path,
        format!("{filename}-{width}w.webp"),
    ))
}

fn replace_src_filename(path: &Path, filename: String) -> String {
    match path.parent() {
        Some(parent) if parent != Path::new("") => {
            parent.join(filename).to_string_lossy().into_owned()
        }
        _ => filename,
    }
}

fn clean_src_for_srcset(src: &str) -> String {
    src.split('#')
        .next()
        .unwrap_or(src)
        .split('?')
        .next()
        .unwrap_or(src)
        .to_owned()
}
