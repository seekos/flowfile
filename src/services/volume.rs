use anyhow::{Context as _, Result};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeInfo {
    pub path: PathBuf,
    pub filesystem: String,
    pub read_only: bool,
}

impl VolumeInfo {
    pub fn is_ntfs(&self) -> bool {
        self.filesystem.eq_ignore_ascii_case("ntfs")
    }

    pub fn status_label(&self) -> Option<&'static str> {
        self.is_ntfs().then_some(if self.read_only {
            "NTFS · 只读"
        } else {
            "NTFS · 可写"
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MountInfo {
    source: PathBuf,
    path: PathBuf,
    filesystem: String,
    read_only: bool,
}

pub(crate) fn ntfs_auto_mount_available() -> bool {
    supports_fskit()
        && ntfs_mount_helper().is_some()
        && Command::new("/usr/sbin/pkgutil")
            .args(["--pkg-info", "io.macfuse.installer.components.core"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
}

pub(crate) fn auto_mount_ntfs(path: &Path) -> Result<bool> {
    let Some(helper) = ntfs_mount_helper() else {
        return Ok(false);
    };
    if !ntfs_auto_mount_available() {
        return Ok(false);
    }

    let Some(mount) = read_mounts()?.into_iter().find(|mount| mount.path == path) else {
        return Ok(false);
    };
    if !mount.filesystem.eq_ignore_ascii_case("ntfs") || !mount.read_only {
        return Ok(false);
    }
    let device = mount.source.to_string_lossy().to_string();
    if !device.starts_with("/dev/disk") || !disk_is_external(&device) {
        return Ok(false);
    }

    let output = Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "set devicePath to item 1 of argv",
            "-e",
            "set mountPoint to item 2 of argv",
            "-e",
            "set helperPath to item 3 of argv",
            "-e",
            "set mountCommand to \"/usr/sbin/diskutil unmount \" & quoted form of devicePath & \" && /bin/mkdir -p \" & quoted form of mountPoint & \" && \" & quoted form of helperPath & \" -o backend=fskit \" & quoted form of devicePath & \" \" & quoted form of mountPoint",
            "-e",
            "set recoveryCommand to \"/usr/sbin/diskutil mount \" & quoted form of devicePath & \" >/dev/null 2>&1\"",
            "-e",
            "do shell script mountCommand & \" || { \" & recoveryCommand & \"; exit 1; }\" with administrator privileges",
            "-e",
            "end run",
        ])
        .arg(&device)
        .arg(path)
        .arg(helper)
        .output()
        .context("无法请求 NTFS 挂载授权")?;

    if !output.status.success() {
        restore_native_mount(&device);
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("(-128)") {
            anyhow::bail!("已取消 NTFS 可写挂载授权，磁盘已恢复为只读");
        }
        anyhow::bail!("NTFS 可写挂载失败，磁盘已恢复为只读：{}", error.trim());
    }

    for _ in 0..40 {
        if read_mounts().is_ok_and(|mounts| {
            mounts
                .iter()
                .any(|mount| mount.path == path && !mount.read_only)
        }) {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(250));
    }

    restore_native_mount(&device);
    anyhow::bail!("未能确认 NTFS 可写挂载，磁盘已恢复为系统只读挂载");
}

pub fn inspect_path(path: &Path) -> Option<VolumeInfo> {
    let mounts = read_mounts().ok()?;
    mounts
        .into_iter()
        .filter(|mount| is_path_within(path, &mount.path))
        .max_by_key(|mount| mount.path.components().count())
        .map(volume_info)
}

pub fn ensure_writable(path: &Path) -> Result<()> {
    let Some(volume) = inspect_path(path) else {
        return Ok(());
    };
    if volume.is_ntfs() && volume.read_only {
        anyhow::bail!(
            "无法写入 {}：NTFS 卷当前以只读方式挂载。请安装支持 NTFS 写入的驱动（如 Paragon NTFS 或 NTFS-3G），重新挂载后重试",
            path.display()
        );
    }
    Ok(())
}

pub(crate) fn annotate(paths: Vec<PathBuf>) -> Vec<VolumeInfo> {
    let mounts = read_mounts().unwrap_or_default();
    paths
        .into_iter()
        .map(|path| {
            mounts
                .iter()
                .filter(|mount| is_path_within(&path, &mount.path))
                .max_by_key(|mount| mount.path.components().count())
                .map(|mount| volume_info_for_path(&path, mount))
                .unwrap_or(VolumeInfo {
                    path,
                    filesystem: "unknown".to_string(),
                    read_only: false,
                })
        })
        .collect()
}

fn read_mounts() -> Result<Vec<MountInfo>> {
    let output = Command::new("/sbin/mount")
        .output()
        .context("无法读取 macOS 挂载信息")?;
    if !output.status.success() {
        anyhow::bail!("读取 macOS 挂载信息失败");
    }
    Ok(parse_mounts(&String::from_utf8_lossy(&output.stdout)))
}

fn volume_info(mount: MountInfo) -> VolumeInfo {
    volume_info_for_path(&mount.path, &mount)
}

fn volume_info_for_path(path: &Path, mount: &MountInfo) -> VolumeInfo {
    let needs_diskutil = matches!(
        mount.filesystem.to_ascii_lowercase().as_str(),
        "fuse" | "fuseblk" | "fusefs" | "fskit" | "macfuse" | "osxfuse" | "unknown"
    );
    let (filesystem, diskutil_read_only) = if needs_diskutil {
        diskutil_filesystem(path).unwrap_or((mount.filesystem.clone(), None))
    } else {
        (mount.filesystem.clone(), None)
    };
    VolumeInfo {
        path: mount.path.clone(),
        filesystem,
        read_only: mount.read_only || diskutil_read_only.unwrap_or(false),
    }
}

fn ntfs_mount_helper() -> Option<PathBuf> {
    [
        "/opt/homebrew/opt/ntfs-3g-mac/sbin/mount_ntfs",
        "/usr/local/opt/ntfs-3g-mac/sbin/mount_ntfs",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

fn supports_fskit() -> bool {
    let output = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| version_supports_fskit(&String::from_utf8_lossy(&output.stdout)))
}

fn version_supports_fskit(version: &str) -> bool {
    let mut parts = version.trim().split('.');
    let major = parts.next().and_then(|part| part.parse::<u32>().ok());
    let minor = parts.next().and_then(|part| part.parse::<u32>().ok());
    matches!((major, minor), (Some(major), Some(minor)) if major > 15 || (major == 15 && minor >= 4))
}

fn disk_is_external(device: &str) -> bool {
    Command::new("/usr/sbin/diskutil")
        .args(["info", device])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                line.trim()
                    .strip_prefix("Device Location:")
                    .is_some_and(|location| location.trim() == "External")
            })
        })
}

