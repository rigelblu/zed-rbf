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

use crate::{
    ModalView, MultiWorkspace, SaveWorkspaceConfigurationAs, Workspace,
    persistence::{StoreBlock, WorkspaceConfigurationStore, model::WorkspaceConfigurationId},
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

            let multi_workspace = multi_workspace.clone();
            menu.item(
                ContextMenuEntry::new("Save Workspace Configuration As…")
                    .action(SaveWorkspaceConfigurationAs.boxed_clone())
                    .disabled(switch_in_progress || store_is_blocked)
                    .handler(move |window, cx| {
                        multi_workspace
                            .update(cx, |multi_workspace, cx| {
                                multi_workspace.show_save_workspace_configuration_as(window, cx);
                            })
                            .ok();
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
