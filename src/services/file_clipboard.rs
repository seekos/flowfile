use anyhow::{Result, anyhow};
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use objc2::{
    ClassType,
    rc::Retained,
    runtime::{AnyClass, AnyObject, ProtocolObject},
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSPasteboard, NSPasteboardWriting};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSArray, NSString, NSURL};

/// Replaces the general macOS pasteboard with native file URLs so Finder and
/// other file managers can paste files copied in FlowFile.
#[cfg(target_os = "macos")]
pub fn write_file_paths(paths: &[PathBuf]) -> Result<isize> {
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    write_paths_to_pasteboard(&pasteboard, paths)
}

#[cfg(target_os = "macos")]
fn write_paths_to_pasteboard(pasteboard: &NSPasteboard, paths: &[PathBuf]) -> Result<isize> {
    if paths.is_empty() {
        return Err(anyhow!("没有可写入剪贴板的文件"));
    }

    let writers: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
        .iter()
        .map(|path| {
            let is_directory = path.is_dir();
            let path = NSString::from_str(&path.to_string_lossy());
            let url = unsafe { NSURL::fileURLWithPath_isDirectory(&path, is_directory) };
            ProtocolObject::from_retained(url)
        })
        .collect();
    let objects = NSArray::from_vec(writers);

    unsafe {
        pasteboard.clearContents();
        if !pasteboard.writeObjects(&objects) {
            return Err(anyhow!("无法将文件写入系统剪贴板"));
        }
        Ok(pasteboard.changeCount())
    }
}

#[cfg(not(target_os = "macos"))]
pub fn write_file_paths(_paths: &[PathBuf]) -> Result<isize> {
    Err(anyhow!("当前系统不支持原生文件剪贴板"))
}

/// Reads file URLs from the general pasteboard. Finder publishes copied files
/// this way, including multiple selections.
#[cfg(target_os = "macos")]
pub fn read_file_paths() -> Vec<PathBuf> {
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    read_paths_from_pasteboard(&pasteboard)
}

#[cfg(target_os = "macos")]
fn read_paths_from_pasteboard(pasteboard: &NSPasteboard) -> Vec<PathBuf> {
    // `readObjectsForClasses:` takes Objective-C Class objects in an NSArray.
    // NSURL is requested specifically, then non-file URLs are filtered out.
    let url_class = {
        let class: *const AnyClass = NSURL::class();
        let object = class.cast_mut().cast::<AnyObject>();
        unsafe { Retained::retain(object) }
    };
    let Some(url_class) = url_class else {
        return Vec::new();
    };
    let classes = NSArray::from_vec(vec![url_class]);
    let Some(objects) = (unsafe { pasteboard.readObjectsForClasses_options(&classes, None) })
    else {
        return Vec::new();
    };

    (0..objects.count())
        .filter_map(|index| {
            let object = unsafe { objects.objectAtIndex(index) };
            let object: *const AnyObject = &*object;
            let url = unsafe { &*object.cast::<NSURL>() };
            if !unsafe { url.isFileURL() } {
                return None;
            }
            let path_url = unsafe { url.filePathURL() }?;
            unsafe { path_url.path() }.map(|path| PathBuf::from(path.to_string()))
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
pub fn read_file_paths() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(target_os = "macos")]
pub fn change_count() -> Option<isize> {
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    Some(unsafe { pasteboard.changeCount() })
}

#[cfg(not(target_os = "macos"))]
pub fn change_count() -> Option<isize> {
    None
}

/// Clears the pasteboard only if it still contains the file operation that
/// FlowFile originally placed there. This avoids erasing a newer copy made in
/// Finder while a transfer was running.
#[cfg(target_os = "macos")]
pub fn clear_if_unchanged(expected_change_count: isize) -> bool {
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    unsafe {
        if pasteboard.changeCount() != expected_change_count {
            return false;
        }
        pasteboard.clearContents();
    }
    true
}

#[cfg(not(target_os = "macos"))]
pub fn clear_if_unchanged(_expected_change_count: isize) -> bool {
    false
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{read_paths_from_pasteboard, write_paths_to_pasteboard};
    use objc2_app_kit::NSPasteboard;
    use std::fs;

    #[test]
    fn native_pasteboard_round_trips_multiple_files_and_directories() {
        let temp = tempfile::tempdir().expect("create temporary directory");
        let file = temp.path().join("sample.txt");
        let directory = temp.path().join("folder");
        fs::write(&file, "sample").expect("create temporary file");
        fs::create_dir(&directory).expect("create temporary subdirectory");
        let expected = vec![file, directory];
        let pasteboard = unsafe { NSPasteboard::pasteboardWithUniqueName() };

        write_paths_to_pasteboard(&pasteboard, &expected).expect("write native file URLs");

        assert_eq!(read_paths_from_pasteboard(&pasteboard), expected);
    }
}
