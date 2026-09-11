use crate::dev_print;
use reqwest::Url;

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use image::{
    imageops::FilterType,
    DynamicImage,
    GenericImageView,
    ImageFormat,
};
use indicatif::{ProgressBar, ProgressStyle};
use kuchiki::traits::*;
use rayon::prelude::*;
use walkdir::WalkDir;

const PROGRESS_TICK_RATE: Duration = Duration::from_millis(100);
const PROGRESS_TEMPLATE: &str =
    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}";

#[derive(Debug, Clone)]
pub struct ImageResult {
    original_width: u32,
    variant_widths: Vec<u32>,
}

pub type ResultsByPath = HashMap<PathBuf, ImageResult>;

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

/// Finds every unique local image referenced by the supplied HTML files and
/// optimizes those images in parallel.
pub fn build_results_by_path(
    dist_site_dir: &Path,
    html_paths: &[PathBuf],
    webp_quality: u8,
) -> Result<ResultsByPath> {
    let jobs = collect_image_jobs(dist_site_dir, html_paths)?;
    let progress = image_progress(jobs.len())?;

    if jobs.is_empty() {
        progress.finish_with_message("No images to optimize");
        return Ok(HashMap::new());
    }

    let results = jobs
        .par_iter()
        .map(|image_path| {
            let result = optimize_image(image_path, webp_quality);

            if result.is_ok() {
                progress.inc(1);
            }

            result
        })
        .collect::<Result<Vec<_>>>();

    match results {
        Ok(results) => {
            progress.finish_with_message(format!(
                "Optimized {} images",
                results.len()
            ));

            Ok(results.into_iter().collect())
        }
        Err(error) => {
            progress.abandon_with_message("Image optimization failed");
            Err(error)
        }
    }
}

fn image_progress(total: usize) -> Result<ProgressBar> {
    let total = u64::try_from(total)
        .context("Deduplicated image count does not fit in u64")?;

    let progress = ProgressBar::new(total);
    progress.set_style(
        ProgressStyle::with_template(PROGRESS_TEMPLATE)?
            .progress_chars("#>-"),
    );
    progress.set_message("Optimizing images");
    progress.enable_steady_tick(PROGRESS_TICK_RATE);

    Ok(progress)
}

/// Rewrites one HTML file using image artifacts that have already been built.
pub fn optimize_html_file(
    dist_site_dir: &Path,
    html_path: &Path,
    results_by_path: &ResultsByPath,
) -> Result<()> {
    dev_print!("Optimizing {}", html_path.display());

    let html = fs::read_to_string(html_path).with_context(|| {
        format!(
            "Failed to read HTML file: {}",
            html_path.display()
        )
    })?;

    let document = kuchiki::parse_html().one(html);

    rewrite_img_tags(
        dist_site_dir,
        html_path,
        &document,
        results_by_path,
    )
    .with_context(|| {
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

fn collect_image_jobs(
    dist_site_dir: &Path,
    html_paths: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let mut image_paths = BTreeSet::new();

    for html_path in html_paths {
        let html = fs::read_to_string(html_path).with_context(|| {
            format!(
                "Failed to read HTML file: {}",
                html_path.display()
            )
        })?;

        let document = kuchiki::parse_html().one(html);
        let img_tags = document
            .select("img")
            .map_err(|_| anyhow::anyhow!("Failed to select img tags"))?;

        for img in img_tags {
            let element = img
                .as_node()
                .as_element()
                .context("img selector returned a non-element node")?;

            let src = {
                let attrs = element.attributes.borrow();

                if attrs
                    .get("srcset")
                    .is_some_and(|srcset| !srcset.trim().is_empty())
                {
                    continue;
                }

                attrs.get("src").map(str::to_owned)
            };

            let Some(src) = src else {
                continue;
            };

            let Some(image_path) = image_path_for_src(
                dist_site_dir,
                html_path,
                &src,
            )? else {
                continue;
            };

            if is_optimizable_image(&image_path) {
                image_paths.insert(image_path);
            }
        }
    }

    Ok(image_paths.into_iter().collect())
}

fn optimize_image(
    image_path: &Path,
    webp_quality: u8,
) -> Result<(PathBuf, ImageResult)> {
    if !image_path.is_file() {
        bail!(
            "Referenced image does not exist on disk: {}",
            image_path.display()
        );
    }

    let (image, source_format) = decode_image(image_path)?;
    let (original_width, original_height) = image.dimensions();
    let variant_widths = responsive_widths(original_width);
    let webp_path = webp_path_for_image(image_path)?;

    if source_format != ImageFormat::WebP {
        write_webp(&image, &webp_path, webp_quality)?;
    } else if webp_path != image_path {
        fs::copy(image_path, &webp_path).with_context(|| {
            format!(
                "Failed to copy WebP image {} to {}",
                image_path.display(),
                webp_path.display()
            )
        })?;
    }

    for &width in &variant_widths {
        let height = scaled_height(
            original_width,
            original_height,
            width,
        );

        let resized = image.resize_exact(
            width,
            height,
            FilterType::Lanczos3,
        );

        let variant_path =
            variant_path_for_width(image_path, width)?;

        write_webp(&resized, &variant_path, webp_quality)?;
    }

    Ok((
        image_path.to_path_buf(),
        ImageResult {
            original_width,
            variant_widths,
        },
    ))
}

fn rewrite_img_tags(
    dist_site_dir: &Path,
    html_path: &Path,
    document: &kuchiki::NodeRef,
    results_by_path: &ResultsByPath,
) -> Result<()> {
    let img_tags = document
        .select("img")
        .map_err(|_| anyhow::anyhow!("Failed to select img tags"))?;

    for img in img_tags {
        rewrite_img_tag(
            dist_site_dir,
            html_path,
            img.as_node(),
            results_by_path,
        )?;
    }

    Ok(())
}

fn rewrite_img_tag(
    dist_site_dir: &Path,
    html_path: &Path,
    img: &kuchiki::NodeRef,
    results_by_path: &ResultsByPath,
) -> Result<()> {
    let element = img
        .as_element()
        .context("img selector returned a non-element node")?;

    let src = {
        let attrs = element.attributes.borrow();

        if attrs
            .get("srcset")
            .is_some_and(|srcset| !srcset.trim().is_empty())
        {
            return Ok(());
        }

        let Some(src) = attrs.get("src") else {
            return Ok(());
        };

        src.to_owned()
    };

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

    let result = results_by_path.get(&image_path).with_context(|| {
        format!(
            "Missing optimization result for {}",
            image_path.display()
        )
    })?;

    let webp_src = webp_src_for_image(&src)?;
    let mut srcset_entries = vec![
        format!("{webp_src} {}w", result.original_width)
    ];

    for &width in &result.variant_widths {
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

fn write_webp(
    image: &DynamicImage,
    output_path: &Path,
    webp_quality: u8,
) -> Result<()> {
    let quality = f32::from(webp_quality);

    let encoded = match image {
        DynamicImage::ImageRgb8(rgb) => {
            webp::Encoder::from_rgb(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
            )
            .encode(quality)
        }

        DynamicImage::ImageRgba8(rgba) => {
            webp::Encoder::from_rgba(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
            )
            .encode(quality)
        }

        _ => {
            let rgba = image.to_rgba8();

            webp::Encoder::from_rgba(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
            )
            .encode(quality)
        }
    };

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
