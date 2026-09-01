use anyhow::{Context as _, Result};
use std::{path::PathBuf, process::Command};

#[derive(Clone, Debug, Eq, PartialEq)]
struct SmbLocation {
    authority: String,
    server: String,
    share: Option<String>,
    path_within_share: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SmbShare {
    pub name: String,
    pub address: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SmbNavigation {
    Server {
        address: String,
        shares: Vec<SmbShare>,
    },
    Directory {
        path: PathBuf,
        server_address: String,
        mount_path: PathBuf,
    },
}

const MOUNT_SMB_SCRIPT: &str = r#"
on run argv
    set networkAddress to item 1 of argv
    set mountedVolume to mount volume networkAddress
    if mountedVolume is not missing value then
        return POSIX path of mountedVolume
    end if
    return ""
end run
"#;

pub(crate) fn looks_like_address(input: &str) -> bool {
    let input = input.trim();
    let lower = input.to_ascii_lowercase();
    lower.starts_with("smb:") || input.starts_with("//") || input.starts_with("\\\\")
}

pub(crate) fn connect(input: &str) -> Result<SmbNavigation> {
    let location = parse_location(input)?;
    if location.share.is_none() {
        return list_server_shares(&location);
    }
    if let Some(mount_path) = find_existing_mount(&location) {
        return directory_navigation(&location, mount_path);
    }

    let mount_url = mount_url(&location)?;
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", MOUNT_SMB_SCRIPT, "--"])
        .arg(&mount_url)
        .output()
        .context("无法启动 macOS SMB 连接服务")?;

    if !output.status.success() {
        if let Some(mount_path) = find_existing_mount(&location) {
            return directory_navigation(&location, mount_path);
        }
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if message.contains("(-128)") {
            anyhow::bail!("已取消连接 SMB 服务器");
        }
        if message.is_empty() {
            anyhow::bail!("无法连接 SMB 服务器，请检查 NAS 地址、网络和访问权限");
        }
        anyhow::bail!("无法连接 SMB 服务器：{message}");
    }

    let returned_path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let mount_path = returned_path
        .is_dir()
        .then_some(returned_path)
        .or_else(|| find_existing_mount(&location))
        .ok_or_else(|| anyhow::anyhow!("SMB 共享目录已连接，但 macOS 未返回可访问的挂载位置"))?;

    directory_navigation(&location, mount_path)
}

fn directory_navigation(location: &SmbLocation, mount_path: PathBuf) -> Result<SmbNavigation> {
    let path = destination_within_share(location, mount_path.clone())?;
    Ok(SmbNavigation::Directory {
        path,
        server_address: format!("smb://{}", location.authority),
        mount_path,
    })
}

fn list_server_shares(location: &SmbLocation) -> Result<SmbNavigation> {
    let mut result = run_smbutil_view(location);
    if result.is_err() {
        authenticate_server(location)?;
        result = run_smbutil_view(location);
    }
    let output = result?;
    let shares = parse_smbutil_shares(&output)
        .into_iter()
        .map(|name| SmbShare {
            address: format!("smb://{}/{}", location.authority, percent_encode(&name)),
            name,
        })
        .collect::<Vec<_>>();
    if shares.is_empty() {
        anyhow::bail!("该 SMB 服务器没有可访问的共享文件夹");
    }
    Ok(SmbNavigation::Server {
        address: format!("smb://{}", location.authority),
        shares,
    })
}

fn run_smbutil_view(location: &SmbLocation) -> Result<String> {
    let output = Command::new("/usr/bin/smbutil")
        .args(["view", "-N", "-G"])
        .arg(format!("//{}", location.authority))
        .output()
        .context("无法启动 macOS SMB 共享查询服务")?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if message.is_empty() {
            anyhow::bail!("无法读取 SMB 服务器共享列表");
        }
        anyhow::bail!("无法读取 SMB 服务器共享列表：{message}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn authenticate_server(location: &SmbLocation) -> Result<()> {
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", MOUNT_SMB_SCRIPT, "--"])
        .arg(format!("smb://{}", location.authority))
        .output()
        .context("无法启动 macOS SMB 认证服务")?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if message.contains("(-128)") {
        anyhow::bail!("已取消连接 SMB 服务器");
    }
    if message.is_empty() {
        anyhow::bail!("SMB 服务器认证失败");
    }
    anyhow::bail!("SMB 服务器认证失败：{message}")
}

fn parse_smbutil_shares(output: &str) -> Vec<String> {
    let lines = output.lines().collect::<Vec<_>>();
    let Some((header_index, type_column)) = lines.iter().enumerate().find_map(|(index, line)| {
        let share_column = line.find("Share")?;
        let type_column = line[share_column + 5..].find("Type")? + share_column + 5;
        Some((index, type_column))
    }) else {
        return Vec::new();
    };

    let mut shares = lines[header_index + 1..]
        .iter()
        .filter_map(|line| {
            if line.len() <= type_column {
                return None;
            }
            let name = line[..type_column].trim();
            let resource_type = line[type_column..].split_whitespace().next()?;
            if name.is_empty() || !resource_type.eq_ignore_ascii_case("disk") {
                return None;
            }
            Some(percent_decode(name).unwrap_or_else(|_| name.to_string()))
        })
        .collect::<Vec<_>>();
    shares.sort_by_key(|name| name.to_lowercase());
    shares.dedup();
    shares
}

fn mount_url(location: &SmbLocation) -> Result<String> {
    let share = location
        .share
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("请指定 SMB 共享目录"))?;
    Ok(format!(
        "smb://{}/{}",
        location.authority,
        percent_encode(share)
    ))
}

fn destination_within_share(location: &SmbLocation, mount_path: PathBuf) -> Result<PathBuf> {
    let destination = mount_path.join(&location.path_within_share);
    if !destination.is_dir() {
        anyhow::bail!("SMB 共享目录中不存在文件夹：{}", destination.display());
    }
    Ok(destination)
}

fn find_existing_mount(location: &SmbLocation) -> Option<PathBuf> {
    let output = Command::new("/sbin/mount").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout))?
        .lines()
        .find_map(|line| parse_matching_mount(line, location))
}

