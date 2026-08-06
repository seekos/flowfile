use crate::models::{FileItem, FileKind};
use anyhow::{Context as _, Result, bail};
use gpui::{Context, RenderImage};
use image::{DynamicImage, GenericImage, ImageBuffer, Rgba, imageops::FilterType};
use lru::LruCache;
use rayon::{ThreadPool, ThreadPoolBuilder};
use smallvec::smallvec;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const THUMBNAIL_EDGE: u32 = 256;
const MEMORY_CACHE_LIMIT: usize = 100 * 1024 * 1024;
const MEMORY_ENTRY_LIMIT: usize = 4096;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ThumbnailKey(String);

impl ThumbnailKey {
    pub fn for_item(item: &FileItem) -> Self {
        let identity = format!("{}\0{}", item.path.to_string_lossy(), item.modified_unix);
        Self(format!("{:x}", md5::compute(identity.as_bytes())))
    }
}

struct CachedThumbnail {
    image: Arc<RenderImage>,
    bytes: usize,
}

struct PendingThumbnail {
    cancelled: Arc<AtomicBool>,
}

/// Background thumbnail coordinator.
///
/// The first cache level stores decoded `RenderImage` surfaces and is bounded
/// by their decoded byte size. The second level stores deterministic PNG files
/// under the user's Library cache directory.
pub struct ThumbnailEngine {
    pool: Arc<ThreadPool>,
    cache_dir: PathBuf,
    memory: LruCache<ThumbnailKey, CachedThumbnail>,
    memory_bytes: usize,
    pending: HashMap<ThumbnailKey, PendingThumbnail>,
    visible_by_owner: HashMap<usize, HashSet<ThumbnailKey>>,
    failed: HashSet<ThumbnailKey>,
}

impl ThumbnailEngine {
    pub fn new() -> Result<Self> {
        let cache_dir = thumbnail_cache_directory();
        fs::create_dir_all(&cache_dir)
            .with_context(|| format!("无法创建缩略图缓存 {}", cache_dir.display()))?;
        let pool = ThreadPoolBuilder::new()
            .thread_name(|index| format!("flowfile-thumbnail-{index}"))
            .build()
            .context("无法创建缩略图线程池")?;

        Ok(Self {
            pool: Arc::new(pool),
            cache_dir,
            memory: LruCache::new(
                std::num::NonZeroUsize::new(MEMORY_ENTRY_LIMIT).expect("non-zero cache size"),
            ),
            memory_bytes: 0,
            pending: HashMap::new(),
            visible_by_owner: HashMap::new(),
            failed: HashSet::new(),
        })
    }

    pub fn key_for(item: &FileItem) -> Option<ThumbnailKey> {
        (!item.is_dir
            && item.size > 0
            && matches!(
                item.kind,
                FileKind::Image | FileKind::Audio | FileKind::Video
            ))
        .then(|| ThumbnailKey::for_item(item))
    }

    /// Records the exact viewport set for one pane and cancels work that is no
    /// longer visible in any pane.
    pub fn set_visible(
        &mut self,
        owner: usize,
        visible_items: &[FileItem],
        cx: &mut Context<Self>,
    ) {
        let visible = visible_items
            .iter()
            .filter_map(Self::key_for)
            .collect::<HashSet<_>>();
        self.visible_by_owner.insert(owner, visible);

        let globally_visible = self
            .visible_by_owner
            .values()
            .flat_map(|keys| keys.iter().cloned())
            .collect::<HashSet<_>>();
        for (key, request) in &self.pending {
            if !globally_visible.contains(key) {
                request.cancelled.store(true, Ordering::Release);
            }
        }

        for item in visible_items {
            self.request(item, cx);
        }
    }

    pub fn image_for(&mut self, item: &FileItem) -> Option<Arc<RenderImage>> {
        let key = Self::key_for(item)?;
        self.memory.get(&key).map(|entry| entry.image.clone())
    }

