use accesskit::{
    ActionHandler, ActionRequest, ActivationHandler, Node, NodeId, Role, Tree, TreeId, TreeUpdate,
};
use accesskit_macos::SubclassingAdapter;
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{Arc, Mutex};

const ROOT_ID: NodeId = NodeId(0);
const TOOLBAR_ID: NodeId = NodeId(1);
const SIDEBAR_ID: NodeId = NodeId(2);
const PANES_ID: NodeId = NodeId(3);
const STATUS_ID: NodeId = NodeId(4);
const MODAL_ID: NodeId = NodeId(5);
const QUICK_LOOK_ID: NodeId = NodeId(6);
const PREFERENCES_ID: NodeId = NodeId(7);
const PANE_ID_BASE: u64 = 100;
const PANE_ID_STRIDE: u64 = 2_000;
const ACCESSIBLE_ITEM_LIMIT: usize = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessibilitySnapshot {
    pub layout: String,
    pub sidebar_visible: bool,
    pub panes: Vec<PaneSnapshot>,
    pub status: String,
    pub modal: Option<String>,
    pub quick_look: Option<String>,
    pub preferences: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneSnapshot {
    pub path: String,
    pub active: bool,
    pub search: Option<String>,
    pub items: Vec<ItemSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemSnapshot {
    pub name: String,
    pub description: String,
    pub selected: bool,
}

struct InitialTree {
    tree: Arc<Mutex<TreeUpdate>>,
}

impl ActivationHandler for InitialTree {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        Some(self.tree.lock().expect("accessibility tree lock").clone())
    }
}

struct IgnoreActions;

impl ActionHandler for IgnoreActions {
    fn do_action(&mut self, _request: ActionRequest) {}
}

pub struct MacAccessibility {
    adapter: SubclassingAdapter,
    tree: Arc<Mutex<TreeUpdate>>,
    last_snapshot: AccessibilitySnapshot,
}

impl MacAccessibility {
    pub fn new(window: &Window, snapshot: AccessibilitySnapshot) -> Self {
        let handle = HasWindowHandle::window_handle(window)
            .expect("FlowFile window must expose a native macOS view");
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            unreachable!("macOS builds must provide an AppKit window handle");
        };
        let tree_update = build_tree(&snapshot);
        let tree = Arc::new(Mutex::new(tree_update));
        let activation_handler = InitialTree {
            tree: Arc::clone(&tree),
        };
        // SAFETY: GPUI owns this NSView for at least as long as WorkspaceView,
        // which owns the adapter. The constructor runs on AppKit's main thread.
        let adapter = unsafe {
            SubclassingAdapter::new(handle.ns_view.as_ptr(), activation_handler, IgnoreActions)
        };
        Self {
            adapter,
            tree,
            last_snapshot: snapshot,
        }
    }

    pub fn update(&mut self, snapshot: AccessibilitySnapshot) {
        if snapshot == self.last_snapshot {
            return;
        }
        let update = build_tree(&snapshot);
        *self.tree.lock().expect("accessibility tree lock") = update.clone();
        self.last_snapshot = snapshot;
        if let Some(events) = self.adapter.update_if_active(|| update) {
            events.raise();
        }
    }
}

