use super::FileEngine;
use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::process::Command;
use tokio::runtime::Handle;

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/seekos/flowfile/releases/latest";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableUpdate {
    pub current_version: String,
    pub version: String,
    pub release_url: String,
}

#[derive(Clone)]
pub struct UpdateChecker {
    runtime: Handle,
    current_version: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

impl UpdateChecker {
    pub fn new(engine: &FileEngine, current_version: String) -> Self {
        Self {
            runtime: engine.runtime_handle(),
            current_version,
        }
    }

    pub async fn check(&self) -> Result<Option<AvailableUpdate>> {
        let current_version = self.current_version.clone();
        self.runtime
            .spawn_blocking(move || check_latest_release(&current_version))
            .await
            .context("版本检查任务异常终止")?
    }
}

fn check_latest_release(current_version: &str) -> Result<Option<AvailableUpdate>> {
    let output = Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "8",
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            "X-GitHub-Api-Version: 2022-11-28",
            "--user-agent",
            concat!("FlowFile/", env!("CARGO_PKG_VERSION")),
            LATEST_RELEASE_URL,
        ])
        .output()
        .context("无法启动版本检查")?;
    if !output.status.success() {
        anyhow::bail!("无法读取最新版本");
    }

    let release: GithubRelease =
        serde_json::from_slice(&output.stdout).context("无法解析最新版本信息")?;
    let version = release.tag_name.trim_start_matches(['v', 'V']).to_string();
    if !is_newer_version(&version, current_version) {
        return Ok(None);
    }

    Ok(Some(AvailableUpdate {
        current_version: current_version.to_string(),
        version,
        release_url: release.html_url,
    }))
}

fn is_newer_version(candidate: &str, current: &str) -> bool {
    let parse = |version: &str| {
        let version = version.trim_start_matches(['v', 'V']);
        let (core, is_prerelease) = version
            .split_once('-')
            .map(|(core, _)| (core, true))
            .unwrap_or((version, false));
        let components = core
            .split('.')
            .map(|component| component.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()?;
        Some((components, is_prerelease))
    };
    let (candidate_core, candidate_prerelease) = match parse(candidate) {
        Some(version) => version,
        None => return false,
    };
    let (current_core, current_prerelease) = match parse(current) {
        Some(version) => version,
        None => return false,
    };

    let component_count = candidate_core.len().max(current_core.len());
    for index in 0..component_count {
        let candidate_component = candidate_core.get(index).copied().unwrap_or(0);
        let current_component = current_core.get(index).copied().unwrap_or(0);
        if candidate_component != current_component {
            return candidate_component > current_component;
        }
    }
    !candidate_prerelease && current_prerelease
}

#[cfg(test)]
mod tests {
    use super::is_newer_version;

    #[test]
    fn compares_release_versions() {
        assert!(is_newer_version("0.2.0", "0.1.9"));
        assert!(is_newer_version("v1.0.0", "1.0.0-beta.1"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.1.0-beta.1", "0.1.0"));
        assert!(!is_newer_version("not-a-version", "0.1.0"));
    }
}