fn parse_matching_mount(line: &str, location: &SmbLocation) -> Option<PathBuf> {
    let (source, mounted) = line.split_once(" on ")?;
    let options_start = mounted.rfind(" (")?;
    let options = mounted[options_start + 2..].strip_suffix(')')?;
    if options.split(',').next()?.trim() != "smbfs" {
        return None;
    }

    let source = source.strip_prefix("//")?;
    let server_and_share = source.rsplit_once('@').map_or(source, |(_, rest)| rest);
    let (server, share) = server_and_share.split_once('/')?;
    let server = decode_mount_field(server)?;
    let share = decode_mount_field(share)?;
    if !server.eq_ignore_ascii_case(&location.server)
        || location.share.as_deref() != Some(share.as_str())
    {
        return None;
    }

    Some(PathBuf::from(&mounted[..options_start]))
}

fn decode_mount_field(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1..index + 4]
                .iter()
                .all(|byte| matches!(byte, b'0'..=b'7'))
        {
            let value = (bytes[index + 1] - b'0') * 64
                + (bytes[index + 2] - b'0') * 8
                + (bytes[index + 3] - b'0');
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    percent_decode(&decoded).ok()
}

fn parse_location(input: &str) -> Result<SmbLocation> {
    let input = input.trim();
    let lower = input.to_ascii_lowercase();
    let remainder = if lower.starts_with("smb://") {
        &input[6..]
    } else if input.starts_with("//") || input.starts_with("\\\\") {
        input.trim_start_matches(['/', '\\'])
    } else if lower.starts_with("smb:") {
        anyhow::bail!("SMB 地址格式无效，请使用 smb://服务器/共享目录");
    } else {
        anyhow::bail!("不是有效的 SMB 地址");
    };

    let normalized = remainder.replace('\\', "/");
    if normalized.contains(['?', '#']) {
        anyhow::bail!("SMB 地址不能包含查询参数或片段");
    }

    let (authority, remote_path) = normalized
        .split_once('/')
        .map_or((normalized.as_str(), ""), |(authority, path)| {
            (authority, path)
        });
    if authority.is_empty() || authority.chars().any(char::is_whitespace) {
        anyhow::bail!("SMB 服务器地址无效");
    }
    let server = authority
        .rsplit_once('@')
        .map_or(authority, |(_, server)| server);
    if server.is_empty() {
        anyhow::bail!("SMB 服务器地址无效");
    }

    let mut components = remote_path.split('/').filter(|part| !part.is_empty());
    let share = components
        .next()
        .map(|share| {
            let share = percent_decode(share).context("SMB 共享目录名称编码无效")?;
            validate_component(&share, "SMB 共享目录名称")?;
            Ok::<_, anyhow::Error>(share)
        })
        .transpose()?;

    let mut path_within_share = PathBuf::new();
    for component in components {
        let component = percent_decode(component).context("SMB 子路径编码无效")?;
        match component.as_str() {
            "." => {}
            ".." => {
                if !path_within_share.pop() {
                    anyhow::bail!("SMB 子路径不能超出共享目录");
                }
            }
            _ => {
                validate_component(&component, "SMB 子路径")?;
                path_within_share.push(component);
            }
        }
    }

    Ok(SmbLocation {
        authority: authority.to_string(),
        server: server.to_string(),
        share,
        path_within_share,
    })
}

