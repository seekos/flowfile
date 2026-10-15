use super::FileEngine;
use crate::models::is_image_extension;
use anyhow::{Context as _, Result, bail};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::UNIX_EPOCH,
};
use tokio::{fs::File, io::AsyncReadExt, runtime::Handle};

pub const TEXT_PREVIEW_LIMIT: usize = 100 * 1024;
const NATIVE_PREVIEW_MAX_DIMENSION: u32 = 8_192;
const NATIVE_PREVIEW_DATA_LIMIT: usize = 512 * 1024 * 1024;
const NATIVE_PREVIEW_CACHE_LIMIT: u64 = 512 * 1024 * 1024;
static NATIVE_PREVIEW_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewKind {
    Image,
    Text,
    Native,
}

#[derive(Clone)]
pub struct QuickLookService {
    runtime: Handle,
    native_child: Arc<Mutex<Option<Child>>>,
    native_generation: Arc<AtomicU64>,
    image_cache_dir: PathBuf,
}

impl QuickLookService {
    pub fn new(engine: &FileEngine) -> Self {
        let image_cache_dir = crate::models::home_directory()
            .join("Library")
            .join("Caches")
            .join("FlowFile")
            .join("image-previews");
        if let Err(error) = fs::create_dir_all(&image_cache_dir) {
            eprintln!(
                "FlowFile: 无法创建图片预览缓存 {}：{error}",
                image_cache_dir.display()
            );
        } else {
            prune_image_cache(&image_cache_dir, NATIVE_PREVIEW_CACHE_LIMIT);
        }
        Self {
            runtime: engine.runtime_handle(),
            native_child: Arc::new(Mutex::new(None)),
            native_generation: Arc::new(AtomicU64::new(0)),
            image_cache_dir,
        }
    }

    pub fn classify(path: &std::path::Path) -> PreviewKind {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if is_image_extension(&extension) {
            PreviewKind::Image
        } else if is_text_extension(&extension) {
            PreviewKind::Text
        } else {
            PreviewKind::Native
        }
    }

    /// Returns true when GPUI's in-process image loader can decode the source
    /// without a native conversion step. GPUI and this application share the
    /// same `image` feature set, so animated GIF/WebP files remain animated.
    /// SVG is intentionally delegated to Quick Look because GPUI's static
    /// renderer does not resolve browser CSS custom properties such as
    /// `var(--color)`, which can otherwise turn a valid SVG into a black image.
    pub fn can_render_image_directly(path: &Path) -> bool {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        matches!(
            extension.as_str(),
            "png"
                | "apng"
                | "jpg"
                | "jpeg"
                | "jpe"
                | "jfif"
                | "gif"
                | "webp"
                | "bmp"
                | "dib"
                | "tif"
                | "tiff"
                | "avif"
                | "ico"
                | "cur"
                | "pnm"
                | "pbm"
                | "pgm"
                | "ppm"
                | "pam"
                | "qoi"
                | "tga"
                | "hdr"
                | "dds"
                | "exr"
                | "ff"
        )
    }

    /// Converts formats such as HEIC, camera RAW, JPEG 2000, PSD, and ICNS
    /// through the native macOS image stack, then returns a cacheable PNG.
    pub async fn prepare_image(&self, path: PathBuf) -> Result<PathBuf> {
        if Self::can_render_image_directly(&path) {
            return Ok(path);
        }
        let cache_dir = self.image_cache_dir.clone();
        self.runtime
            .spawn_blocking(move || prepare_native_image(&path, &cache_dir))
            .await
            .context("原生图片预览任务异常终止")?
    }

    pub async fn read_text(&self, path: PathBuf) -> Result<(String, bool)> {
        self.runtime
            .spawn(async move {
                let file = File::open(&path)
                    .await
                    .with_context(|| format!("无法打开 {}", path.display()))?;
                let mut buffer = Vec::with_capacity(TEXT_PREVIEW_LIMIT);
                file.take(TEXT_PREVIEW_LIMIT as u64 + 1)
                    .read_to_end(&mut buffer)
                    .await?;
                let truncated = buffer.len() > TEXT_PREVIEW_LIMIT;
                buffer.truncate(TEXT_PREVIEW_LIMIT);
                Ok((String::from_utf8_lossy(&buffer).into_owned(), truncated))
            })
            .await
            .context("文本预览任务异常终止")?
    }

