use super::tooltip::delayed_tooltip;
use crate::{
    models::{
        Favorites, FileDragPayload, FileOperationController, Model, MultiPaneModel, home_directory,
    },
    services::{FileEngine, FileWatcher, SmbMountInfo, TransferMode, VolumeInfo},
    theme,
};
use gpui::{
    Context, Entity, ExternalPaths, FontWeight, IntoElement, Render, SharedString, Timer, Window,
    div, prelude::*, px,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Eq, PartialEq)]
struct SidebarLocation {
    icon: &'static str,
    label: String,
    path: PathBuf,
    detail: Option<String>,
    navigation_address: Option<String>,
    eject_paths: Vec<PathBuf>,
}

pub struct SidebarView {
    model: Model<MultiPaneModel>,
    operations: Entity<FileOperationController>,
    favorites: Entity<Favorites>,
    quick_access: Vec<SidebarLocation>,
    volumes: Vec<SidebarLocation>,
    volumes_loading: bool,
    engine: FileEngine,
    ntfs_mounting: HashSet<PathBuf>,
    ntfs_mount_failures: HashSet<PathBuf>,
    ejecting_volumes: HashSet<PathBuf>,
    _volumes_watcher: Option<FileWatcher>,
}

impl SidebarView {
    pub fn new(
        model: Model<MultiPaneModel>,
        operations: Entity<FileOperationController>,
        favorites: Entity<Favorites>,
        engine: FileEngine,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&favorites, |_, _, cx| cx.notify()).detach();
        let panes = model.read(cx).panes.clone();
        for pane in &panes {
            cx.observe(pane, |_, _, cx| cx.notify()).detach();
        }

        let home = home_directory();
        let quick_access = [
            ("⌂", "个人文件夹", home.clone()),
            ("↓", "下载", home.join("Downloads")),
            ("▧", "桌面", home.join("Desktop")),
            ("◇", "文稿", home.join("Documents")),
        ]
        .into_iter()
        .filter(|(_, _, path)| path.is_dir())
        .map(|(icon, label, path)| SidebarLocation {
            icon,
            label: label.to_string(),
            path,
            detail: None,
            navigation_address: None,
            eject_paths: Vec::new(),
        })
        .collect();

        let initial_engine = engine.clone();
        cx.spawn(async move |this, cx| {
            let result = initial_engine.list_volumes().await;
            let _ = this.update(cx, |sidebar, cx| {
                sidebar.apply_volumes(result.ok(), cx);
            });
        })
        .detach();

