use super::smb_credentials;
use anyhow::{Context as _, Result};
use std::{
    io::{Read as _, Write as _},
    path::PathBuf,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use zeroize::Zeroize as _;

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
pub(crate) struct SmbMountInfo {
    pub server_address: String,
    pub share_name: String,
    pub mount_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SmbNavigation {
    AuthenticationRequired {
        address: String,
        suggested_username: Option<String>,
    },
    Server {
        address: String,
        shares: Vec<SmbShare>,
    },
    Directory {
        path: PathBuf,
        server_address: String,
        share_name: String,
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

const SMB_QUERY_TIMEOUT: Duration = Duration::from_secs(15);
const SMB_MOUNT_TIMEOUT: Duration = Duration::from_secs(60);
const EXPECT_SMB_PASSWORD_SCRIPT: &str = r#"
set timeout 15
match_max 1000000
log_user 0
if {[gets stdin password] < 0} {
    exit 2
}
spawn -noecho $env(FLOWFILE_SMB_PROGRAM) view $env(FLOWFILE_SMB_TARGET)
set submitted 0
set captured ""
while {1} {
    expect {
        -nocase -re {password[^:\r\n]*:} {
            append captured $expect_out(buffer)
            if {$submitted} {
                catch {exec /bin/kill -KILL [exp_pid]}
                catch {close}
                catch {wait}
                exit 77
            }
            send -- "$password\r"
            set submitted 1
        }
        timeout {
            catch {exec /bin/kill -KILL [exp_pid]}
            catch {close}
            catch {wait}
            exit 124
        }
        eof {
            append captured $expect_out(buffer)
            break
        }
    }
}
set result [wait]
set captured [string map [list $password ""] $captured]
send_user -- $captured
exit [lindex $result 3]
"#;

pub(crate) fn looks_like_address(input: &str) -> bool {
    let input = input.trim();
    let lower = input.to_ascii_lowercase();
    lower.starts_with("smb:") || input.starts_with("//") || input.starts_with("\\\\")
}

pub(crate) fn connect(input: &str) -> Result<SmbNavigation> {
    let location = parse_location(input)?;
    if location.share.is_some()
        && let Some(mount_path) = find_existing_mount(&location)
    {
        return directory_navigation(&location, mount_path);
    }

    let cached_credential = match smb_credentials::load(&location.server) {
        Ok(credential) => credential,
        Err(error) => {
            eprintln!(
                "FlowFile: 无法读取 {} 的 SMB 钥匙串凭证：{error}",
                location.server
            );
            None
        }
    };
    if let Some(credential) = cached_credential {
        let suggested_username = credential.username.clone();
        match authenticated_navigation(&location, &credential.username, &credential.password)? {
            Some(navigation) => return Ok(navigation),
            None => {
                if let Err(error) = smb_credentials::delete(&location.server) {
                    eprintln!(
                        "FlowFile: 无法删除 {} 的失效 SMB 凭证：{error}",
                        location.server
                    );
                }
                return Ok(authentication_required(&location, Some(suggested_username)));
            }
        }
    }

    unauthenticated_navigation(&location)
}

pub(crate) fn connect_with_credentials(
    input: &str,
    username: &str,
    password: &str,
) -> Result<SmbNavigation> {
    let location = parse_location(input)?;
    let Some(navigation) = authenticated_navigation(&location, username, password)? else {
        anyhow::bail!("用户名或密码不正确，请重试");
    };
    smb_credentials::save(&location.server, username, password)?;
    Ok(navigation)
}

fn unauthenticated_navigation(location: &SmbLocation) -> Result<SmbNavigation> {
    let output = match run_smbutil_view(location, None)? {
        ShareQuery::Output(output) => output,
        ShareQuery::AuthenticationRequired => {
            return Ok(authentication_required(
                location,
                username_from_authority(&location.authority),
            ));
        }
    };
    if location.share.is_none() {
        server_navigation(location, output)
    } else {
        mount_share(location, None)
    }
}

fn authenticated_navigation(
    location: &SmbLocation,
    username: &str,
    password: &str,
) -> Result<Option<SmbNavigation>> {
    let credentials = SmbCredentials { username, password };
    let output = match run_smbutil_view(location, Some(credentials))? {
        ShareQuery::Output(output) => output,
        ShareQuery::AuthenticationRequired => return Ok(None),
    };
    let authenticated_location = location_with_username(location, username);
    if authenticated_location.share.is_none() {
        server_navigation(&authenticated_location, output).map(Some)
    } else {
        mount_share(&authenticated_location, Some(credentials)).map(Some)
    }
}

fn mount_share(
    location: &SmbLocation,
    credentials: Option<SmbCredentials<'_>>,
) -> Result<SmbNavigation> {
    if let Some(mount_path) = find_existing_mount(location) {
        return directory_navigation(location, mount_path);
    }
    let mount_url = mount_url(location)?;
    let mut command = Command::new("/usr/bin/osascript");
    let output = if let Some(credentials) = credentials {
        // Feed the authenticated mount script over stdin. This keeps the
        // password out of argv, the environment, temporary files, and logs.
        let mut script = authenticated_mount_script(&mount_url, credentials);
        command.arg("-");
        let output = run_command_with_timeout(
            &mut command,
            Some(script.as_bytes()),
            SMB_MOUNT_TIMEOUT,
            "连接 SMB 服务器超时，请检查 NAS 地址和网络后重试",
        );
        script.zeroize();
        output?
    } else {
        command.args(["-e", MOUNT_SMB_SCRIPT, "--"]).arg(&mount_url);
        run_command_with_timeout(
            &mut command,
            None,
            SMB_MOUNT_TIMEOUT,
            "连接 SMB 服务器超时，请检查 NAS 地址和网络后重试",
        )?
    };

    if !output.status.success() {
        if let Some(mount_path) = find_existing_mount(location) {
            return directory_navigation(location, mount_path);
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
        .or_else(|| find_existing_mount(location))
        .ok_or_else(|| anyhow::anyhow!("SMB 共享目录已连接，但 macOS 未返回可访问的挂载位置"))?;

    directory_navigation(location, mount_path)
}

fn authenticated_mount_script(mount_url: &str, credentials: SmbCredentials<'_>) -> String {
    format!(
        "on run\nset networkAddress to {}\nset mountedVolume to mount volume networkAddress as user name {} with password {}\nif mountedVolume is not missing value then\nreturn POSIX path of mountedVolume\nend if\nreturn \"\"\nend run\n",
        applescript_string_literal(mount_url),
        applescript_string_literal(credentials.username),
        applescript_string_literal(credentials.password),
    )
}

fn applescript_string_literal(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('"');
    for character in value.chars() {
        match character {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\" & linefeed & \""),
            '\r' => literal.push_str("\" & return & \""),
            '\t' => literal.push_str("\" & tab & \""),
            character if character.is_control() => {
                literal.push_str(&format!("\" & character id {} & \"", character as u32));
            }
            character => literal.push(character),
        }
    }
    literal.push('"');
    literal
}

fn directory_navigation(location: &SmbLocation, mount_path: PathBuf) -> Result<SmbNavigation> {
    let path = destination_within_share(location, mount_path.clone())?;
    Ok(SmbNavigation::Directory {
        path,
        server_address: format!("smb://{}", location.server),
        share_name: location
            .share
            .clone()
            .ok_or_else(|| anyhow::anyhow!("请指定 SMB 共享目录"))?,
        mount_path,
    })
}

fn authentication_required(
    location: &SmbLocation,
    suggested_username: Option<String>,
) -> SmbNavigation {
    SmbNavigation::AuthenticationRequired {
        address: logical_address(location),
        suggested_username,
    }
}

fn location_with_username(location: &SmbLocation, username: &str) -> SmbLocation {
    let mut location = location.clone();
    location.authority = format!("{}@{}", encode_username(username), location.server);
    location
}

fn logical_address(location: &SmbLocation) -> String {
    let mut address = format!("smb://{}", location.authority);
    if let Some(share) = &location.share {
        address.push('/');
        address.push_str(&percent_encode(share));
    }
    for component in location.path_within_share.components() {
        address.push('/');
        address.push_str(&percent_encode(&component.as_os_str().to_string_lossy()));
    }
    address
}

fn server_navigation(location: &SmbLocation, output: String) -> Result<SmbNavigation> {
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

#[derive(Clone, Copy)]
struct SmbCredentials<'a> {
    username: &'a str,
    password: &'a str,
}

enum ShareQuery {
    Output(String),
    AuthenticationRequired,
}

fn run_smbutil_view(
    location: &SmbLocation,
    credentials: Option<SmbCredentials<'_>>,
) -> Result<ShareQuery> {
    let target = credentials.map_or_else(
        || format!("//{}", location.authority),
        |credentials| {
            format!(
                "//{}@{}",
                encode_username(credentials.username),
                location.server
            )
        },
    );
    let output = if let Some(credentials) = credentials {
        run_smbutil_with_password(&target, credentials.password)?
    } else {
        let mut command = Command::new("/usr/bin/smbutil");
        command.args(["view", "-N", "-G"]).arg(target);
        run_command_with_timeout(
            &mut command,
            None,
            SMB_QUERY_TIMEOUT,
            "连接 SMB 服务器超时，请检查 NAS 地址和网络后重试",
        )?
    };
    if output.status.code() == Some(124) {
        anyhow::bail!("连接 SMB 服务器超时，请检查 NAS 地址和网络后重试");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if is_authentication_error(output.status.code(), &message)
            || is_authentication_error(output.status.code(), &stdout)
        {
            return Ok(ShareQuery::AuthenticationRequired);
        }
        if credentials.is_some() {
            anyhow::bail!("无法连接 SMB 服务器，请检查 NAS 地址、网络和访问权限");
        }
        if message.is_empty() {
            anyhow::bail!("无法直接读取 SMB 服务器共享列表，请检查网络或访问权限");
        }
        anyhow::bail!("无法读取 SMB 服务器共享列表：{message}");
    }
    if is_authentication_error(None, &stdout) {
        return Ok(ShareQuery::AuthenticationRequired);
    }
    Ok(ShareQuery::Output(stdout.into_owned()))
}

fn run_smbutil_with_password(target: &str, password: &str) -> Result<std::process::Output> {
    run_smbutil_with_password_command(
        std::path::Path::new("/usr/bin/smbutil"),
        target,
        password,
        SMB_QUERY_TIMEOUT + Duration::from_secs(2),
    )
}

fn run_smbutil_with_password_command(
    program: &std::path::Path,
    target: &str,
    password: &str,
    timeout: Duration,
) -> Result<std::process::Output> {
    // smbutil requires a terminal for passwords. Expect supplies that terminal,
    // submits the password once, and treats a second prompt as an authentication
    // failure instead of waiting forever. The password only travels over stdin;
    // it is never placed in argv, the environment, logs, or the session file.
    let mut input = password.as_bytes().to_vec();
    input.push(b'\n');
    let mut command = Command::new("/usr/bin/expect");
    command
        .args(["-c", EXPECT_SMB_PASSWORD_SCRIPT])
        .env("FLOWFILE_SMB_PROGRAM", program)
        .env("FLOWFILE_SMB_TARGET", target);
    let output = run_command_with_timeout(
        &mut command,
        Some(&input),
        timeout,
        "SMB 登录超时，请检查 NAS 地址和网络后重试",
    );
    input.zeroize();
    output
}

fn run_command_with_timeout(
    command: &mut Command,
    stdin: Option<&[u8]>,
    timeout: Duration,
    timeout_message: &str,
) -> Result<Output> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("无法启动 SMB 系统命令")?;

    let stdout = child.stdout.take().context("无法读取 SMB 命令输出")?;
    let stderr = child.stderr.take().context("无法读取 SMB 命令错误")?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(usize::MAX as u64).read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(usize::MAX as u64).read_to_end(&mut bytes);
        bytes
    });

    if let Some(input) = stdin {
        let mut child_stdin = child.stdin.take().context("无法打开 SMB 命令输入")?;
        child_stdin.write_all(input).context("无法提交 SMB 凭据")?;
        drop(child_stdin);
    }

    let started_at = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(timeout_message.to_string());
        }
        thread::sleep(Duration::from_millis(25));
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("SMB 命令输出读取任务异常终止"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("SMB 命令错误读取任务异常终止"))?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn username_from_authority(authority: &str) -> Option<String> {
    authority
        .rsplit_once('@')
        .map(|(username, _)| username)
        .filter(|username| !username.is_empty())
        .and_then(|username| percent_decode(username).ok())
}

fn encode_username(username: &str) -> String {
    if let Some((domain, account)) = username.split_once(['\\', ';']) {
        format!("{};{}", percent_encode(domain), percent_encode(account))
    } else {
        percent_encode(username)
    }
}

fn is_authentication_error(status_code: Option<i32>, message: &str) -> bool {
    status_code == Some(77)
        || message
            .to_ascii_lowercase()
            .contains("authentication error")
        || message
            .to_ascii_lowercase()
            .contains("server rejected the authentication")
}

fn parse_smbutil_shares(output: &str) -> Vec<String> {
    let lines = output.lines().collect::<Vec<_>>();
    let Some((header_index, type_column)) = lines.iter().enumerate().find_map(|(index, line)| {
        line.find("Share")?;
        Some((index, line.find("Type")?))
    }) else {
        return Vec::new();
    };

    let mut shares = lines[header_index + 1..]
        .iter()
        .filter_map(|line| {
            let (type_index, resource_type) = smbutil_resource_type(line, type_column)?;
            let name = line[..type_index].trim();
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

fn smbutil_resource_type(line: &str, expected_column: usize) -> Option<(usize, &str)> {
    const RESOURCE_TYPES: &[&str] = &["Disk", "Pipe", "Printer", "Comm"];
    let mut search_from = 0;
    line.split_whitespace()
        .filter_map(|token| {
            let relative_index = line[search_from..].find(token)?;
            let index = search_from + relative_index;
            search_from = index + token.len();
            RESOURCE_TYPES
                .iter()
                .any(|resource_type| token.eq_ignore_ascii_case(resource_type))
                .then_some((index, token))
        })
        .min_by_key(|(index, _)| index.abs_diff(expected_column))
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

pub(crate) fn mounted_location_for_path(path: &std::path::Path) -> Option<SmbMountInfo> {
    if !path.starts_with("/Volumes") {
        return None;
    }
    let output = Command::new("/sbin/mount").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout))?
        .lines()
        .filter_map(parse_smb_mount)
        .filter(|mount| path.starts_with(&mount.mount_path))
        .max_by_key(|mount| mount.mount_path.components().count())
}

fn parse_matching_mount(line: &str, location: &SmbLocation) -> Option<PathBuf> {
    let mount = parse_smb_mount(line)?;
    if !mount
        .server_address
        .strip_prefix("smb://")?
        .eq_ignore_ascii_case(&location.server)
        || location.share.as_deref() != Some(mount.share_name.as_str())
    {
        return None;
    }
    Some(mount.mount_path)
}

fn parse_smb_mount(line: &str) -> Option<SmbMountInfo> {
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
    Some(SmbMountInfo {
        server_address: format!("smb://{server}"),
        share_name: share,
        mount_path: PathBuf::from(&mounted[..options_start]),
    })
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
    if let Some((user_info, _)) = authority.rsplit_once('@')
        && percent_decode(user_info)?.contains(':')
    {
        anyhow::bail!("SMB 地址不能包含密码，请在登录窗口中输入凭据");
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
    use super::{
        SmbCredentials, SmbLocation, SmbNavigation, applescript_string_literal,
        authenticated_mount_script, authentication_required, encode_username,
        is_authentication_error, location_with_username, logical_address, looks_like_address,
        parse_location, parse_matching_mount, run_command_with_timeout,
        run_smbutil_with_password_command,
    };
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        path::PathBuf,
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    fn system_command_timeout_returns_without_waiting_for_the_process() {
        let mut command = Command::new("/bin/sleep");
        command.arg("5");
        let started_at = Instant::now();
        let error =
            run_command_with_timeout(&mut command, None, Duration::from_millis(50), "测试超时")
                .expect_err("sleep should be terminated at the deadline");

        assert_eq!(error.to_string(), "测试超时");
        assert!(started_at.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn system_command_runner_captures_output_and_exit_status() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf output; printf error >&2; exit 7"]);
        let output =
            run_command_with_timeout(&mut command, None, Duration::from_secs(1), "不应超时")
                .unwrap();

        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"output");
        assert_eq!(output.stderr, b"error");
    }

    #[test]
    fn repeated_password_prompt_returns_authentication_failure_without_leaking_password() {
        let directory = tempfile::tempdir().unwrap();
        let mock_smbutil = directory.path().join("mock smbutil");
        fs::write(
            &mock_smbutil,
            r#"#!/bin/sh
printf 'Password: '
IFS= read -r password
if [ "$password" = "correct-password" ]; then
    printf '\nShare                                           Type    Comments\nMedia                                           Disk\n'
    exit 0
fi
printf '\nPassword: '
IFS= read -r password
exit 1
"#,
        )
        .unwrap();
        fs::set_permissions(&mock_smbutil, fs::Permissions::from_mode(0o700)).unwrap();

        let started_at = Instant::now();
        let output = run_smbutil_with_password_command(
            &mock_smbutil,
            "//tester@nas.local",
            "wrong-secret",
            Duration::from_secs(2),
        )
        .unwrap();

        assert_eq!(output.status.code(), Some(77));
        assert!(started_at.elapsed() < Duration::from_secs(1));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("wrong-secret"));
    }

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
    fn authentication_prompt_preserves_direct_share_destination() {
        let location = parse_location("smb://nas.local/My%20Files/photos").expect("valid location");

        assert_eq!(
            logical_address(&location),
            "smb://nas.local/My%20Files/photos"
        );
        assert_eq!(
            authentication_required(&location, Some("alice".to_string())),
            SmbNavigation::AuthenticationRequired {
                address: "smb://nas.local/My%20Files/photos".to_string(),
                suggested_username: Some("alice".to_string()),
            }
        );
    }

    #[test]
    fn authenticated_location_includes_encoded_username_without_a_password() {
        let location = parse_location("smb://nas.local/Media").expect("valid location");
        let authenticated = location_with_username(&location, r"OFFICE\张三");

        assert_eq!(
            authenticated.authority,
            "OFFICE;%E5%BC%A0%E4%B8%89@nas.local"
        );
        let address = logical_address(&authenticated);
        assert_eq!(address, "smb://OFFICE;%E5%BC%A0%E4%B8%89@nas.local/Media");
        let user_info = address
            .strip_prefix("smb://")
            .unwrap()
            .split('@')
            .next()
            .unwrap();
        assert!(!user_info.contains(':'));
    }

    #[test]
    fn applescript_mount_values_escape_quotes_slashes_and_line_breaks() {
        assert_eq!(
            applescript_string_literal("a\\\"b\nc"),
            "\"a\\\\\\\"b\" & linefeed & \"c\""
        );
    }

    #[test]
    fn authenticated_mount_script_compiles_without_exposing_credentials_in_argv() {
        let directory = tempfile::tempdir().unwrap();
        let compiled_script = directory.path().join("mount.scpt");
        let script = authenticated_mount_script(
            "smb://nas.local/My%20Files",
            SmbCredentials {
                username: "测试用户",
                password: "quote-\"-slash-\\-line-\n",
            },
        );
        let mut command = Command::new("/usr/bin/osacompile");
        command.args(["-o"]).arg(&compiled_script).arg("-");
        let output = run_command_with_timeout(
            &mut command,
            Some(script.as_bytes()),
            Duration::from_secs(2),
            "AppleScript 编译超时",
        )
        .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(compiled_script.is_file());
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
    fn recognizes_authentication_failures_without_masking_network_errors() {
        assert!(is_authentication_error(
            Some(77),
            "server rejected the authentication: Authentication error"
        ));
        assert!(is_authentication_error(Some(1), "Authentication error"));
        assert!(!is_authentication_error(
            Some(68),
            "server connection failed: No route to host"
        ));
    }

    #[test]
    fn encodes_domain_and_username_for_smbutil() {
        assert_eq!(encode_username(r"OFFICE\张三"), "OFFICE;%E5%BC%A0%E4%B8%89");
        assert_eq!(encode_username("local user"), "local%20user");
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
    fn rejects_passwords_embedded_in_smb_addresses() {
        let error = parse_location("smb://alice:secret@nas.local/share")
            .expect_err("inline password must be rejected");
        assert!(error.to_string().contains("不能包含密码"));
        assert!(parse_location("smb://alice%3Asecret@nas.local/share").is_err());
        assert!(parse_location("smb://alice@nas.local/share").is_ok());
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

    #[test]
    fn parses_long_utf8_share_names_without_splitting_a_character() {
        let output = format!(
            "Share                                           Type    Comments\n\
             -------------------------------\n\
             {}中 Disk    Archive\n\
             1 share listed\n",
            "a".repeat(47)
        );
        assert_eq!(
            super::parse_smbutil_shares(&output),
            vec![format!("{}中", "a".repeat(47))]
        );
    }

    #[test]
    fn finds_the_type_column_when_a_share_name_contains_the_word_disk() {
        let output = "Share                                           Type    Comments\n\
                      -------------------------------\n\
                      Disk Images                                     Disk    Backups\n\
                      Office Disk                                     Pipe    IPC\n";
        assert_eq!(
            super::parse_smbutil_shares(output),
            vec!["Disk Images".to_string()]
        );
    }
}
