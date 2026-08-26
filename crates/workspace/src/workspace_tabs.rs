use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
};

use gpui::{
    Action, Anchor, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    Pixels, PromptLevel, ScrollHandle, SharedString, TaskExt, WeakEntity, Window,
    WindowControlArea, px, rems,
};
use project::ProjectGroupKey;
use ui::{
    Button, ButtonLike, ButtonStyle, ContextMenu, ContextMenuEntry, IconButtonShape, Indicator,
    PopoverMenu, Tooltip, WithScrollbar, prelude::*, utils::platform_title_bar_height,
};
use ui_input::InputField;
use util::ResultExt as _;

use crate::{
    ModalView, MultiWorkspace, SaveWorkspaceConfigurationAs, Workspace,
    multi_workspace::{
        ManageWorkspaceConfigurations, SelectNextWorkspaceConfiguration,
        SelectPreviousWorkspaceConfiguration,
    },
    persistence::{
        StoreBlock, WorkspaceConfigurationMutation, WorkspaceConfigurationStore,
        model::WorkspaceConfigurationId,
    },
};

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
        let store_requires_recovery = WorkspaceConfigurationStore::try_global(cx)
            .is_some_and(|store| store.blocked().is_some());
        let has_saved_configurations = WorkspaceConfigurationStore::try_global(cx)
            .is_some_and(|store| !store.configurations().is_empty());
        if workspaces.len() < 2 && !has_saved_configurations && !store_requires_recovery {
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
                        .child(self.workspace_tab_heading(cx))
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

    pub(crate) fn test_workspace_configuration_menu_is_deployed(&self) -> bool {
        self.workspace_configuration_menu_handle.is_deployed()
    }

    #[cfg(test)]
    pub(crate) fn test_workspace_configuration_management_modal_is_open(&self, cx: &App) -> bool {
        self.active_modal::<WorkspaceConfigurationManagementModal>(cx)
            .is_some()
    }

    #[cfg(test)]
    pub(crate) fn test_workspace_configuration_management_modal_is_renaming(
        &self,
        cx: &App,
    ) -> bool {
        self.active_modal::<WorkspaceConfigurationManagementModal>(cx)
            .is_some_and(|modal| {
                matches!(
                    modal.read(cx).mode,
                    WorkspaceConfigurationManagementMode::Rename { .. }
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn test_workspace_configuration_management_rename_text(
        &self,
        cx: &App,
    ) -> Option<String> {
        let modal = self.active_modal::<WorkspaceConfigurationManagementModal>(cx)?;
        match &modal.read(cx).mode {
            WorkspaceConfigurationManagementMode::Rename { name, .. } => {
                Some(name.read(cx).text(cx))
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_workspace_configuration_management_rename_input(
        &self,
        cx: &App,
    ) -> Option<Entity<InputField>> {
        let modal = self.active_modal::<WorkspaceConfigurationManagementModal>(cx)?;
        match &modal.read(cx).mode {
            WorkspaceConfigurationManagementMode::Rename { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_workspace_configuration_management_modal_is_confirming_delete(
        &self,
        cx: &App,
    ) -> bool {
        self.active_modal::<WorkspaceConfigurationManagementModal>(cx)
            .is_some_and(|modal| {
                matches!(
                    modal.read(cx).mode,
                    WorkspaceConfigurationManagementMode::ConfirmDelete { .. }
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn test_workspace_configuration_management_focused_control(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<&'static str> {
        let modal = self.active_modal::<WorkspaceConfigurationManagementModal>(cx)?;
        let modal = modal.read(cx);
        if modal.delete_focus_handle.is_focused(window) {
            Some("list-delete")
        } else if modal.done_focus_handle.is_focused(window) {
            Some("list-done")
        } else if modal.rename_cancel_focus_handle.is_focused(window) {
            Some("rename-cancel")
        } else if modal.rename_save_focus_handle.is_focused(window) {
            Some("rename-save")
        } else if modal.delete_cancel_focus_handle.is_focused(window) {
            Some("delete-cancel")
        } else if modal.delete_confirm_focus_handle.is_focused(window) {
            Some("delete-confirm")
        } else if modal.focus_handle.is_focused(window) {
            Some("list")
        } else {
            None
        }
    }
}

impl MultiWorkspace {
    fn workspace_tab_heading(&self, cx: &Context<Self>) -> AnyElement {
        let active_name = self
            .active_configuration_id()
            .and_then(|configuration_id| {
                WorkspaceConfigurationStore::try_global(cx)
                    .and_then(|store| store.configuration(configuration_id))
            })
            .map(|configuration| configuration.name.clone());
        let label: SharedString = active_name
            .as_ref()
            .map(|name| format!("Workspaces: {name}").into())
            .unwrap_or_else(|| "Workspaces".into());
        let is_stale = self.configuration_checkpoint_error().is_some();
        let mut tooltip = active_name
            .map(|name| format!("Workspace Configurations: {name}"))
            .unwrap_or_else(|| "Workspace Configurations".to_string());
        if is_stale {
            tooltip.push_str(" — changes not saved; open this menu to Retry");
        }
        let aria_label = if is_stale {
            "Workspace Configurations, changes not saved"
        } else {
            "Workspace Configurations"
        };
        let multi_workspace = cx.weak_entity();

        h_flex()
            .debug_selector(|| "WORKSPACE-TABS-HEADING".to_string())
            .h(px(24.))
            .flex_shrink_0()
            .w_full()
            .px_1()
            .items_center()
            .child(
                PopoverMenu::new("workspace-configuration-menu")
                    .full_width(true)
                    .with_handle(self.workspace_configuration_menu_handle.clone())
                    .trigger_with_tooltip(
                        ButtonLike::new("workspace-configuration-menu-trigger")
                            .full_width()
                            .style(ButtonStyle::Subtle)
                            .size(ButtonSize::None)
                            .aria_label(aria_label)
                            .child(
                                h_flex().w_full().min_w_0().justify_start().child(
                                    h_flex()
                                        .debug_selector(|| {
                                            "WORKSPACE-CONFIGURATION-TRIGGER-CONTENT".to_string()
                                        })
                                        .min_w_0()
                                        .overflow_hidden()
                                        .gap_1()
                                        .child(
                                            Label::new(label)
                                                .size(LabelSize::Small)
                                                .color(Color::Muted)
                                                .truncate(),
                                        )
                                        .when(is_stale, |this| {
                                            this.child(
                                                h_flex()
                                                    .debug_selector(|| {
                                                        "WORKSPACE-CONFIGURATION-STALE-WARNING"
                                                            .to_string()
                                                    })
                                                    .flex_none()
                                                    .size_4()
                                                    .child(
                                                        Icon::new(IconName::Warning)
                                                            .size(IconSize::XSmall)
                                                            .color(Color::Warning),
                                                    ),
                                            )
                                        })
                                        .child(
                                            Icon::new(IconName::ChevronDown)
                                                .size(IconSize::XSmall)
                                                .color(Color::Muted),
                                        ),
                                ),
                            ),
                        Tooltip::text(tooltip),
                    )
                    .anchor(Anchor::TopLeft)
                    .menu(move |window, cx| {
                        multi_workspace
                            .update(cx, |multi_workspace, cx| {
                                multi_workspace.build_workspace_configuration_menu(window, cx)
                            })
                            .ok()
                    }),
            )
            .into_any_element()
    }

    fn build_workspace_configuration_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ContextMenu> {
        let configurations = WorkspaceConfigurationStore::try_global(cx)
            .map(|store| store.configurations().to_vec())
            .unwrap_or_default();
        let store_block_message = WorkspaceConfigurationStore::try_global(cx)
            .and_then(WorkspaceConfigurationStore::blocked)
            .map(StoreBlock::user_facing_message);
        let store_is_blocked = store_block_message.is_some();
        let has_configurations = !configurations.is_empty();
        let active_configuration_id = self.active_configuration_id();
        let is_stale = self.configuration_checkpoint_error().is_some();
        let switch_in_progress = self.configuration_switch_in_progress();
        let multi_workspace = cx.weak_entity();

        ContextMenu::build(window, cx, move |mut menu, _, _| {
            if let Some(message) = store_block_message {
                menu = menu.item(ContextMenuEntry::new(message).disabled(true));
            } else if configurations.is_empty() {
                menu = menu.item(ContextMenuEntry::new("No Saved Configurations").disabled(true));
            } else {
                for configuration in &configurations {
                    let configuration_id = configuration.id;
                    let configuration_name = configuration.name.clone();
                    let multi_workspace = multi_workspace.clone();
                    menu = menu.toggleable_entry_disabled_when(
                        configuration.name.clone(),
                        active_configuration_id == Some(configuration_id),
                        switch_in_progress,
                        IconPosition::Start,
                        None,
                        move |window, cx| {
                            multi_workspace
                                .update(cx, |multi_workspace, cx| {
                                    multi_workspace.switch_workspace_configuration_from_ui(
                                        configuration_id,
                                        configuration_name.clone(),
                                        window,
                                        cx,
                                    );
                                })
                                .ok();
                        },
                    );
                }
            }

            menu = menu.separator();
            if is_stale {
                let multi_workspace = multi_workspace.clone();
                menu = menu.item(
                    ContextMenuEntry::new("Retry")
                        .disabled(switch_in_progress)
                        .handler(move |window, cx| {
                            multi_workspace
                                .update(cx, |multi_workspace, cx| {
                                    multi_workspace
                                        .retry_workspace_configuration_from_ui(window, cx);
                                })
                                .ok();
                        }),
                );
            }

            let save_workspace = multi_workspace.clone();
            menu = menu.item(
                ContextMenuEntry::new("Save Workspace Configuration As…")
                    .action(SaveWorkspaceConfigurationAs.boxed_clone())
                    .disabled(switch_in_progress || store_is_blocked)
                    .handler(move |window, cx| {
                        save_workspace
                            .update(cx, |multi_workspace, cx| {
                                multi_workspace.show_save_workspace_configuration_as(window, cx);
                            })
                            .log_err();
                    }),
            );

            let management_workspace = multi_workspace.clone();
            menu.item(
                ContextMenuEntry::new("Manage Workspace Configurations…")
                    .action(ManageWorkspaceConfigurations.boxed_clone())
                    .disabled(switch_in_progress || store_is_blocked || !has_configurations)
                    .handler(move |window, cx| {
                        management_workspace
                            .update(cx, |multi_workspace, cx| {
                                multi_workspace.show_manage_workspace_configurations(window, cx);
                            })
                            .log_err();
                    }),
            )
            .key_context("WorkspaceConfigurations")
        })
    }

    pub(crate) fn show_save_workspace_configuration_as(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.configuration_switch_in_progress() {
            return;
        }
        WorkspaceConfigurationStore::init(cx);
        let multi_workspace = cx.weak_entity();
        self.toggle_modal(window, cx, move |window, cx| {
            SaveWorkspaceConfigurationModal::new(multi_workspace, None, window, cx)
        });
    }

    pub(crate) fn show_manage_workspace_configurations(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.configuration_switch_in_progress() {
            return;
        }
        WorkspaceConfigurationStore::init(cx);
        if WorkspaceConfigurationStore::global(cx).blocked().is_some()
            || WorkspaceConfigurationStore::global(cx)
                .configurations()
                .is_empty()
        {
            return;
        }
        let multi_workspace = cx.weak_entity();
        self.toggle_modal(window, cx, move |window, cx| {
            WorkspaceConfigurationManagementModal::new(multi_workspace, window, cx)
        });
    }

    pub(crate) fn switch_workspace_configuration_from_ui(
        &mut self,
        configuration_id: WorkspaceConfigurationId,
        configuration_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_configuration_id().is_none()
            && !self.unnamed_configuration_is_pristine_scratch(window, cx)
        {
            let prompt = window.prompt(
                PromptLevel::Warning,
                "Save Current Workspace Configuration?",
                Some(&format!(
                    "Save the current workspace set before switching to “{configuration_name}”?"
                )),
                &[
                    "Save Workspace Configuration As…",
                    "Switch Without Saving",
                    "Cancel",
                ],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                match prompt.await {
                    Ok(0) => {
                        this.update_in(cx, |this, window, cx| {
                            let multi_workspace = cx.weak_entity();
                            this.toggle_modal(window, cx, move |window, cx| {
                                SaveWorkspaceConfigurationModal::new(
                                    multi_workspace,
                                    Some((configuration_id, configuration_name)),
                                    window,
                                    cx,
                                )
                            });
                        })?;
                    }
                    Ok(1) => {
                        this.update_in(cx, |this, window, cx| {
                            this.start_workspace_configuration_switch(
                                configuration_id,
                                configuration_name,
                                window,
                                cx,
                            );
                        })?;
                    }
                    Ok(_) | Err(_) => {}
                }
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
            return;
        }

        self.start_workspace_configuration_switch(configuration_id, configuration_name, window, cx);
    }

    fn start_workspace_configuration_switch(
        &mut self,
        configuration_id: WorkspaceConfigurationId,
        configuration_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let switch = self.switch_workspace_configuration(configuration_id, window, cx);
        cx.spawn_in(window, async move |_, cx| {
            if let Err(error) = switch.await {
                if Self::workspace_configuration_switch_was_canceled(&error) {
                    return anyhow::Ok(());
                }
                log::error!(
                    "failed to switch workspace configuration {:?}: {error:#}",
                    configuration_id
                );
                cx.update(|window, cx| {
                    let detail = format!(
                        "“{configuration_name}” wasn’t applied. Your current workspaces are unchanged.\n\n{error}"
                    );
                    drop(window.prompt(
                        PromptLevel::Critical,
                        "Couldn’t Switch Workspace Configuration",
                        Some(&detail),
                        &["OK"],
                        cx,
                    ));
                })?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn retry_workspace_configuration_from_ui(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let retry = self.retry_configuration_checkpoint(cx);
        cx.spawn_in(window, async move |_, cx| {
            if let Err(error) = retry.await {
                log::error!("failed to retry workspace configuration checkpoint: {error:#}");
                cx.update(|window, cx| {
                    let detail = format!(
                        "Your current workspaces are intact, but the saved configuration is still out of date.\n\n{error}"
                    );
                    drop(window.prompt(
                        PromptLevel::Critical,
                        "Couldn’t Save Workspace Configuration",
                        Some(&detail),
                        &["OK"],
                        cx,
                    ));
                })?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }
}

struct SaveWorkspaceConfigurationModal {
    name: Entity<InputField>,
    multi_workspace: WeakEntity<MultiWorkspace>,
    switch_after_save: Option<(WorkspaceConfigurationId, String)>,
    saving: bool,
}

impl SaveWorkspaceConfigurationModal {
    fn new(
        multi_workspace: WeakEntity<MultiWorkspace>,
        switch_after_save: Option<(WorkspaceConfigurationId, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputField::new(window, cx, "Name").label("Name"));
        Self {
            name,
            multi_workspace,
            switch_after_save,
            saving: false,
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if !self.saving {
            cx.emit(DismissEvent);
        }
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let name = self.name.read(cx).text(cx).trim().to_string();
        let validation_error = if name.is_empty() {
            Some("Enter a configuration name.".to_string())
        } else if WorkspaceConfigurationStore::try_global(cx).is_some_and(|store| {
            store
                .configurations()
                .iter()
                .any(|configuration| configuration.name.eq_ignore_ascii_case(&name))
        }) {
            Some(format!(
                "A workspace configuration named “{name}” already exists."
            ))
        } else {
            None
        };
        if let Some(error) = validation_error {
            self.name
                .update(cx, |name, cx| name.set_error(Some(error), cx));
            return;
        }

        self.name
            .update(cx, |name, cx| name.set_error(None::<String>, cx));
        self.saving = true;
        cx.notify();
        let multi_workspace = self.multi_workspace.clone();
        cx.spawn_in(window, async move |this, cx| {
            let save = multi_workspace.update(cx, |multi_workspace, cx| {
                multi_workspace.save_configuration_as(name, cx)
            })?;
            let result = save.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(_) => {
                        cx.emit(DismissEvent);
                        if let Some((configuration_id, configuration_name)) =
                            this.switch_after_save.take()
                        {
                            this.multi_workspace
                                .update(cx, |multi_workspace, cx| {
                                    multi_workspace.start_workspace_configuration_switch(
                                        configuration_id,
                                        configuration_name,
                                        window,
                                        cx,
                                    );
                                })
                                .ok();
                        }
                    }
                    Err(error) => this
                        .name
                        .update(cx, |name, cx| name.set_error(Some(error.to_string()), cx)),
                }
                cx.notify();
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }
}

impl Focusable for SaveWorkspaceConfigurationModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for SaveWorkspaceConfigurationModal {}
impl ModalView for SaveWorkspaceConfigurationModal {}

impl Render for SaveWorkspaceConfigurationModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .debug_selector(|| "SAVE-WORKSPACE-CONFIGURATION-MODAL".to_string())
            .key_context("SaveWorkspaceConfiguration")
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .w(rems(30.))
            .elevation_3(cx)
            .bg(cx.theme().colors().elevated_surface_background)
            .rounded_md()
            .overflow_hidden()
            .child(
                v_flex()
                    .p_3()
                    .gap_3()
                    .child(Label::new("Save Workspace Configuration"))
                    .child(self.name.clone()),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(
                        Button::new("cancel-workspace-configuration", "Cancel")
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel(&menu::Cancel, window, cx)
                            })),
                    )
                    .child(
                        Button::new("save-workspace-configuration", "Save")
                            .style(ButtonStyle::Filled)
                            .loading(self.saving)
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm(&menu::Confirm, window, cx)
                            })),
                    ),
            )
    }
}

#[derive(Clone)]
enum WorkspaceConfigurationManagementMode {
    List,
    Rename {
        id: WorkspaceConfigurationId,
        name: Entity<InputField>,
    },
    ConfirmDelete {
        id: WorkspaceConfigurationId,
        name: String,
    },
}

struct WorkspaceConfigurationManagementModal {
    focus_handle: FocusHandle,
    delete_focus_handle: FocusHandle,
    done_focus_handle: FocusHandle,
    rename_cancel_focus_handle: FocusHandle,
    rename_save_focus_handle: FocusHandle,
    delete_cancel_focus_handle: FocusHandle,
    delete_confirm_focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    multi_workspace: WeakEntity<MultiWorkspace>,
    selected_configuration_id: Option<WorkspaceConfigurationId>,
    mode: WorkspaceConfigurationManagementMode,
    busy: bool,
    error: Option<String>,
}

impl WorkspaceConfigurationManagementModal {
    fn new(
        multi_workspace: WeakEntity<MultiWorkspace>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected_configuration_id = WorkspaceConfigurationStore::try_global(cx)
            .and_then(|store| store.configurations().first())
            .map(|configuration| configuration.id);
        Self {
            focus_handle: cx.focus_handle(),
            delete_focus_handle: cx.focus_handle(),
            done_focus_handle: cx.focus_handle(),
            rename_cancel_focus_handle: cx.focus_handle(),
            rename_save_focus_handle: cx.focus_handle(),
            delete_cancel_focus_handle: cx.focus_handle(),
            delete_confirm_focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            multi_workspace,
            selected_configuration_id,
            mode: WorkspaceConfigurationManagementMode::List,
            busy: false,
            error: None,
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match self.mode {
            WorkspaceConfigurationManagementMode::List => cx.emit(DismissEvent),
            _ => {
                self.mode = WorkspaceConfigurationManagementMode::List;
                self.error = None;
                self.focus_handle.focus(window, cx);
                cx.notify();
            }
        }
    }

    fn start_rename(
        &mut self,
        id: WorkspaceConfigurationId,
        current_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_configuration_id = Some(id);
        let name = cx.new(|cx| {
            InputField::new(window, cx, "Name")
                .label("Name")
                .tab_index(0)
        });
        name.update(cx, |name, cx| {
            name.set_text(&current_name, window, cx);
            name.editor().select_all(window, cx);
        });
        name.focus_handle(cx).focus(window, cx);
        self.mode = WorkspaceConfigurationManagementMode::Rename { id, name };
        self.error = None;
        cx.notify();
    }

    fn start_delete(
        &mut self,
        id: WorkspaceConfigurationId,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_configuration_id = Some(id);
        self.mode = WorkspaceConfigurationManagementMode::ConfirmDelete { id, name };
        self.error = None;
        self.delete_cancel_focus_handle.focus(window, cx);
        cx.notify();
    }

    fn select_next(
        &mut self,
        _: &SelectNextWorkspaceConfiguration,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(true, cx);
    }

    fn select_previous(
        &mut self,
        _: &SelectPreviousWorkspaceConfiguration,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(false, cx);
    }

    fn move_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        if !matches!(self.mode, WorkspaceConfigurationManagementMode::List) {
            return;
        }
        let configurations = WorkspaceConfigurationStore::try_global(cx)
            .map(|store| store.configurations())
            .unwrap_or_default();
        if configurations.is_empty() {
            self.selected_configuration_id = None;
            return;
        }
        let selected_index = self
            .selected_configuration_id
            .and_then(|selected_id| {
                configurations
                    .iter()
                    .position(|configuration| configuration.id == selected_id)
            })
            .unwrap_or(0);
        let next_index = if forward {
            (selected_index + 1) % configurations.len()
        } else if selected_index == 0 {
            configurations.len() - 1
        } else {
            selected_index - 1
        };
        self.selected_configuration_id = Some(configurations[next_index].id);
        self.scroll_handle.scroll_to_item(next_index);
        cx.notify();
    }

    fn focus_next(&mut self, _: &menu::SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        match &self.mode {
            WorkspaceConfigurationManagementMode::List => {
                if self.delete_focus_handle.is_focused(window) {
                    self.done_focus_handle.focus(window, cx);
                } else if self.done_focus_handle.is_focused(window) {
                    self.focus_handle.focus(window, cx);
                } else {
                    self.delete_focus_handle.focus(window, cx);
                }
            }
            WorkspaceConfigurationManagementMode::Rename { name, .. } => {
                if self.rename_cancel_focus_handle.is_focused(window) {
                    self.rename_save_focus_handle.focus(window, cx);
                } else if self.rename_save_focus_handle.is_focused(window) {
                    name.focus_handle(cx).focus(window, cx);
                } else {
                    self.rename_cancel_focus_handle.focus(window, cx);
                }
            }
            WorkspaceConfigurationManagementMode::ConfirmDelete { .. } => {
                if self.delete_cancel_focus_handle.is_focused(window) {
                    self.delete_confirm_focus_handle.focus(window, cx);
                } else {
                    self.delete_cancel_focus_handle.focus(window, cx);
                }
            }
        }
    }

    fn focus_previous(
        &mut self,
        _: &menu::SelectPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &self.mode {
            WorkspaceConfigurationManagementMode::List => {
                if self.done_focus_handle.is_focused(window) {
                    self.delete_focus_handle.focus(window, cx);
                } else if self.delete_focus_handle.is_focused(window) {
                    self.focus_handle.focus(window, cx);
                } else {
                    self.done_focus_handle.focus(window, cx);
                }
            }
            WorkspaceConfigurationManagementMode::Rename { name, .. } => {
                if self.rename_save_focus_handle.is_focused(window) {
                    self.rename_cancel_focus_handle.focus(window, cx);
                } else if self.rename_cancel_focus_handle.is_focused(window) {
                    name.focus_handle(cx).focus(window, cx);
                } else {
                    self.rename_save_focus_handle.focus(window, cx);
                }
            }
            WorkspaceConfigurationManagementMode::ConfirmDelete { .. } => {
                if self.delete_confirm_focus_handle.is_focused(window) {
                    self.delete_cancel_focus_handle.focus(window, cx);
                } else {
                    self.delete_confirm_focus_handle.focus(window, cx);
                }
            }
        }
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let cancel_is_focused = match &self.mode {
            WorkspaceConfigurationManagementMode::List => self.done_focus_handle.is_focused(window),
            WorkspaceConfigurationManagementMode::Rename { .. } => {
                self.rename_cancel_focus_handle.is_focused(window)
            }
            WorkspaceConfigurationManagementMode::ConfirmDelete { .. } => {
                self.delete_cancel_focus_handle.is_focused(window)
            }
        };
        if cancel_is_focused {
            self.cancel(&menu::Cancel, window, cx);
            return;
        }
        if matches!(self.mode, WorkspaceConfigurationManagementMode::List)
            && self.delete_focus_handle.is_focused(window)
        {
            let Some(configuration_id) = self.selected_configuration_id else {
                return;
            };
            let Some(configuration_name) = WorkspaceConfigurationStore::try_global(cx)
                .and_then(|store| store.configuration(configuration_id))
                .map(|configuration| configuration.name.clone())
            else {
                return;
            };
            self.start_delete(configuration_id, configuration_name, window, cx);
            return;
        }
        self.confirm_primary(window, cx);
    }

    fn confirm_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match &self.mode {
            WorkspaceConfigurationManagementMode::List => {
                let Some(configuration_id) = self.selected_configuration_id else {
                    return;
                };
                let Some(configuration_name) = WorkspaceConfigurationStore::try_global(cx)
                    .and_then(|store| store.configuration(configuration_id))
                    .map(|configuration| configuration.name.clone())
                else {
                    return;
                };
                self.start_rename(configuration_id, configuration_name, window, cx);
            }
            WorkspaceConfigurationManagementMode::Rename { id, name } => {
                let id = *id;
                let name_field = name.clone();
                let name = name.read(cx).text(cx).trim().to_string();
                let validation_error = if name.is_empty() {
                    Some("Enter a configuration name.".to_string())
                } else if WorkspaceConfigurationStore::try_global(cx).is_some_and(|store| {
                    store.configurations().iter().any(|configuration| {
                        configuration.id != id && configuration.name.eq_ignore_ascii_case(&name)
                    })
                }) {
                    Some(format!(
                        "A workspace configuration named “{name}” already exists."
                    ))
                } else {
                    None
                };
                if let Some(error) = validation_error {
                    name_field.update(cx, |name, cx| name.set_error(Some(error), cx));
                    return;
                }

                name_field.update(cx, |name, cx| name.set_error(None::<String>, cx));
                self.busy = true;
                self.error = None;
                cx.notify();
                let rename = WorkspaceConfigurationStore::mutate_global(
                    WorkspaceConfigurationMutation::Rename { id, name },
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    let result = rename.await;
                    this.update_in(cx, |this, window, cx| {
                        this.busy = false;
                        match result {
                            Ok(_) => {
                                this.mode = WorkspaceConfigurationManagementMode::List;
                                this.error = None;
                                this.focus_handle.focus(window, cx);
                            }
                            Err(error) => {
                                log::error!(
                                    "failed to rename workspace configuration {:?}: {error:#}",
                                    id
                                );
                                if WorkspaceConfigurationStore::global(cx)
                                    .configuration(id)
                                    .is_none()
                                {
                                    this.mode = WorkspaceConfigurationManagementMode::List;
                                    this.selected_configuration_id =
                                        WorkspaceConfigurationStore::global(cx)
                                            .configurations()
                                            .first()
                                            .map(|configuration| configuration.id);
                                    this.focus_handle.focus(window, cx);
                                }
                                this.error = Some(
                                    "Couldn't Rename Workspace Configuration\n\nThe saved name was not changed. See the Zed log for details."
                                        .to_string(),
                                );
                            }
                        }
                        cx.notify();
                    })?;
                    anyhow::Ok(())
                })
                .detach_and_log_err(cx);
            }
            WorkspaceConfigurationManagementMode::ConfirmDelete { id, name } => {
                let id = *id;
                let configuration_name = name.clone();
                let delete = match self.multi_workspace.update(cx, |multi_workspace, cx| {
                    multi_workspace.delete_workspace_configuration(id, cx)
                }) {
                    Ok(delete) => delete,
                    Err(error) => {
                        log::error!(
                            "failed to start deleting workspace configuration {:?}: {error:#}",
                            id
                        );
                        self.error = Some(format!(
                            "Couldn't Delete Workspace Configuration\n\n“{configuration_name}” was not deleted. Your workspaces are unchanged. See the Zed log for details."
                        ));
                        cx.notify();
                        return;
                    }
                };
                self.busy = true;
                self.error = None;
                cx.notify();
                cx.spawn_in(window, async move |this, cx| {
                    let result = delete.await;
                    this.update_in(cx, |this, window, cx| {
                        this.busy = false;
                        match result {
                            Ok(()) => {
                                if WorkspaceConfigurationStore::global(cx)
                                    .configurations()
                                    .is_empty()
                                {
                                    cx.emit(DismissEvent);
                                } else {
                                    this.mode = WorkspaceConfigurationManagementMode::List;
                                    this.error = None;
                                    this.selected_configuration_id =
                                        WorkspaceConfigurationStore::global(cx)
                                            .configurations()
                                            .first()
                                            .map(|configuration| configuration.id);
                                    this.focus_handle.focus(window, cx);
                                }
                            }
                            Err(error) => {
                                log::error!(
                                    "failed to delete workspace configuration {:?}: {error:#}",
                                    id
                                );
                                this.error = Some(format!(
                                    "Couldn't Delete Workspace Configuration\n\n“{configuration_name}” was not deleted. Your workspaces are unchanged. See the Zed log for details."
                                ));
                            }
                        }
                        cx.notify();
                    })?;
                    anyhow::Ok(())
                })
                .detach_and_log_err(cx);
            }
        }
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let configurations = WorkspaceConfigurationStore::try_global(cx)
            .map(|store| store.configurations().to_vec())
            .unwrap_or_default();
        let active_configuration_id = self
            .multi_workspace
            .read_with(cx, |multi_workspace, _| {
                multi_workspace.active_configuration_id()
            })
            .ok()
            .flatten();
        let mode = self.mode.clone();
        let mut rows = Vec::with_capacity(configurations.len());
        for (index, configuration) in configurations.into_iter().enumerate() {
            let configuration_id = configuration.id;
            let is_selected = self.selected_configuration_id == Some(configuration_id);
            let is_active = active_configuration_id == Some(configuration_id);

            if let WorkspaceConfigurationManagementMode::Rename { id, name } = &mode
                && *id == configuration_id
            {
                rows.push(
                    v_flex()
                        .when(index > 0, |this| {
                            this.border_t_1()
                                .border_color(cx.theme().colors().border_variant)
                        })
                        .w_full()
                        .gap_2()
                        .p_3()
                        .border_l_2()
                        .border_color(cx.theme().colors().border_focused)
                        .bg(cx.theme().colors().ghost_element_selected)
                        .child(
                            h_flex()
                                .min_w_0()
                                .gap_2()
                                .child(configuration_marker(is_active))
                                .child(Label::new("Rename Workspace Configuration")),
                        )
                        .child(name.clone())
                        .when_some(self.error.clone(), |this, error| {
                            this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
                        })
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("cancel-workspace-configuration-rename", "Cancel")
                                        .track_focus(&self.rename_cancel_focus_handle)
                                        .tab_index(1_isize)
                                        .disabled(self.busy)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.cancel(&menu::Cancel, window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("save-workspace-configuration-rename", "Save")
                                        .track_focus(&self.rename_save_focus_handle)
                                        .tab_index(2_isize)
                                        .style(ButtonStyle::Filled)
                                        .loading(self.busy)
                                        .disabled(self.busy)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.confirm_primary(window, cx)
                                        })),
                                ),
                        )
                        .into_any_element(),
                );
                continue;
            }

            let rename_name = configuration.name.clone();
            let delete_name = configuration.name.clone();
            rows.push(
                h_flex()
                    .id(("workspace-configuration-management-row", index))
                    .when(index > 0, |this| {
                        this.border_t_1()
                            .border_color(cx.theme().colors().border_variant)
                    })
                    .w_full()
                    .min_w_0()
                    .justify_between()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .border_l_2()
                    .border_color(if is_selected {
                        cx.theme().colors().border_focused
                    } else {
                        cx.theme().colors().border.opacity(0.)
                    })
                    .cursor_pointer()
                    .hover(|this| this.bg(cx.theme().colors().ghost_element_hover))
                    .when(is_selected, |this| {
                        this.bg(cx.theme().colors().ghost_element_selected)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected_configuration_id = Some(configuration_id);
                        this.focus_handle.focus(window, cx);
                        cx.notify();
                    }))
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(configuration_marker(is_active))
                            .child(Label::new(configuration.name).truncate()),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .child(
                                Button::new(("rename-workspace-configuration", index), "Rename")
                                    .style(ButtonStyle::Subtle)
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.selected_configuration_id = Some(configuration_id);
                                        this.start_rename(
                                            configuration_id,
                                            rename_name.clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(("delete-workspace-configuration", index), "Delete")
                                    .style(ButtonStyle::Subtle)
                                    .when(is_selected, |this| {
                                        this.track_focus(&self.delete_focus_handle)
                                            .tab_index(0_isize)
                                    })
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start_delete(
                                            configuration_id,
                                            delete_name.clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }

        v_flex()
            .p_3()
            .gap_3()
            .child(Label::new("Manage Workspace Configurations"))
            .child(
                v_flex()
                    .border_1()
                    .border_color(cx.theme().colors().border_variant)
                    .rounded_md()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .id("workspace-configuration-management-list")
                            .max_h(rems(24.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .when(rows.is_empty(), |this| {
                                this.child(
                                    Label::new("No saved workspace configurations.")
                                        .color(Color::Muted),
                                )
                                .p_3()
                            })
                            .children(rows),
                    ),
            )
            .when(
                matches!(mode, WorkspaceConfigurationManagementMode::List),
                |this| {
                    this.when_some(self.error.clone(), |this, error| {
                        this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
                    })
                },
            )
            .when(
                matches!(mode, WorkspaceConfigurationManagementMode::List),
                |this| {
                    this.child(
                        h_flex().justify_end().child(
                            Button::new("done-workspace-configuration-management", "Done")
                                .track_focus(&self.done_focus_handle)
                                .tab_index(1_isize)
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel(&menu::Cancel, window, cx)
                                })),
                        ),
                    )
                },
            )
            .into_any_element()
    }

    fn render_delete(&self, name: String, _cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .p_3()
            .gap_3()
            .child(
                h_flex()
                    .items_start()
                    .gap_3()
                    .child(Icon::new(IconName::Trash).color(Color::Error))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(Label::new(format!("Delete “{name}”?")))
                            .child(Label::new(
                                "Its saved set will be removed. Its workspaces and editor state will not be deleted. Any open window using it will keep its current workspaces as an unnamed set.",
                            )),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
            })
            .into_any_element()
    }
}

fn configuration_marker(is_active: bool) -> AnyElement {
    div()
        .w(rems(1.))
        .flex_none()
        .when(is_active, |this| {
            this.child(
                Icon::new(IconName::Check)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
        })
        .into_any_element()
}

impl Focusable for WorkspaceConfigurationManagementModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.mode {
            WorkspaceConfigurationManagementMode::Rename { name, .. } => name.focus_handle(cx),
            WorkspaceConfigurationManagementMode::ConfirmDelete { .. } => {
                self.delete_cancel_focus_handle.clone()
            }
            WorkspaceConfigurationManagementMode::List => self.focus_handle.clone(),
        }
    }
}

impl EventEmitter<DismissEvent> for WorkspaceConfigurationManagementModal {}
impl ModalView for WorkspaceConfigurationManagementModal {}

impl Render for WorkspaceConfigurationManagementModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edited_configuration_is_missing = match &self.mode {
            WorkspaceConfigurationManagementMode::Rename { id, .. } => {
                WorkspaceConfigurationStore::try_global(cx)
                    .is_none_or(|store| store.configuration(*id).is_none())
            }
            _ => false,
        };
        if edited_configuration_is_missing {
            self.mode = WorkspaceConfigurationManagementMode::List;
            self.selected_configuration_id = WorkspaceConfigurationStore::try_global(cx)
                .and_then(|store| store.configurations().first())
                .map(|configuration| configuration.id);
            self.error = Some(
                "Couldn't Rename Workspace Configuration\n\nThe saved name was not changed. See the Zed log for details."
                    .to_string(),
            );
            self.focus_handle.focus(window, cx);
        }
        let mode = self.mode.clone();
        let content = match &mode {
            WorkspaceConfigurationManagementMode::List
            | WorkspaceConfigurationManagementMode::Rename { .. } => self.render_list(cx),
            WorkspaceConfigurationManagementMode::ConfirmDelete { name, .. } => {
                self.render_delete(name.clone(), cx)
            }
        };

        v_flex()
            .debug_selector(|| "WORKSPACE-CONFIGURATION-MANAGEMENT-MODAL".to_string())
            .key_context("ManageWorkspaceConfigurations")
            .track_focus(&self.focus_handle)
            .tab_group()
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .w(rems(38.))
            .elevation_3(cx)
            .bg(cx.theme().colors().elevated_surface_background)
            .rounded_md()
            .overflow_hidden()
            .child(content)
            .when(
                matches!(
                    mode,
                    WorkspaceConfigurationManagementMode::ConfirmDelete { .. }
                ),
                |this| {
                    this.child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .p_3()
                            .border_t_1()
                            .border_color(cx.theme().colors().border_variant)
                            .child(
                                Button::new("cancel-workspace-configuration-management", "Cancel")
                                    .track_focus(&self.delete_cancel_focus_handle)
                                    .tab_index(0_isize)
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.cancel(&menu::Cancel, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("confirm-workspace-configuration-management", "Delete")
                                    .track_focus(&self.delete_confirm_focus_handle)
                                    .tab_index(1_isize)
                                    .style(ButtonStyle::Tinted(ui::TintColor::Error))
                                    .loading(self.busy)
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_primary(window, cx);
                                    })),
                            ),
                    )
                },
            )
    }
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