        let (volumes_watcher, volume_events) = FileWatcher::watch(std::path::Path::new("/Volumes"))
            .map(|(watcher, receiver)| (Some(watcher), Some(receiver)))
            .unwrap_or((None, None));
        if let Some(volume_events) = volume_events {
            let watcher_engine = engine.clone();
            cx.spawn(async move |this, cx| {
                while volume_events.recv().await.is_ok() {
                    Timer::after(Duration::from_millis(150)).await;
                    while volume_events.try_recv().is_ok() {}

                    let result = watcher_engine.list_volumes().await;
                    if this
                        .update(cx, |sidebar, cx| sidebar.apply_volumes(result.ok(), cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }

        Self {
            model,
            operations,
            favorites,
            quick_access,
            volumes: Vec::new(),
            volumes_loading: true,
            engine,
            ntfs_mounting: HashSet::new(),
            ntfs_mount_failures: HashSet::new(),
            ejecting_volumes: HashSet::new(),
            _volumes_watcher: volumes_watcher,
        }
    }

    fn apply_volumes(&mut self, paths: Option<Vec<VolumeInfo>>, cx: &mut Context<Self>) {
        self.volumes_loading = false;
        if let Some(paths) = paths {
            let present_paths = paths
                .iter()
                .map(|volume| volume.path.clone())
                .collect::<HashSet<_>>();
            self.ntfs_mount_failures
                .retain(|path| present_paths.contains(path) || self.ntfs_mounting.contains(path));
            let ntfs_to_mount = paths
                .iter()
                .find(|volume| {
                    volume.path.starts_with(Path::new("/Volumes"))
                        && volume.is_ntfs()
                        && volume.read_only
                        && !self.ntfs_mounting.contains(&volume.path)
                        && !self.ntfs_mount_failures.contains(&volume.path)
                })
                .map(|volume| volume.path.clone());
            let volumes = sidebar_locations_for_volumes(paths, FileEngine::mounted_smb_for_path);
            if self.volumes != volumes {
                self.volumes = volumes;
                cx.notify();
            }
            if let Some(path) = ntfs_to_mount
                && self.engine.ntfs_auto_mount_available()
            {
                self.start_ntfs_auto_mount(path, cx);
            }
        }
    }

    fn start_ntfs_auto_mount(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.ntfs_mounting.insert(path.clone());
        self.operations.update(cx, |operations, cx| {
            operations.show_notice(
                format!("正在将 {} 挂载为可写…", volume_label(&path)),
                false,
                cx,
            );
        });

        let engine = self.engine.clone();
        let operations = self.operations.clone();
        cx.spawn(async move |this, cx| {
            let result = engine.auto_mount_ntfs(path.clone()).await;
            let refreshed_volumes = engine.list_volumes().await.ok();
            let _ = this.update(cx, |sidebar, cx| {
                sidebar.ntfs_mounting.remove(&path);
                match result {
                    Ok(true) => operations.update(cx, |operations, cx| {
                        operations.show_notice(
                            format!("{} 已自动挂载为 NTFS 可写", volume_label(&path)),
                            false,
                            cx,
                        );
                    }),
                    Ok(false) => {
                        sidebar.ntfs_mount_failures.insert(path.clone());
                    }
                    Err(error) => {
                        sidebar.ntfs_mount_failures.insert(path.clone());
                        operations.update(cx, |operations, cx| {
                            operations.show_notice(error.to_string(), true, cx);
                        });
                    }
                }
                sidebar.apply_volumes(refreshed_volumes, cx);
            });
        })
        .detach();
    }

    fn section_title(title: &'static str) -> impl IntoElement {
        div()
            .px_3()
            .pt_4()
            .pb_2()
            .text_size(theme::font(10.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::text_tertiary())
            .child(title)
    }

    fn eject_location(&mut self, location: SidebarLocation, cx: &mut Context<Self>) {
        let identity = location.path.clone();
        if !self.ejecting_volumes.insert(identity.clone()) {
            return;
        }
        let label = location.label.clone();
        let paths = location.eject_paths.clone();
        let navigation_address = location.navigation_address.clone();
        self.operations.update(cx, |operations, cx| {
            operations.show_notice(format!("正在弹出 {label}…"), false, cx);
        });
        cx.notify();

        let engine = self.engine.clone();
        let operations = self.operations.clone();
        cx.spawn(async move |this, cx| {
            let mut ejected_paths = Vec::new();
            let mut first_error = None;
            for path in paths {
                match engine.eject_volume(path.clone()).await {
                    Ok(()) => ejected_paths.push(path),
                    Err(error) if first_error.is_none() => first_error = Some(error),
                    Err(_) => {}
                }
            }
            let _ = this.update(cx, |sidebar, cx| {
                sidebar.ejecting_volumes.remove(&identity);
                let disconnect_server = navigation_address.is_some() && first_error.is_none();
                if !ejected_paths.is_empty() {
                    let fallback = home_directory();
                    let panes = sidebar.model.read(cx).panes.clone();
                    for pane in panes {
                        let pane_state = pane.read(cx);
                        let active_ejected_mount = ejected_paths
                            .iter()
                            .find(|path| pane_state.current_path.starts_with(path));
                        let pane_server = smb_server_address(&pane_state.display_path());
                        let ejecting_server = disconnect_server
                            && navigation_address
                                .as_ref()
                                .is_some_and(|address| pane_server.as_ref() == Some(address));
                        let server_address = if navigation_address.is_none() {
                            active_ejected_mount
                                .and_then(|path| pane_state.smb_server_for_mount(path))
                        } else {
                            None
                        };
                        let should_leave_mount = active_ejected_mount.is_some() || ejecting_server;
                        if should_leave_mount {
                            pane.update(cx, |pane, cx| {
                                if let Some(server_address) = server_address.clone() {
                                    pane.navigate_to_address(server_address, cx);
                                } else {
                                    pane.navigate_to(fallback.clone(), cx);
                                }
                            });
                        }
                    }
                }
                match first_error {
                    None => {
                        operations.update(cx, |operations, cx| {
                            operations.show_notice(format!("已弹出 {label}"), false, cx);
                        });
                        sidebar.volumes.retain(|volume| volume.path != identity);
                    }
                    Some(error) => operations.update(cx, |operations, cx| {
                        operations.show_notice(error.to_string(), true, cx);
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remove_favorite(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let label = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("文件夹")
            .to_string();
        let result = self.favorites.update(cx, |favorites, cx| {
            let result = favorites.remove(&path);
            cx.notify();
            result
        });

        match result {
            Ok(true) => self.operations.update(cx, |operations, cx| {
                operations.show_notice(format!("已取消收藏 {label}"), false, cx);
            }),
            Ok(false) => {}
            Err(error) => self.operations.update(cx, |operations, cx| {
                operations.show_notice(error.to_string(), true, cx);
            }),
        }
    }

    fn item(
        &self,
        id: usize,
        location: SidebarLocation,
        is_active: bool,
        can_remove_favorite: bool,
        can_eject: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let model = self.model.clone();
        let operations = self.operations.clone();
        let path = location.path.clone();
        let navigation_address = location.navigation_address.clone();
        let accepts_drop = navigation_address.is_none();
        let drop_path = path.clone();
        let external_drop_path = path.clone();
        let external_operations = self.operations.clone();
        let tooltip = if let Some(address) = &navigation_address {
            format!("在当前面板中打开 {address}")
        } else {
            format!("在当前面板中打开 {}", path.display())
        };
        let label_text = location.label.clone();
        let label: SharedString = label_text.clone().into();
        let detail = location.detail.clone().map(SharedString::from);
        let favorite_path = location.path.clone();
        let eject_identity = location.path.clone();
        let eject_location = location.clone();
        let is_ejecting = self.ejecting_volumes.contains(&eject_identity);
        let hover_group: SharedString = format!("sidebar-location-{id}").into();

        div()
            .id(("sidebar-location", id))
            .group(hover_group.clone())
            .flex()
            .items_center()
            .gap_2()
            .mx_2()
            .h(px(36.0))
            .px_2()
            .rounded_md()
            .text_size(theme::font(12.0))
            .text_color(if is_active {
                theme::accent()
            } else {
                theme::text_primary()
            })
            .bg(if is_active {
                theme::accent_soft()
            } else {
                theme::sidebar()
            })
            .hover(|style| style.bg(theme::surface().opacity(0.8)))
            .tooltip(delayed_tooltip(tooltip))
            .when(accepts_drop, |item| {
                item.drag_over::<FileDragPayload>(|style, _, _, _| {
                    style.bg(theme::accent_soft()).text_color(theme::accent())
                })
                .on_drop(move |payload: &FileDragPayload, window, cx| {
                    let mode = if window.modifiers().alt {
                        TransferMode::Copy
                    } else {
                        TransferMode::Move
                    };
                    operations.update(cx, |operations, cx| {
                        operations.transfer_to_path(
                            payload.paths.clone(),
                            drop_path.clone(),
                            mode,
                            cx,
                        );
                    });
                })
                .drag_over::<ExternalPaths>(|style, _, _, _| {
                    style.bg(theme::accent_soft()).text_color(theme::accent())
                })
                .on_drop(move |payload: &ExternalPaths, window, cx| {
                    let paths = payload.paths().to_vec();
                    if paths.iter().all(|path| {
                        path == &external_drop_path
                            || path.parent() == Some(external_drop_path.as_path())
                    }) {
                        return;
                    }
                    let mode = if window.modifiers().alt {
                        TransferMode::Copy
                    } else {
                        TransferMode::Move
                    };
                    external_operations.update(cx, |operations, cx| {
                        operations.transfer_to_path(paths, external_drop_path.clone(), mode, cx);
                    });
                })
            })
            .on_click(move |_, _, cx| {
                let pane = {
                    let model = model.read(cx);
                    model.panes[model.active_pane_index].clone()
                };
                pane.update(cx, |pane, cx| {
                    if let Some(address) = navigation_address.clone() {
                        pane.navigate_to_address(address, cx);
                    } else {
                        pane.navigate_to(path.clone(), cx);
                    }
                });
            })
            .child(
                div()
                    .w(px(24.0))
                    .text_size(theme::font(14.0))
                    .text_color(if is_active {
                        theme::accent()
                    } else {
                        theme::file_blue()
                    })
                    .child(location.icon),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(div().truncate().child(label))
                    .when_some(detail, |location, detail| {
                        location
                            .text_size(theme::font(8.0))
                            .text_color(theme::text_tertiary())
                            .child(detail)
                    }),
            )
            .when(can_remove_favorite, |item| {
                item.child(
                    div()
                        .id(("sidebar-remove-favorite", id))
                        .w(px(24.0))
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .invisible()
                        .group_hover(hover_group, |style| style.visible())
                        .text_size(theme::font(14.0))
                        .text_color(theme::text_tertiary())
                        .hover(|style| style.bg(theme::surface()).text_color(theme::danger()))
                        .tooltip(delayed_tooltip("取消收藏".to_string()))
                        .child("×")
                        .on_click(cx.listener(move |sidebar, _, _, cx| {
                            cx.stop_propagation();
                            sidebar.remove_favorite(favorite_path.clone(), cx);
                        })),
                )
            })
            .when(can_eject, |item| {
                item.child(
                    div()
                        .id(("sidebar-eject", id))
                        .w(px(24.0))
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(theme::font(12.0))
                        .text_color(theme::text_tertiary())
                        .hover(|style| style.bg(theme::surface()).text_color(theme::text_primary()))
                        .tooltip(delayed_tooltip(if is_ejecting {
                            "正在弹出…".to_string()
                        } else {
                            format!("弹出 {label_text}")
                        }))
                        .child(if is_ejecting { "…" } else { "⏏" })
                        .on_click(cx.listener(move |sidebar, _, _, cx| {
                            cx.stop_propagation();
                            sidebar.eject_location(eject_location.clone(), cx);
                        })),
                )
            })
    }
}

impl Render for SidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (current_path, current_smb_server, connected_smb_servers) = {
            let model = self.model.read(cx);
            let pane = model.panes[model.active_pane_index].read(cx);
            let connected_smb_servers = model
                .panes
                .iter()
                .filter_map(|pane| smb_server_address(&pane.read(cx).display_path()))
                .collect::<BTreeSet<_>>();
            (
                pane.current_path.clone(),
                smb_server_address(&pane.display_path()),
                connected_smb_servers,
            )
        };
        let quick_access = self.quick_access.clone();
        let favorites = self
            .favorites
            .read(cx)
            .paths()
            .iter()
            .filter(|path| path.is_dir())
            .map(|path| SidebarLocation {
                icon: "★",
                label: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("收藏文件夹")
                    .to_string(),
                path: path.clone(),
                detail: None,
                navigation_address: None,
                eject_paths: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut volumes = self.volumes.clone();
        for address in connected_smb_servers {
            if volumes
                .iter()
                .any(|location| location.navigation_address.as_ref() == Some(&address))
            {
                continue;
            }
            volumes.push(SidebarLocation {
                icon: "◉",
                label: smb_server_label(&address),
                path: PathBuf::from(&address),
                detail: Some("SMB · 网络服务器".to_string()),
                navigation_address: Some(address),
                eject_paths: Vec::new(),
            });
        }

        div()
            .flex()
            .flex_col()
            .w(px(205.0))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::sidebar())
            .child(Self::section_title("快速访问"))
            .children(
                quick_access
                    .into_iter()
                    .enumerate()
                    .map(|(index, location)| {
                        let is_active = current_path == location.path;
                        self.item(index, location, is_active, false, false, cx)
                    }),
            )
            .child(Self::section_title("收藏夹"))
            .when(favorites.is_empty(), |sidebar| {
                sidebar.child(
                    div()
                        .mx_3()
                        .pb_1()
                        .text_size(theme::font(10.0))
                        .text_color(theme::text_tertiary())
                        .child("暂无收藏的文件夹"),
                )
            })
            .children(favorites.into_iter().enumerate().map(|(index, location)| {
                let is_active = current_path == location.path;
                self.item(50 + index, location, is_active, true, false, cx)
            }))
            .child(Self::section_title("位置"))
            .when(self.volumes_loading, |sidebar| {
                sidebar.child(
                    div()
                        .mx_3()
                        .text_size(theme::font(10.0))
                        .text_color(theme::text_tertiary())
                        .child("正在读取挂载卷…"),
                )
            })
            .children(volumes.into_iter().enumerate().map(|(index, location)| {
                let is_active = location
                    .navigation_address
                    .as_ref()
                    .is_some_and(|address| current_smb_server.as_ref() == Some(address))
                    || current_path == location.path;
                let can_eject = !location.eject_paths.is_empty();
                self.item(100 + index, location, is_active, false, can_eject, cx)
            }))
            .child(
                div()
                    .mt_auto()
                    .mx_3()
                    .mb_3()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme::border_strong())
                    .text_size(theme::font(9.0))
                    .text_color(theme::text_tertiary())
                    .child("FlowFile · macOS local filesystem"),
            )
    }
}

fn sidebar_locations_for_volumes(
    volumes: Vec<VolumeInfo>,
    mut mounted_smb_for_path: impl FnMut(&Path) -> Option<SmbMountInfo>,
) -> Vec<SidebarLocation> {
    let mut locations = Vec::new();
    let mut smb_servers = BTreeMap::<String, Vec<PathBuf>>::new();

    for volume in volumes {
        if volume.is_smb()
            && let Some(mount) = mounted_smb_for_path(&volume.path)
        {
            smb_servers
                .entry(mount.server_address)
                .or_default()
                .push(volume.path);
            continue;
        }

        let eject_paths = if volume.path.starts_with(Path::new("/Volumes")) {
            vec![volume.path.clone()]
        } else {
            Vec::new()
        };
        locations.push(SidebarLocation {
            icon: "◉",
            label: volume_label(&volume.path),
            detail: volume.status_label().map(str::to_string),
            path: volume.path,
            navigation_address: None,
            eject_paths,
        });
    }

    locations.extend(smb_servers.into_iter().map(|(address, mut mount_paths)| {
        mount_paths.sort();
        mount_paths.dedup();
        SidebarLocation {
            icon: "◉",
            label: smb_server_label(&address),
            path: PathBuf::from(&address),
            detail: Some("SMB · 网络服务器".to_string()),
            navigation_address: Some(address),
            eject_paths: mount_paths,
        }
    }));
    locations
}

fn smb_server_address(address: &str) -> Option<String> {
    let remainder = address.strip_prefix("smb://")?;
    let authority = remainder.split('/').next()?;
    let server = authority
        .rsplit_once('@')
        .map_or(authority, |(_, server)| server);
    (!server.is_empty()).then(|| format!("smb://{server}"))
}

fn smb_server_label(address: &str) -> String {
    address
        .strip_prefix("smb://")
        .unwrap_or(address)
        .rsplit_once('@')
        .map_or_else(
            || address.strip_prefix("smb://").unwrap_or(address),
            |(_, server)| server,
        )
        .to_string()
}

fn volume_label(path: &std::path::Path) -> String {
    if path == std::path::Path::new("/") {
        return "Macintosh HD".to_string();
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Volume")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{sidebar_locations_for_volumes, smb_server_address};
    use crate::services::{SmbMountInfo, VolumeInfo};
    use std::path::{Path, PathBuf};

    #[test]
    fn smb_shares_from_the_same_server_become_one_sidebar_location() {
        let volumes = vec![
            VolumeInfo {
                path: PathBuf::from("/"),
                filesystem: "apfs".to_string(),
                read_only: false,
            },
            VolumeInfo {
                path: PathBuf::from("/Volumes/Design"),
                filesystem: "smbfs".to_string(),
                read_only: false,
            },
            VolumeInfo {
                path: PathBuf::from("/Volumes/Media"),
                filesystem: "smbfs".to_string(),
                read_only: false,
            },
        ];

        let locations = sidebar_locations_for_volumes(volumes, |path| {
            let share_name = path.file_name()?.to_str()?.to_string();
            Some(SmbMountInfo {
                server_address: "smb://nas.local".to_string(),
                share_name,
                mount_path: path.to_path_buf(),
            })
        });

        assert_eq!(locations.len(), 2);
        assert_eq!(locations[1].label, "nas.local");
        assert_eq!(
            locations[1].navigation_address.as_deref(),
            Some("smb://nas.local")
        );
        assert_eq!(
            locations[1].eject_paths,
            [Path::new("/Volumes/Design"), Path::new("/Volumes/Media")]
        );
    }

    #[test]
    fn active_smb_path_resolves_to_its_server_without_user_or_share() {
        assert_eq!(
            smb_server_address("smb://office;alice@nas.local/Media/Movies").as_deref(),
            Some("smb://nas.local")
        );
        assert_eq!(smb_server_address("/Users/zy"), None);
    }
}
