use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
};

use gpui::{
    Anchor, App, Context, Entity, Pixels, ScrollHandle, SharedString, TaskExt, Window,
    WindowControlArea, px,
};
use project::ProjectGroupKey;
use ui::{
    ContextMenu, ContextMenuEntry, IconButtonShape, Indicator, PopoverMenu, Tooltip, WithScrollbar,
    prelude::*, utils::platform_title_bar_height,
};

use crate::{MultiWorkspace, Workspace};

#[derive(Clone)]
struct DraggedWorkspaceTab {
    ix: usize,
    workspace: Entity<Workspace>,
    label: SharedString,
    is_active: bool,
}

impl MultiWorkspace {
    pub(crate) fn cycle_workspace_tab(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspaces = self.ordered_workspaces(cx);
        if workspaces.len() < 2 {
            return;
        }

        let active_index = workspaces
            .iter()
            .position(|workspace| workspace == self.workspace())
            .unwrap_or(0);
        let next_index = if forward {
            (active_index + 1) % workspaces.len()
        } else if active_index == 0 {
            workspaces.len() - 1
        } else {
            active_index - 1
        };

        self.activate(workspaces[next_index].clone(), None, window, cx);
    }

    pub(crate) fn render_workspace_tabs(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.retention_enabled(cx) {
            return None;
        }
        if self.sidebar_ui_enabled(cx) && self.sidebar_open() {
            return None;
        }

        let workspaces = self.ordered_workspaces(cx);
        if workspaces.len() < 2 {
            return None;
        }

        let active_workspace = self.workspace().clone();
        let active_workspace_index = workspaces
            .iter()
            .position(|workspace| workspace == &active_workspace)
            .unwrap_or(0);
        let active_workspace_id = active_workspace.entity_id();
        if self.workspace_tabs_last_scrolled_workspace_id.get() != Some(active_workspace_id)
            || self.workspace_tabs_last_scrolled_index.get() != Some(active_workspace_index)
        {
            self.workspace_tabs_scroll_handle
                .scroll_to_item(active_workspace_index);
            self.workspace_tabs_last_scrolled_workspace_id
                .set(Some(active_workspace_id));
            self.workspace_tabs_last_scrolled_index
                .set(Some(active_workspace_index));
        }

        let workspace_paths = workspaces
            .iter()
            .map(|workspace| workspace_tab_paths(workspace.read(cx), cx))
            .collect::<Vec<_>>();
        let workspace_has_unsaved_changes = workspaces
            .iter()
            .map(|workspace| workspace_has_unsaved_changes(workspace.read(cx), cx))
            .collect::<Vec<_>>();
        let path_detail_map = workspace_tab_path_detail_map(&workspace_paths);
        let title_bar_fill_height = if cfg!(target_os = "macos") && !window.is_fullscreen() {
            Some(platform_title_bar_height(window))
        } else {
            None
        };
        let top_padding = if let Some(title_bar_fill_height) = title_bar_fill_height {
            title_bar_fill_height
        } else {
            px(4.)
        };
        let overflow_edges = workspace_tab_overflow_edges(&self.workspace_tabs_scroll_handle);

        Some(
            div()
                .id("workspace-tab-strip")
                .relative()
                .h_full()
                .w(px(168.))
                .flex_shrink_0()
                .bg(cx.theme().colors().panel_background)
                .border_r_1()
                .border_color(cx.theme().colors().border)
                .vertical_scrollbar_for(&self.workspace_tabs_scroll_handle, window, cx)
                .when_some(title_bar_fill_height, |this, title_bar_fill_height| {
                    this.child(
                        div()
                            .id("workspace-tab-title-bar-fill")
                            .debug_selector(|| "WORKSPACE-TAB-TITLE-BAR-FILL".to_string())
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(title_bar_fill_height)
                            .bg(cx.theme().colors().title_bar_background)
                            .window_control_area(WindowControlArea::Drag)
                            .on_click(|event, window, _| {
                                if event.click_count() == 2 {
                                    window.titlebar_double_click();
                                }
                            }),
                    )
                })
                .child(
                    v_flex()
                        .id("workspace-tab-strip-scroll")
                        .h_full()
                        .w_full()
                        .pt(top_padding)
                        .pb_1()
                        .overflow_y_scroll()
                        .track_scroll(&self.workspace_tabs_scroll_handle)
                        .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                        .child(workspace_tab_heading())
                        .children(
                            workspaces
                                .into_iter()
                                .zip(workspace_paths)
                                .zip(workspace_has_unsaved_changes)
                                .enumerate()
                                .map(|(ix, ((workspace, paths), has_unsaved_changes))| {
                                    let label = workspace_tab_label(&paths, &path_detail_map);
                                    let tooltip = workspace_tab_tooltip(&paths);
                                    let is_active = workspace == active_workspace;
                                    let project_group_key =
                                        self.project_group_key_for_workspace(&workspace, cx);
                                    let dragged_tab = DraggedWorkspaceTab {
                                        ix,
                                        workspace: workspace.clone(),
                                        label: label.clone(),
                                        is_active,
                                    };
                                    let workspace = workspace.clone();
                                    let close_workspace = workspace.clone();

                                    h_flex()
                                        .id(ix)
                                        .debug_selector(|| format!("WORKSPACE-TAB-{ix}"))
                                        .h(px(32.))
                                        .flex_shrink_0()
                                        .w_full()
                                        .min_w_0()
                                        .px_2()
                                        .gap_1()
                                        .border_l_2()
                                        .border_color(if is_active {
                                            cx.theme().colors().border_focused
                                        } else {
                                            cx.theme().colors().border.opacity(0.)
                                        })
                                        .cursor_pointer()
                                        .overflow_hidden()
                                        .hover(|this| {
                                            this.bg(cx.theme().colors().ghost_element_hover)
                                        })
                                        .when(is_active, |this| {
                                            this.bg(cx.theme().colors().ghost_element_selected)
                                        })
                                        .tooltip(Tooltip::text(tooltip))
                                        .on_drag(dragged_tab, |tab, _, _, cx| {
                                            cx.new(|_| tab.clone())
                                        })
                                        .drag_over::<DraggedWorkspaceTab>({
                                            let tab_ix = ix;
                                            move |element, dragged_tab, _, cx| {
                                                let element = element
                                                    .bg(cx.theme().colors().drop_target_background)
                                                    .border_color(
                                                        cx.theme().colors().drop_target_border,
                                                    )
                                                    .border_0();

                                                if tab_ix < dragged_tab.ix {
                                                    element.border_t_2()
                                                } else if tab_ix > dragged_tab.ix {
                                                    element.border_b_2()
                                                } else {
                                                    element
                                                }
                                            }
                                        })
                                        .on_drop({
                                            let tab_ix = ix;
                                            cx.listener(
                                                move |this, dragged_tab: &DraggedWorkspaceTab, _, cx| {
                                                    this.move_workspace_tab_to_index(
                                                        &dragged_tab.workspace,
                                                        tab_ix,
                                                        cx,
                                                    );
                                                },
                                            )
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.activate(workspace.clone(), None, window, cx);
                                        }))
                                        .child(
                                            Label::new(label)
                                                .size(LabelSize::Small)
                                                .color(if is_active {
                                                    Color::Default
                                                } else {
                                                    Color::Muted
                                                })
                                                .flex_1()
                                                .truncate(),
                                        )
                                        .when(has_unsaved_changes, |this| {
                                            this.child(
                                                div()
                                                    .id(("workspace-tab-unsaved", ix))
                                                    .debug_selector(|| {
                                                        format!("WORKSPACE-TAB-UNSAVED-{ix}")
                                                    })
                                                    .tooltip(Tooltip::text("Unsaved Changes"))
                                                    .child(Indicator::dot().color(Color::Accent)),
                                            )
                                        })
                                        .child(workspace_tab_actions_menu(
                                            ix,
                                            project_group_key,
                                            cx,
                                        ))
                                        .child(
                                            div()
                                                .debug_selector(|| {
                                                    format!("WORKSPACE-TAB-CLOSE-{ix}")
                                                })
                                                .child(
                                                    IconButton::new(
                                                        ("close-workspace-tab", ix),
                                                        IconName::Close,
                                                    )
                                                    .shape(IconButtonShape::Square)
                                                    .icon_color(Color::Muted)
                                                    .size(ButtonSize::None)
                                                    .icon_size(IconSize::Small)
                                                    .tooltip(Tooltip::text("Close Workspace"))
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            cx.stop_propagation();
                                                            window.prevent_default();
                                                            this.close_workspace(
                                                                &close_workspace,
                                                                window,
                                                                cx,
                                                            )
                                                            .detach_and_log_err(cx);
                                                        },
                                                    )),
                                                ),
                                        )
                                        .into_any_element()
                                }),
                        ),
                )
                .when(overflow_edges.above, |this| {
                    this.child(workspace_tab_overflow_cue(
                        "workspace-tab-overflow-above",
                        IconName::ChevronUp,
                        "More Workspace Tabs Above",
                        Some(top_padding),
                        cx,
                    ))
                })
                .when(overflow_edges.below, |this| {
                    this.child(workspace_tab_overflow_cue(
                        "workspace-tab-overflow-below",
                        IconName::ChevronDown,
                        "More Workspace Tabs Below",
                        None,
                        cx,
                    ))
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
impl MultiWorkspace {
    pub(crate) fn test_workspace_tab_labels(&self, cx: &App) -> Vec<String> {
        let workspace_paths = self
            .ordered_workspaces(cx)
            .iter()
            .map(|workspace| workspace_tab_paths(workspace.read(cx), cx))
            .collect::<Vec<_>>();
        let path_detail_map = workspace_tab_path_detail_map(&workspace_paths);

        workspace_paths
            .iter()
            .map(|paths| workspace_tab_label(paths, &path_detail_map).to_string())
            .collect()
    }

    pub(crate) fn test_workspace_tab_unsaved_states(&self, cx: &App) -> Vec<(String, bool)> {
        let workspace_paths = self
            .ordered_workspaces(cx)
            .iter()
            .map(|workspace| workspace_tab_paths(workspace.read(cx), cx))
            .collect::<Vec<_>>();
        let path_detail_map = workspace_tab_path_detail_map(&workspace_paths);

        self.ordered_workspaces(cx)
            .iter()
            .zip(workspace_paths.iter())
            .map(|(workspace, paths)| {
                (
                    workspace_tab_label(paths, &path_detail_map).to_string(),
                    workspace_has_unsaved_changes(workspace.read(cx), cx),
                )
            })
            .collect()
    }
}

fn workspace_tab_heading() -> AnyElement {
    h_flex()
        .debug_selector(|| "WORKSPACE-TABS-HEADING".to_string())
        .h(px(24.))
        .flex_shrink_0()
        .w_full()
        .px_2()
        .items_center()
        .child(
            Label::new("Workspaces")
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
        .into_any_element()
}

fn workspace_tab_actions_menu(
    ix: usize,
    project_group_key: ProjectGroupKey,
    cx: &Context<MultiWorkspace>,
) -> AnyElement {
    let open_in_new_window_enabled = project_group_key.host().is_none();
    let multi_workspace = cx.weak_entity();

    div()
        .id(("workspace-tab-menu", ix))
        .debug_selector(|| format!("WORKSPACE-TAB-MENU-{ix}"))
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(
            PopoverMenu::new(format!("workspace-tab-menu-{ix}"))
                .trigger_with_tooltip(
                    IconButton::new(("workspace-tab-menu-trigger", ix), IconName::Ellipsis)
                        .shape(IconButtonShape::Square)
                        .icon_color(Color::Muted)
                        .size(ButtonSize::None)
                        .icon_size(IconSize::Small),
                    Tooltip::text("Workspace Tab Actions"),
                )
                .anchor(Anchor::TopRight)
                .menu(move |window, cx| {
                    let new_window_workspace = multi_workspace.clone();
                    let new_window_key = project_group_key.clone();

                    Some(ContextMenu::build(window, cx, move |menu, _, _| {
                        menu.item(
                            ContextMenuEntry::new("Open in New Window")
                                .icon(IconName::ArrowUpRight)
                                .disabled(!open_in_new_window_enabled)
                                .handler({
                                    let new_window_workspace = new_window_workspace.clone();
                                    let new_window_key = new_window_key.clone();
                                    move |window, cx| {
                                        new_window_workspace
                                            .update(cx, |multi_workspace, cx| {
                                                multi_workspace
                                                    .open_project_group_in_new_window(
                                                        &new_window_key,
                                                        window,
                                                        cx,
                                                    )
                                                    .detach_and_log_err(cx);
                                            })
                                            .ok();
                                    }
                                }),
                        )
                    }))
                }),
        )
        .into_any_element()
}

impl Render for DraggedWorkspaceTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(px(32.))
            .w(px(168.))
            .px_2()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().panel_background)
            .child(
                Label::new(self.label.clone())
                    .size(LabelSize::Small)
                    .color(if self.is_active {
                        Color::Default
                    } else {
                        Color::Muted
                    })
                    .truncate(),
            )
    }
}