    fn request(&mut self, item: &FileItem, cx: &mut Context<Self>) {
        let Some(key) = Self::key_for(item) else {
            return;
        };
        if self.memory.contains(&key)
            || self.pending.contains_key(&key)
            || self.failed.contains(&key)
        {
            return;
        }

        let source = item.path.clone();
        let cache_path = self.cache_dir.join(format!("{}.png", key.0));
        let work_root = self.cache_dir.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.pending.insert(
            key.clone(),
            PendingThumbnail {
                cancelled: cancelled.clone(),
            },
        );

        let (sender, receiver) = async_channel::bounded(1);
        self.pool.spawn(move || {
            let result = generate_thumbnail(&source, &cache_path, &work_root, &key, &cancelled);
            let _ = sender.send_blocking((key, result));
        });

        cx.spawn(async move |this, cx| {
            let Ok((key, result)) = receiver.recv().await else {
                return;
            };
            let _ = this.update(cx, |engine, cx| {
                engine.pending.remove(&key);
                match result {
                    Ok(Some(image)) => engine.insert(key, image),
                    Ok(None) => {}
                    Err(error) => {
                        eprintln!("FlowFile: 缩略图生成失败：{error:#}");
                        engine.failed.insert(key);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn insert(&mut self, key: ThumbnailKey, image: Arc<RenderImage>) {
        let bytes = image
            .as_bytes(0)
            .map(|bytes| bytes.len())
            .unwrap_or((THUMBNAIL_EDGE * THUMBNAIL_EDGE * 4) as usize);
        if let Some(previous) = self.memory.put(key, CachedThumbnail { image, bytes }) {
            self.memory_bytes = self.memory_bytes.saturating_sub(previous.bytes);
        }
        self.memory_bytes = self.memory_bytes.saturating_add(bytes);

        while self.memory_bytes > MEMORY_CACHE_LIMIT {
            let Some((_, evicted)) = self.memory.pop_lru() else {
                break;
            };
            self.memory_bytes = self.memory_bytes.saturating_sub(evicted.bytes);
        }
    }
}

fn generate_thumbnail(
    source: &Path,
    cache_path: &Path,
    work_root: &Path,
    key: &ThumbnailKey,
    cancelled: &AtomicBool,
) -> Result<Option<Arc<RenderImage>>> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(None);
    }

    if !cache_path.is_file() {
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let generated = if matches!(
            extension.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "avif"
        ) {
            render_raster(source, cache_path).is_ok()
        } else {
            false
        };

        if !generated {
            render_with_quicklook(source, cache_path, work_root, key)?;
        }
    }

    if cancelled.load(Ordering::Acquire) {
        return Ok(None);
    }
    let image = image::open(cache_path)
        .with_context(|| format!("无法解码缓存缩略图 {}", cache_path.display()))?;
    Ok(Some(dynamic_image_to_render_image(image)))
}

fn render_raster(source: &Path, cache_path: &Path) -> Result<()> {
    let source_image =
        image::open(source).with_context(|| format!("无法解码图片 {}", source.display()))?;
    render_raster_from_image(source_image, cache_path)
}

fn render_raster_from_image(image: DynamicImage, cache_path: &Path) -> Result<()> {
    let thumbnail = image.resize(THUMBNAIL_EDGE, THUMBNAIL_EDGE, FilterType::Triangle);
    let mut canvas = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(
        THUMBNAIL_EDGE,
        THUMBNAIL_EDGE,
        Rgba([0, 0, 0, 0]),
    ));
    let left = (THUMBNAIL_EDGE.saturating_sub(thumbnail.width())) / 2;
    let top = (THUMBNAIL_EDGE.saturating_sub(thumbnail.height())) / 2;
    canvas
        .copy_from(&thumbnail, left, top)
        .context("无法合成缩略图")?;
    canvas
        .save_with_format(cache_path, image::ImageFormat::Png)
        .with_context(|| format!("无法写入缩略图 {}", cache_path.display()))
}

fn render_with_quicklook(
    source: &Path,
    cache_path: &Path,
    work_root: &Path,
    key: &ThumbnailKey,
) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("当前平台不支持 Quick Look 缩略图");
    }

    let work_dir = work_root.join(format!(".work-{}-{}", key.0, std::process::id()));
    fs::create_dir_all(&work_dir)?;
    let output = Command::new("/usr/bin/qlmanage")
        .args(["-t", "-s", "256", "-o"])
        .arg(&work_dir)
        .arg(source)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("无法启动 Quick Look：{}", source.display()))?;

    let generated = fs::read_dir(&work_dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("png"));
    let result = match (output.success(), generated) {
        (true, Some(generated)) => {
            let image = image::open(&generated)?;
            render_raster_from_image(image, cache_path)
        }
        _ => Err(anyhow::anyhow!(
            "Quick Look 未能为 {} 生成缩略图",
            source.display()
        )),
    };
    if work_dir.starts_with(work_root) {
        let _ = fs::remove_dir_all(&work_dir);
    }
    result
}

fn dynamic_image_to_render_image(image: DynamicImage) -> Arc<RenderImage> {
    let mut rgba = image.into_rgba8();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(smallvec![image::Frame::new(rgba)]))
}