fn restore_native_mount(device: &str) {
    let _ = Command::new("/usr/sbin/diskutil")
        .args(["mount", device])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn diskutil_filesystem(path: &Path) -> Option<(String, Option<bool>)> {
    let output = Command::new("/usr/sbin/diskutil")
        .arg("info")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_diskutil_info(&String::from_utf8_lossy(&output.stdout))
}

fn parse_diskutil_info(output: &str) -> Option<(String, Option<bool>)> {
    let filesystem = output.lines().find_map(|line| {
        line.strip_prefix("   File System Personality:")
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "None")
            .map(str::to_string)
    })?;
    let read_only = output.lines().find_map(|line| {
        line.strip_prefix("   Volume Read-Only:")
            .map(str::trim)
            .map(|value| value.starts_with("Yes"))
    });
    Some((filesystem, read_only))
}

fn parse_mounts(output: &str) -> Vec<MountInfo> {
    output.lines().filter_map(parse_mount_line).collect()
}

fn parse_mount_line(line: &str) -> Option<MountInfo> {
    let (source, mounted) = line.split_once(" on ")?;
    let options_start = mounted.rfind(" (")?;
    let path = PathBuf::from(&mounted[..options_start]);
    let options = mounted[options_start + 2..].strip_suffix(')')?;
    let mut options = options.split(',');
    let filesystem = options.next()?.to_string();
    let read_only = options.any(|option| option.trim() == "read-only");
    Some(MountInfo {
        source: PathBuf::from(source),
        path,
        filesystem,
        read_only,
    })
}

fn is_path_within(path: &Path, mount: &Path) -> bool {
    path == mount || path.strip_prefix(mount).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{
        is_path_within, parse_diskutil_info, parse_mount_line, parse_mounts, version_supports_fskit,
    };
    use std::path::Path;

    #[test]
    fn parses_ntfs_read_only_mounts_with_spaces() {
        let mount = parse_mount_line(
            "/dev/disk4s1 on /Volumes/External Disk (ntfs, local, read-only, noowners)",
        )
        .expect("mount line");

        assert_eq!(mount.path, Path::new("/Volumes/External Disk"));
        assert_eq!(mount.source, Path::new("/dev/disk4s1"));
        assert_eq!(mount.filesystem, "ntfs");
        assert!(mount.read_only);
    }

    #[test]
    fn selects_the_deepest_mount_for_nested_paths() {
        let mounts = parse_mounts(
            "/dev/disk1 on / (apfs, local, read-only)\n/dev/disk4s1 on /Volumes/USB (ntfs, local, read-only)",
        );
        let selected = mounts
            .iter()
            .filter(|mount| is_path_within(Path::new("/Volumes/USB/file.txt"), &mount.path))
            .max_by_key(|mount| mount.path.components().count())
            .expect("nested mount");

        assert_eq!(selected.filesystem, "ntfs");
        assert!(selected.read_only);
    }

    #[test]
    fn parses_ntfs_personality_from_diskutil() {
        let info = parse_diskutil_info(
            "   File System Personality: NTFS\n   Volume Read-Only: Yes (read-only mount flag set)",
        )
        .expect("diskutil info");

        assert_eq!(info.0, "NTFS");
        assert_eq!(info.1, Some(true));
    }

    #[test]
    fn fskit_requires_macos_15_4_or_newer() {
        assert!(!version_supports_fskit("15.3.2"));
        assert!(version_supports_fskit("15.4"));
        assert!(version_supports_fskit("26.5.2"));
    }
}