fn workspace_tab_paths(workspace: &Workspace, cx: &App) -> Vec<PathBuf> {
    workspace
        .root_paths(cx)
        .into_iter()
        .map(|path| path.as_ref().to_path_buf())
        .collect()
}

fn workspace_tab_path_detail_map(workspace_paths: &[Vec<PathBuf>]) -> HashMap<PathBuf, usize> {
    let mut paths = workspace_paths
        .iter()
        .flat_map(|paths| paths.iter().cloned())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();

    let path_details =
        util::disambiguate::compute_disambiguation_details(&paths, |path, detail| {
            let display_path = workspace_tab_display_path(path);
            project::path_suffix(display_path.as_ref(), detail)
        });
    paths.into_iter().zip(path_details).collect()
}

fn workspace_tab_label(
    paths: &[PathBuf],
    path_detail_map: &HashMap<PathBuf, usize>,
) -> SharedString {
    let mut names = Vec::with_capacity(paths.len());
    for path in paths {
        let detail = path_detail_map.get(path).copied().unwrap_or(0);
        let display_path = workspace_tab_display_path(path);
        let suffix = project::path_suffix(display_path.as_ref(), detail);
        if !suffix.is_empty() {
            names.push(suffix);
        }
    }
    if names.is_empty() {
        "Empty Workspace".into()
    } else {
        names.join(", ").into()
    }
}