    pub fn open_native(&self, path: PathBuf) {
        self.close_native();
        let native_child = self.native_child.clone();
        let native_generation = self.native_generation.clone();
        let generation = native_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.runtime.spawn_blocking(move || {
            let result = std::process::Command::new("/usr/bin/qlmanage")
                .arg("-p")
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            match result {
                Ok(mut child) => {
                    if native_generation.load(Ordering::Acquire) == generation {
                        if let Ok(mut slot) = native_child.lock() {
                            *slot = Some(child);
                        }
                    } else {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
                Err(error) => eprintln!("FlowFile: 无法启动 macOS Quick Look：{error}"),
            }
        });
    }

    pub fn close_native(&self) {
        self.native_generation.fetch_add(1, Ordering::AcqRel);
        let child = self
            .native_child
            .lock()
            .ok()
            .and_then(|mut child| child.take());
        if let Some(mut child) = child {
            self.runtime.spawn_blocking(move || {
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

fn prepare_native_image(source: &Path, cache_dir: &Path) -> Result<PathBuf> {
    let metadata =
        fs::metadata(source).with_context(|| format!("无法读取图片元数据 {}", source.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let identity = format!(
        "{}\0{}\0{}",
        source.to_string_lossy(),
        metadata.len(),
        modified
    );
    let destination = cache_dir.join(format!("{:x}.png", md5::compute(identity.as_bytes())));
    if destination.is_file() {
        return Ok(destination);
    }
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("无法创建图片预览缓存 {}", cache_dir.display()))?;
    let temporary = cache_dir.join(format!(
        ".{:x}-{}-{}.tmp.png",
        md5::compute(identity.as_bytes()),
        std::process::id(),
        NATIVE_PREVIEW_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = render_native_image_to_png(source, &temporary).and_then(|_| {
        fs::rename(&temporary, &destination)
            .with_context(|| format!("无法提交图片预览缓存 {}", destination.display()))
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(destination)
}

fn prune_image_cache(cache_dir: &Path, limit: u64) {
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut total = 0_u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if entry.file_name().to_string_lossy().starts_with('.') {
            let _ = fs::remove_file(path);
            continue;
        }
        total = total.saturating_add(metadata.len());
        files.push((metadata.modified().ok(), metadata.len(), path));
    }
    if total <= limit {
        return;
    }
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, bytes, path) in files {
        if total <= limit {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(bytes);
        }
    }
}

#[cfg(target_os = "macos")]
fn render_native_image_to_png(source: &Path, destination: &Path) -> Result<()> {
    if source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return render_svg_with_quicklook(source, destination);
    }

    use image::imageops::FilterType;
    use objc2::{ClassType, rc::autoreleasepool};
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSString;

    let tiff = autoreleasepool(|_| -> Result<Vec<u8>> {
        let path = NSString::from_str(&source.to_string_lossy());
        // SAFETY: The NSString remains alive for the duration of the AppKit
        // call, and the retained NSImage/NSData values are kept inside this
        // autorelease pool while their bytes are copied into owned Rust data.
        let image = unsafe { NSImage::initWithContentsOfFile(NSImage::alloc(), &path) }
            .with_context(|| format!("macOS 无法解码图片 {}", source.display()))?;
        // SAFETY: `image` is a live retained NSImage instance.
        let data = unsafe { image.TIFFRepresentation() }
            .with_context(|| format!("macOS 无法转换图片 {}", source.display()))?;
        if data.len() > NATIVE_PREVIEW_DATA_LIMIT {
            bail!("图片解码数据超过 512 MB 安全限制");
        }
        Ok(data.bytes().to_vec())
    })?;

    let mut decoded = image::load_from_memory_with_format(&tiff, image::ImageFormat::Tiff)
        .with_context(|| format!("无法读取 macOS 图片数据 {}", source.display()))?;
    if decoded.width() > NATIVE_PREVIEW_MAX_DIMENSION
        || decoded.height() > NATIVE_PREVIEW_MAX_DIMENSION
    {
        decoded = decoded.resize(
            NATIVE_PREVIEW_MAX_DIMENSION,
            NATIVE_PREVIEW_MAX_DIMENSION,
            FilterType::Triangle,
        );
    }
    decoded
        .save_with_format(destination, image::ImageFormat::Png)
        .with_context(|| format!("无法写入图片预览 {}", destination.display()))
}

#[cfg(target_os = "macos")]
fn render_svg_with_quicklook(source: &Path, destination: &Path) -> Result<()> {
    let (source_width, source_height) = native_image_dimensions(source)
        .with_context(|| format!("无法读取 SVG 尺寸 {}", source.display()))?;
    let render_edge = source_width
        .max(source_height)
        .clamp(1, NATIVE_PREVIEW_MAX_DIMENSION);
    let work_dir = destination.with_extension(format!(
        "quicklook-{}-{}",
        std::process::id(),
        NATIVE_PREVIEW_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("无法创建 SVG 预览临时目录 {}", work_dir.display()))?;

    let result = (|| -> Result<()> {
        let status = std::process::Command::new("/usr/bin/qlmanage")
            .args(["-t", "-s"])
            .arg(render_edge.to_string())
            .arg("-o")
            .arg(&work_dir)
            .arg(source)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| format!("无法启动 SVG Quick Look 渲染：{}", source.display()))?;
        if !status.success() {
            bail!("Quick Look 未能渲染 SVG {}", source.display());
        }

        let generated = fs::read_dir(&work_dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .find(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            })
            .with_context(|| format!("Quick Look 未生成 SVG 预览 {}", source.display()))?;
        let rendered = image::open(&generated)
            .with_context(|| format!("无法读取 SVG Quick Look 预览 {}", generated.display()))?;
        let (crop_width, crop_height) = aspect_fit_size(
            source_width,
            source_height,
            rendered.width(),
            rendered.height(),
        );
        rendered
            .crop_imm(0, 0, crop_width, crop_height)
            .save_with_format(destination, image::ImageFormat::Png)
            .with_context(|| format!("无法写入 SVG 预览 {}", destination.display()))
    })();

    let _ = fs::remove_dir_all(&work_dir);
    result
}

fn aspect_fit_size(
    source_width: u32,
    source_height: u32,
    bounds_width: u32,
    bounds_height: u32,
) -> (u32, u32) {
    let source_width = source_width.max(1) as u64;
    let source_height = source_height.max(1) as u64;
    let bounds_width = bounds_width.max(1) as u64;
    let bounds_height = bounds_height.max(1) as u64;
    if source_width * bounds_height >= source_height * bounds_width {
        let height = (bounds_width * source_height / source_width).max(1);
        (bounds_width as u32, height.min(bounds_height) as u32)
    } else {
        let width = (bounds_height * source_width / source_height).max(1);
        (width.min(bounds_width) as u32, bounds_height as u32)
    }
}

#[cfg(not(target_os = "macos"))]
fn render_native_image_to_png(_source: &Path, _destination: &Path) -> Result<()> {
    bail!("当前平台不支持此图片格式的原生解码")
}

#[cfg(target_os = "macos")]
pub(crate) fn native_image_dimensions(path: &Path) -> Option<(u32, u32)> {
    use objc2::{ClassType, rc::autoreleasepool};
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSString;

    autoreleasepool(|_| {
        let path = NSString::from_str(&path.to_string_lossy());
        // SAFETY: The NSString remains alive during initialization and the
        // returned retained image is only used inside this autorelease pool.
        let image = unsafe { NSImage::initWithContentsOfFile(NSImage::alloc(), &path) }?;
        // SAFETY: `image` is a live retained NSImage instance.
        let size = unsafe { image.size() };
        let width = size.width.round();
        let height = size.height.round();
        (width > 0.0 && height > 0.0).then_some((width as u32, height as u32))
    })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn native_image_dimensions(_path: &Path) -> Option<(u32, u32)> {
    None
}

pub fn is_text_extension(extension: &str) -> bool {
    matches!(
        extension,
        "txt"
            | "md"
            | "log"
            | "rs"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "py"
            | "go"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "css"
            | "html"
            | "xml"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "sh"
            | "zsh"
            | "swift"
    )
}

#[cfg(test)]
mod tests {
    use super::{PreviewKind, QuickLookService, aspect_fit_size};
    use std::path::Path;

    #[test]
    fn classifies_extended_image_formats_for_the_builtin_viewer() {
        for path in [
            "photo.avif",
            "photo.heic",
            "photo.jxl",
            "photo.qoi",
            "photo.cr3",
            "photo.psd",
        ] {
            assert_eq!(
                QuickLookService::classify(Path::new(path)),
                PreviewKind::Image
            );
        }
        assert_eq!(
            QuickLookService::classify(Path::new("document.pdf")),
            PreviewKind::Native
        );
    }

    #[test]
    fn native_only_formats_are_not_sent_to_the_rust_decoder() {
        assert!(QuickLookService::can_render_image_directly(Path::new(
            "photo.tiff"
        )));
        assert!(QuickLookService::can_render_image_directly(Path::new(
            "animation.gif"
        )));
        assert!(!QuickLookService::can_render_image_directly(Path::new(
            "diagram.svg"
        )));
        assert!(!QuickLookService::can_render_image_directly(Path::new(
            "photo.heic"
        )));
        assert!(!QuickLookService::can_render_image_directly(Path::new(
            "photo.cr3"
        )));
    }

    #[test]
    fn svg_quicklook_canvas_is_cropped_to_the_source_aspect_ratio() {
        assert_eq!(aspect_fit_size(960, 640, 960, 960), (960, 640));
        assert_eq!(aspect_fit_size(640, 960, 960, 960), (640, 960));
        assert_eq!(aspect_fit_size(1920, 1080, 8192, 8192), (8192, 4608));
    }
}