fn validate_component(component: &str, label: &str) -> Result<()> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.contains(['/', '\\', '\0'])
    {
        anyhow::bail!("{label}无效");
    }
    Ok(())
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                anyhow::bail!("不完整的百分号编码");
            }
            let high = hex_value(bytes[index + 1]).context("无效的百分号编码")?;
            let low = hex_value(bytes[index + 2]).context("无效的百分号编码")?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).context("SMB 地址必须使用 UTF-8 编码")
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(
                char::from_digit((byte >> 4) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            encoded.push(
                char::from_digit((byte & 0x0f) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    encoded
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{SmbLocation, looks_like_address, parse_location, parse_matching_mount};
    use std::path::PathBuf;

    #[test]
    fn parses_smb_url_and_preserves_subdirectory() {
        assert_eq!(
            parse_location("smb://nas.local/Media/Movies/2026").unwrap(),
            SmbLocation {
                authority: "nas.local".to_string(),
                server: "nas.local".to_string(),
                share: Some("Media".to_string()),
                path_within_share: PathBuf::from("Movies/2026"),
            }
        );
    }

    #[test]
    fn parses_windows_unc_address() {
        assert_eq!(
            parse_location(r"\\192.168.1.8\共享文件\照片").unwrap(),
            SmbLocation {
                authority: "192.168.1.8".to_string(),
                server: "192.168.1.8".to_string(),
                share: Some("共享文件".to_string()),
                path_within_share: PathBuf::from("照片"),
            }
        );
    }

    #[test]
    fn decodes_url_components_without_letting_path_escape_share() {
        assert_eq!(
            parse_location("//nas/My%20Files/a/../b").unwrap(),
            SmbLocation {
                authority: "nas".to_string(),
                server: "nas".to_string(),
                share: Some("My Files".to_string()),
                path_within_share: PathBuf::from("b"),
            }
        );
        assert!(parse_location("smb://nas/share/../..//private").is_err());
    }

    #[test]
    fn recognizes_supported_smb_address_forms() {
        assert!(looks_like_address("smb://nas/share"));
        assert!(looks_like_address("//nas/share"));
        assert!(looks_like_address(r"\\nas\share"));
        assert!(!looks_like_address("Documents/share"));
    }

    #[test]
    fn accepts_a_server_root_without_a_share_name() {
        let root = parse_location("smb://nas.local").unwrap();
        assert_eq!(root.authority, "nas.local");
        assert_eq!(root.server, "nas.local");
        assert_eq!(root.share, None);
        assert_eq!(parse_location("smb://nas.local/").unwrap(), root);
    }

    #[test]
    fn matches_an_already_mounted_share() {
        let location = parse_location("smb://person@nas.local/My%20Files/photos").unwrap();
        assert_eq!(
            parse_matching_mount(
                "//person@NAS.local/My\\040Files on /Volumes/My Files (smbfs, nodev, nosuid)",
                &location,
            ),
            Some(PathBuf::from("/Volumes/My Files"))
        );
        assert_eq!(
            parse_matching_mount(
                "//person@nas.local/Other on /Volumes/Other (smbfs, nodev)",
                &location,
            ),
            None
        );
    }

    #[test]
    fn parses_disk_shares_from_smbutil_output() {
        let output = "    Share                                           Type    Comments\n    -------------------------------\n    IPC$                                            Pipe    IPC Service\n    Media                                           Disk\n    Shared Photos                                   Disk    Family files\n    Office Printer                                  Printer\n    3 shares listed from 4 available\n";
        assert_eq!(
            super::parse_smbutil_shares(output),
            vec!["Media".to_string(), "Shared Photos".to_string()]
        );
    }

    #[test]
    fn parses_non_ascii_share_names_from_macos_columns() {
        let output = "Share                                           Type    Comments\n-------------------------------\n备份视频                                    Disk    备份视频\n临时文件夹                                 Disk    临时文件夹\nIPC$                                            Pipe    IPC Service\n数据备份                                    Disk    数据备份\n\n3 shares listed\n";
        assert_eq!(
            super::parse_smbutil_shares(output),
            vec![
                "临时文件夹".to_string(),
                "备份视频".to_string(),
                "数据备份".to_string(),
            ]
        );
    }
}