fn workspace_has_unsaved_changes(workspace: &Workspace, cx: &App) -> bool {
    workspace.items(cx).any(|item| item.is_dirty(cx))
}

fn workspace_tab_display_path(path: &Path) -> Cow<'_, Path> {
    if path.extension() == Some(std::ffi::OsStr::new("git")) {
        Cow::Owned(path.with_extension(""))
    } else {
        Cow::Borrowed(path)
    }
}

struct WorkspaceTabOverflowEdges {
    above: bool,
    below: bool,
}

fn workspace_tab_overflow_edges(scroll_handle: &ScrollHandle) -> WorkspaceTabOverflowEdges {
    let max_offset = scroll_handle.max_offset().y;
    let offset = scroll_handle.offset().y;
    let scrollable = max_offset > px(2.);

    WorkspaceTabOverflowEdges {
        above: scrollable && offset < px(-2.),
        below: scrollable && offset > -max_offset + px(2.),
    }
}

fn workspace_tab_overflow_cue(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    top: Option<Pixels>,
    cx: &App,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .absolute()
        .left_0()
        .right_0()
        .h(px(16.))
        .when_some(top, |this, top| this.top(top))
        .when(top.is_none(), |this| this.bottom_0())
        .items_center()
        .justify_center()
        .bg(cx.theme().colors().panel_background.opacity(0.92))
        .tooltip(Tooltip::text(tooltip))
        .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Muted))
        .into_any_element()
}

fn workspace_tab_tooltip(paths: &[PathBuf]) -> SharedString {
    let paths = paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        "Empty Workspace".into()
    } else {
        paths.join("\n").into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_tab_label_uses_opened_folder_leaf() {
        let skills = PathBuf::from("/repo/dotfiles/rb-agents/skills");
        let prompts = PathBuf::from("/repo/dotfiles/rb-agents/prompts");
        let path_detail_map = workspace_tab_path_detail_map(&[vec![skills.clone()], vec![prompts]]);

        assert_eq!(
            workspace_tab_label(&[skills], &path_detail_map).to_string(),
            "skills"
        );
    }
}
