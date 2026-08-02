use super::{FileEngine, quick_look::is_text_extension};
use exif::{In, Reader, Tag};
use gpui::Context;
use std::{
    fs::{self, File},
    io::{BufReader, Read},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use tokio::runtime::Handle;

const INSPECTION_READ_LIMIT: usize = 100 * 1024;

#[derive(Clone, Debug)]
pub struct FileInspectorMetadata {
    pub dimensions: Option<(u32, u32)>,
    pub line_count: Option<usize>,
    pub word_count: Option<usize>,
    pub permissions: String,
    pub exif: Vec<(String, String)>,
}

pub struct FileInspector {
    runtime: Handle,
    pub current_path: Option<PathBuf>,
    pub metadata: Option<FileInspectorMetadata>,
    pub is_loading: bool,
    pub error: Option<String>,
    generation: u64,
}

impl FileInspector {
    pub fn new(engine: &FileEngine) -> Self {
        Self {
            runtime: engine.runtime_handle(),
            current_path: None,
            metadata: None,
            is_loading: false,
            error: None,
            generation: 0,
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.current_path.take().is_some() {
            self.generation += 1;
            self.metadata = None;
            self.error = None;
            self.is_loading = false;
            cx.notify();
        }
    }

    pub fn request(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.current_path.as_ref() == Some(&path) {
            return;
        }
        self.current_path = Some(path.clone());
        self.metadata = None;
        self.error = None;
        self.is_loading = true;
        self.generation += 1;
        let generation = self.generation;
        let task = self
            .runtime
            .spawn_blocking(move || inspect_path(path.as_path()));

        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |inspector, cx| {
                if inspector.generation != generation {
                    return;
                }
                inspector.is_loading = false;
                match result {
                    Ok(Ok(metadata)) => inspector.metadata = Some(metadata),
                    Ok(Err(error)) => inspector.error = Some(error.to_string()),
                    Err(error) => inspector.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn inspect_path(path: &Path) -> anyhow::Result<FileInspectorMetadata> {
    let metadata = fs::symlink_metadata(path)?;
    let permissions = format_permissions(metadata.permissions().mode());
    let dimensions = if metadata.is_file() {
        image::image_dimensions(path).ok()
    } else {
        None
    };
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (line_count, word_count) = if metadata.is_file() && is_text_extension(&extension) {
        let mut bytes = Vec::with_capacity(INSPECTION_READ_LIMIT);
        File::open(path)?
            .take(INSPECTION_READ_LIMIT as u64)
            .read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        (
            Some(text.lines().count()),
            Some(text.split_whitespace().count()),
        )
    } else {
        (None, None)
    };
    let exif = read_exif(path);

    Ok(FileInspectorMetadata {
        dimensions,
        line_count,
        word_count,
        permissions,
        exif,
    })
}

fn read_exif(path: &Path) -> Vec<(String, String)> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let Ok(exif) = Reader::new().read_from_container(&mut reader) else {
        return Vec::new();
    };
    [
        (Tag::Make, "相机"),
        (Tag::Model, "型号"),
        (Tag::DateTimeOriginal, "拍摄时间"),
        (Tag::ExposureTime, "曝光"),
        (Tag::FNumber, "光圈"),
        (Tag::PhotographicSensitivity, "ISO"),
    ]
    .into_iter()
    .filter_map(|(tag, label)| {
        exif.get_field(tag, In::PRIMARY).map(|field| {
            (
                label.to_string(),
                field.display_value().with_unit(&exif).to_string(),
            )
        })
    })
    .collect()
}

fn format_permissions(mode: u32) -> String {
    let file_type = if mode & 0o170000 == 0o040000 {
        'd'
    } else if mode & 0o170000 == 0o120000 {
        'l'
    } else {
        '-'
    };
    let mut value = String::with_capacity(10);
    value.push(file_type);
    for shift in [6, 3, 0] {
        value.push(if mode & (0o4 << shift) != 0 { 'r' } else { '-' });
        value.push(if mode & (0o2 << shift) != 0 { 'w' } else { '-' });
        value.push(if mode & (0o1 << shift) != 0 { 'x' } else { '-' });
    }
    value
}

#[cfg(test)]
mod tests {
    use super::format_permissions;

    #[test]
    fn formats_posix_permissions() {
        assert_eq!(format_permissions(0o100644), "-rw-r--r--");
        assert_eq!(format_permissions(0o040755), "drwxr-xr-x");
    }
}