fn thumbnail_cache_directory() -> PathBuf {
    crate::models::home_directory()
        .join("Library")
        .join("Caches")
        .join("FlowFile")
        .join("thumbnails")
}

#[cfg(test)]
mod tests {
    use super::{THUMBNAIL_EDGE, ThumbnailEngine, ThumbnailKey, render_raster};
    use crate::models::{FileItem, FileKind};
    use image::{ImageBuffer, Rgba};
    use std::path::PathBuf;

    fn item(modified_unix: i64) -> FileItem {
        FileItem {
            path: PathBuf::from("/tmp/photo.jpg"),
            name: "photo.jpg".to_string(),
            is_dir: false,
            extension: Some("jpg".to_string()),
            size: 10,
            modified_unix,
            modified: String::new(),
            is_hidden: false,
            kind: FileKind::Image,
        }
    }

    #[test]
    fn cache_key_changes_with_modification_time() {
        assert_ne!(
            ThumbnailKey::for_item(&item(1)),
            ThumbnailKey::for_item(&item(2))
        );
    }

    #[test]
    fn program_icons_are_not_replaced_by_generated_thumbnails() {
        let mut application = item(1);
        application.is_dir = true;
        application.extension = Some("app".to_string());
        application.kind = FileKind::Application;
        let mut executable = item(1);
        executable.extension = None;
        executable.kind = FileKind::Executable;

        assert!(ThumbnailEngine::key_for(&application).is_none());
        assert!(ThumbnailEngine::key_for(&executable).is_none());
    }

    #[test]
    fn empty_files_use_default_icons_instead_of_blank_thumbnails() {
        let mut text_file = item(1);
        text_file.path = PathBuf::from("/tmp/empty.txt");
        text_file.name = "empty.txt".to_string();
        text_file.extension = Some("txt".to_string());
        text_file.size = 0;
        text_file.kind = FileKind::Document;

        assert!(ThumbnailEngine::key_for(&text_file).is_none());
    }

    #[test]
    fn only_media_files_receive_thumbnail_keys() {
        let mut file = item(1);
        assert!(ThumbnailEngine::key_for(&file).is_some());

        for kind in [FileKind::Audio, FileKind::Video] {
            file.kind = kind;
            assert!(ThumbnailEngine::key_for(&file).is_some());
        }

        for kind in [FileKind::Document, FileKind::Archive, FileKind::Other] {
            file.kind = kind;
            assert!(ThumbnailEngine::key_for(&file).is_none());
        }
    }

    #[test]
    fn raster_cache_is_a_fixed_256_pixel_canvas() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("wide.png");
        let output = directory.path().join("thumbnail.png");
        ImageBuffer::from_pixel(800, 200, Rgba([20_u8, 40, 60, 255]))
            .save(&source)
            .expect("write source image");

        render_raster(&source, &output).expect("render thumbnail");

        assert_eq!(
            image::image_dimensions(output).expect("thumbnail dimensions"),
            (THUMBNAIL_EDGE, THUMBNAIL_EDGE)
        );
    }
}
