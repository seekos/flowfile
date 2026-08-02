use super::FileEngine;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tokio::runtime::Handle;

/// Opens the active FlowFile directory in the macOS system Terminal.
#[derive(Clone)]
pub struct SystemTerminal {
    runtime: Handle,
}

impl SystemTerminal {
    pub fn new(engine: &FileEngine) -> Self {
        Self {
            runtime: engine.runtime_handle(),
        }
    }

    pub fn open(&self, path: PathBuf) {
        self.runtime.spawn_blocking(move || {
            let status = Command::new("/usr/bin/open")
                .args(terminal_open_arguments(&path))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match status {
                Ok(status) if status.success() => {}
                Ok(status) => eprintln!("FlowFile: 系统终端启动失败，open 退出状态：{status}"),
                Err(error) => eprintln!("FlowFile: 无法打开系统终端：{error}"),
            }
        });
    }
}

fn terminal_open_arguments(path: &Path) -> [OsString; 3] {
    [
        OsString::from("-a"),
        OsString::from("Terminal"),
        path.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::terminal_open_arguments;
    use std::{ffi::OsString, path::Path};

    #[test]
    fn system_terminal_arguments_preserve_paths_with_spaces() {
        assert_eq!(
            terminal_open_arguments(Path::new("/tmp/Flow File")),
            [
                OsString::from("-a"),
                OsString::from("Terminal"),
                OsString::from("/tmp/Flow File"),
            ]
        );
    }
}