fn build_tree(snapshot: &AccessibilitySnapshot) -> TreeUpdate {
    let mut nodes = Vec::new();
    let mut root_children = vec![TOOLBAR_ID];

    let mut toolbar = Node::new(Role::Toolbar);
    toolbar.set_label("导航工具栏");
    toolbar.set_description(format!("当前布局：{}", snapshot.layout));
    nodes.push((TOOLBAR_ID, toolbar));

    if snapshot.sidebar_visible {
        root_children.push(SIDEBAR_ID);
        let mut sidebar = Node::new(Role::Navigation);
        sidebar.set_label("位置与收藏侧边栏");
        nodes.push((SIDEBAR_ID, sidebar));
    }

    root_children.push(PANES_ID);
    let mut pane_ids = Vec::new();
    for (pane_index, pane) in snapshot.panes.iter().enumerate() {
        let pane_id = NodeId(PANE_ID_BASE + pane_index as u64 * PANE_ID_STRIDE);
        let location_id = NodeId(pane_id.0 + 1);
        let list_id = NodeId(pane_id.0 + 2);
        let truncation_id = NodeId(pane_id.0 + PANE_ID_STRIDE - 1);
        pane_ids.push(pane_id);

        let mut pane_node = Node::new(Role::Pane);
        pane_node.set_label(if pane.active {
            format!("活动文件面板 {}", pane_index + 1)
        } else {
            format!("文件面板 {}", pane_index + 1)
        });
        pane_node.set_children(vec![location_id, list_id]);
        nodes.push((pane_id, pane_node));

        let mut location = Node::new(Role::Label);
        location.set_value(format!("位置：{}", pane.path));
        nodes.push((location_id, location));

        let mut item_ids = Vec::new();
        for (item_index, item) in pane.items.iter().take(ACCESSIBLE_ITEM_LIMIT).enumerate() {
            let item_id = NodeId(pane_id.0 + 10 + item_index as u64);
            item_ids.push(item_id);
            let mut item_node = Node::new(Role::ListItem);
            item_node.set_label(item.name.as_str());
            item_node.set_description(item.description.as_str());
            item_node.set_selected(item.selected);
            nodes.push((item_id, item_node));
        }
        if pane.items.len() > ACCESSIBLE_ITEM_LIMIT {
            item_ids.push(truncation_id);
            let mut truncation = Node::new(Role::Label);
            truncation.set_value(format!(
                "另有 {} 个项目未加入辅助功能树；可通过搜索缩小范围",
                pane.items.len() - ACCESSIBLE_ITEM_LIMIT
            ));
            nodes.push((truncation_id, truncation));
        }

        let mut list = Node::new(Role::List);
        list.set_label(match &pane.search {
            Some(query) => format!("“{query}”的搜索结果，{} 项", pane.items.len()),
            None => format!("文件列表，{} 项", pane.items.len()),
        });
        list.set_multiselectable();
        list.set_children(item_ids);
        nodes.push((list_id, list));
    }

    let mut panes = Node::new(Role::Group);
    panes.set_label(format!("{} 个文件面板", snapshot.panes.len()));
    panes.set_children(pane_ids);
    nodes.push((PANES_ID, panes));

    root_children.push(STATUS_ID);
    let mut status = Node::new(Role::Status);
    status.set_label("状态栏");
    status.set_value(snapshot.status.as_str());
    nodes.push((STATUS_ID, status));

    push_optional_dialog(
        &mut nodes,
        &mut root_children,
        MODAL_ID,
        "操作对话框",
        snapshot.modal.as_deref(),
    );
    push_optional_dialog(
        &mut nodes,
        &mut root_children,
        QUICK_LOOK_ID,
        "Quick Look 预览",
        snapshot.quick_look.as_deref(),
    );
    push_optional_dialog(
        &mut nodes,
        &mut root_children,
        PREFERENCES_ID,
        "FlowFile 设置",
        snapshot.preferences.as_deref(),
    );

    let mut root = Node::new(Role::Window);
    root.set_label("FlowFile 文件管理器");
    root.set_children(root_children);
    nodes.push((ROOT_ID, root));

    let mut tree = Tree::new(ROOT_ID);
    tree.toolkit_name = Some("GPUI + AccessKit".to_string());
    tree.toolkit_version = Some(env!("CARGO_PKG_VERSION").to_string());
    TreeUpdate {
        nodes,
        tree: Some(tree),
        tree_id: TreeId::ROOT,
        focus: ROOT_ID,
    }
}

fn push_optional_dialog(
    nodes: &mut Vec<(NodeId, Node)>,
    root_children: &mut Vec<NodeId>,
    id: NodeId,
    label: &str,
    description: Option<&str>,
) {
    let Some(description) = description else {
        return;
    };
    root_children.push(id);
    let mut dialog = Node::new(Role::Dialog);
    dialog.set_label(label);
    dialog.set_description(description);
    dialog.set_modal();
    nodes.push((id, dialog));
}

#[cfg(test)]
mod tests {
    use super::{AccessibilitySnapshot, ItemSnapshot, PaneSnapshot, ROOT_ID, build_tree};

    #[test]
    fn tree_exposes_file_names_selection_and_dialogs() {
        let snapshot = AccessibilitySnapshot {
            layout: "单面板".to_string(),
            sidebar_visible: true,
            panes: vec![PaneSnapshot {
                path: "/Users/test".to_string(),
                active: true,
                search: None,
                items: vec![ItemSnapshot {
                    name: "文档".to_string(),
                    description: "文件夹".to_string(),
                    selected: true,
                }],
            }],
            status: "1 个项目，已选择 1 个".to_string(),
            modal: None,
            quick_look: None,
            preferences: Some("主题：跟随系统".to_string()),
        };

        let update = build_tree(&snapshot);

        assert_eq!(update.focus, ROOT_ID);
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some("文档"))
        );
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, node)| node.is_selected() == Some(true))
        );
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some("FlowFile 设置"))
        );
    }
}
