use std::{num::NonZeroUsize, time::Duration};

use crate::DockPosition;
use collections::{HashMap, HashSet};
use gpui::{App, Subscription};
use serde::Deserialize;
pub use settings::{
    AutosaveSetting, BottomDockLayout, EncodingDisplayOptions, InactiveOpacity,
    PaneSplitDirectionHorizontal, PaneSplitDirectionVertical, RegisterSetting,
    RestoreOnStartupBehavior, Settings, StatusBarItemLocation,
};
use settings::{CommandAliasTarget, SettingsStore};

#[derive(RegisterSetting)]
pub struct WorkspaceSettings {
    pub active_pane_modifiers: ActivePanelModifiers,
    pub bottom_dock_layout: settings::BottomDockLayout,
    pub pane_split_direction_horizontal: settings::PaneSplitDirectionHorizontal,
    pub pane_split_direction_vertical: settings::PaneSplitDirectionVertical,
    pub centered_layout: settings::CenteredLayoutSettings,
    pub confirm_quit: bool,
    pub show_call_status_icon: bool,
    pub autosave: AutosaveSetting,
    pub restore_on_startup: settings::RestoreOnStartupBehavior,
    pub cli_default_open_behavior: settings::CliDefaultOpenBehavior,
    pub default_open_behavior: settings::DefaultOpenBehavior,
    pub restore_on_file_reopen: bool,
    pub drop_target_size: f32,
    pub use_system_path_prompts: bool,
    pub use_system_prompts: bool,
    pub accessible_mode: bool,
    pub command_aliases: HashMap<String, CommandAliasTarget>,
    pub pinned_projects: Vec<String>,
    pub max_tabs: Option<NonZeroUsize>,
    pub when_closing_with_no_tabs: settings::CloseWindowWhenNoItems,
    pub on_last_window_closed: settings::OnLastWindowClosed,
    pub text_rendering_mode: settings::TextRenderingMode,
    pub resize_all_panels_in_dock: Vec<DockPosition>,
    pub close_on_file_delete: bool,
    pub close_panel_on_toggle: bool,
    pub use_system_window_tabs: bool,
    pub zoomed_padding: bool,
    pub window_decorations: settings::WindowDecorations,
    pub focus_follows_mouse: FocusFollowsMouse,
}

#[derive(Copy, Clone, Deserialize)]
pub struct FocusFollowsMouse {
    pub enabled: bool,
    pub debounce: Duration,
}

#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct ActivePanelModifiers {
    /// Size of the border surrounding the active pane.
    /// When set to 0, the active pane doesn't have any border.
    /// The border is drawn inset.
    ///
    /// Default: `0.0`
    // TODO: make this not an option, it is never None
    pub border_size: Option<f32>,
    /// Opacity of inactive panels.
    /// When set to 1.0, the inactive panes have the same opacity as the active one.
    /// If set to 0, the inactive panes content will not be visible at all.
    /// Values are clamped to the [0.0, 1.0] range.
    ///
    /// Default: `1.0`
    // TODO: make this not an option, it is never None
    pub inactive_opacity: Option<InactiveOpacity>,
}

#[derive(Deserialize, RegisterSetting)]
pub struct TabBarSettings {
    pub show: bool,
    pub show_nav_history_buttons: bool,
    pub show_tab_bar_buttons: bool,
    pub show_pinned_tabs_in_separate_row: bool,
}

impl Settings for WorkspaceSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let workspace = &content.workspace;
        Self {
            active_pane_modifiers: ActivePanelModifiers {
                border_size: Some(
                    workspace
                        .active_pane_modifiers
                        .unwrap()
                        .border_size
                        .unwrap(),
                ),
                inactive_opacity: Some(
                    workspace
                        .active_pane_modifiers
                        .unwrap()
                        .inactive_opacity
                        .unwrap(),
                ),
            },
            bottom_dock_layout: workspace.bottom_dock_layout.unwrap(),
            pane_split_direction_horizontal: workspace.pane_split_direction_horizontal.unwrap(),
            pane_split_direction_vertical: workspace.pane_split_direction_vertical.unwrap(),
            centered_layout: workspace.centered_layout.unwrap(),
            confirm_quit: workspace.confirm_quit.unwrap(),
            show_call_status_icon: workspace.show_call_status_icon.unwrap(),
            autosave: workspace.autosave.unwrap(),
            restore_on_startup: workspace.restore_on_startup.unwrap(),
            cli_default_open_behavior: workspace.cli_default_open_behavior.unwrap(),
            default_open_behavior: workspace.default_open_behavior.unwrap(),
            restore_on_file_reopen: workspace.restore_on_file_reopen.unwrap(),
            drop_target_size: workspace.drop_target_size.unwrap(),
            use_system_path_prompts: workspace.use_system_path_prompts.unwrap(),
            use_system_prompts: workspace.use_system_prompts.unwrap(),
            accessible_mode: workspace.accessible_mode.unwrap(),
            command_aliases: workspace.command_aliases.clone(),
            pinned_projects: workspace.pinned_projects.clone().unwrap_or_default(),
            max_tabs: workspace.max_tabs,
            when_closing_with_no_tabs: workspace.when_closing_with_no_tabs.unwrap(),
            on_last_window_closed: workspace.on_last_window_closed.unwrap(),
            text_rendering_mode: workspace.text_rendering_mode.unwrap(),
            resize_all_panels_in_dock: workspace
                .resize_all_panels_in_dock
                .clone()
                .unwrap()
                .into_iter()
                .map(Into::into)
                .collect(),
            close_on_file_delete: workspace.close_on_file_delete.unwrap(),
            close_panel_on_toggle: workspace.close_panel_on_toggle.unwrap(),
            use_system_window_tabs: workspace.use_system_window_tabs.unwrap(),
            zoomed_padding: workspace.zoomed_padding.unwrap(),
            window_decorations: workspace.window_decorations.unwrap(),
            focus_follows_mouse: FocusFollowsMouse {
                enabled: workspace
                    .focus_follows_mouse
                    .unwrap()
                    .enabled
                    .unwrap_or(false),
                debounce: Duration::from_millis(
                    workspace
                        .focus_follows_mouse
                        .unwrap()
                        .debounce_ms
                        .unwrap_or(250),
                ),
            },
        }
    }
}

/// Provides convenient access to whether "accessible mode" is enabled, mirroring
/// [`theme::ActiveTheme`] for the active theme. Import this trait to call
/// `cx.accessible_mode()`.
pub trait AccessibleMode {
    /// Returns whether accessible mode is enabled.
    fn accessible_mode(&self) -> bool;
}

impl AccessibleMode for App {
    fn accessible_mode(&self) -> bool {
        WorkspaceSettings::get_global(self).accessible_mode
    }
}

/// The components currently marked hidden everywhere.
fn hidden_component_keys(cx: &App) -> HashSet<String> {
    WorkspaceLayoutSettings::get_global(cx)
        .component_positions
        .iter()
        .filter(|(_, component)| component.hidden)
        .map(|(key, _)| key.clone())
        .collect()
}

/// Observes `workspace_layout.component_positions` and invokes `callback` with
/// the components whose `hidden` flag went `true` to `false`.
///
/// This exists because settings.json is a real unhide path, not only a back
/// door: a hand edit reaches no code, so an unhide performed there would
/// otherwise leave every dormant workspace's open rows in place. Dropping an
/// entry entirely counts as unhiding it, since `hidden` defaults to false.
///
/// Mirrors [`observe_accessible_mode`] — it holds the previous set in its
/// closure and fires only on a real change. The returned [`Subscription`] must
/// be retained for the callback to keep firing.
pub fn observe_components_unhidden(
    cx: &mut App,
    mut callback: impl FnMut(Vec<String>, &mut App) + 'static,
) -> Subscription {
    let mut last = hidden_component_keys(cx);
    cx.observe_global::<SettingsStore>(move |cx| {
        let current = hidden_component_keys(cx);
        if current == last {
            return;
        }
        let unhidden: Vec<String> = last.difference(&current).cloned().collect();
        last = current;
        if !unhidden.is_empty() {
            callback(unhidden, cx);
        }
    })
}

/// Observes changes to the accessible-mode setting, invoking `callback` with the
/// new value whenever it changes. Mirrors the common
/// `cx.observe_global::<SettingsStore>` pattern, but only fires when the value
/// actually changes. The returned [`Subscription`] must be retained for the
/// callback to keep firing.
pub fn observe_accessible_mode(
    cx: &mut App,
    mut callback: impl FnMut(bool, &mut App) + 'static,
) -> Subscription {
    let mut last = cx.accessible_mode();
    cx.observe_global::<SettingsStore>(move |cx| {
        let current = cx.accessible_mode();
        if current != last {
            last = current;
            callback(current, cx);
        }
    })
}

impl Settings for TabBarSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let tab_bar = content.tab_bar.clone().unwrap();
        TabBarSettings {
            show: tab_bar.show.unwrap(),
            show_nav_history_buttons: tab_bar.show_nav_history_buttons.unwrap(),
            show_tab_bar_buttons: tab_bar.show_tab_bar_buttons.unwrap(),
            show_pinned_tabs_in_separate_row: tab_bar.show_pinned_tabs_in_separate_row.unwrap(),
        }
    }
}

#[derive(Deserialize, RegisterSetting)]
pub struct StatusBarSettings {
    pub show: bool,
    pub show_active_file: bool,
    pub active_language_button: bool,
    pub cursor_position_button: bool,
    pub line_endings_button: bool,
    pub active_encoding_button: EncodingDisplayOptions,
    pub item_locations: HashMap<String, StatusBarItemLocation>,
}

impl Settings for StatusBarSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let status_bar = content.status_bar.clone().unwrap();
        StatusBarSettings {
            show: status_bar.show.unwrap(),
            show_active_file: status_bar.show_active_file.unwrap(),
            active_language_button: status_bar.active_language_button.unwrap(),
            cursor_position_button: status_bar.cursor_position_button.unwrap(),
            line_endings_button: status_bar.line_endings_button.unwrap(),
            active_encoding_button: status_bar.active_encoding_button.unwrap(),
            item_locations: status_bar.item_locations.unwrap(),
        }
    }
}

impl StatusBarSettings {
    /// An `item_locations` entry naming a workspace component is ignored rather
    /// than honoured. Component placement moved to
    /// `workspace_layout.component_positions`; obeying a stale entry here would
    /// move the icon while the component stayed where it was, which is precisely
    /// the model human Check 3 disproved. Entries for ids we do not recognise are
    /// preserved and honoured, so an extension keeps its placement while its
    /// crate is absent.
    fn explicit_location(&self, item_id: &str) -> Option<StatusBarItemLocation> {
        if is_workspace_component(item_id) {
            return None;
        }
        self.item_locations.get(item_id).copied()
    }

    /// Where a status control sits. `None` means we have nothing to say about this
    /// id — it names a workspace component, or an item the registry does not know
    /// and the user has not placed.
    pub fn status_control_location(&self, item_id: &str) -> Option<StatusBarItemLocation> {
        if is_workspace_component(item_id) {
            return None;
        }
        self.explicit_location(item_id)
            .or_else(|| registered_status_control_location(item_id))
    }

    /// Where a status control actually renders, given its own legacy setting.
    /// An explicit placement wins in both directions; with none, the legacy value
    /// decides between the registered position and nowhere at all — which is what
    /// keeps an existing configuration behaving as it did before this setting
    /// existed.
    ///
    /// `None` means we have nothing to say: the id names a workspace component, or
    /// an item the registry does not know.
    pub fn resolved_status_control_location(
        &self,
        item_id: &str,
        legacy_enabled: bool,
    ) -> Option<StatusBarItemLocation> {
        if let Some(explicit) = self.explicit_location(item_id) {
            return Some(explicit);
        }
        let registered = registered_status_control_location(item_id)?;
        Some(if legacy_enabled {
            registered
        } else {
            StatusBarItemLocation::Hidden
        })
    }

    /// Whether a status control renders. Derived from the resolver above rather
    /// than deciding separately: "is it visible" and "where is it" are one fact,
    /// and answering them from two places is how they come to disagree.
    pub fn item_enabled(&self, item_id: &str, legacy_enabled: bool) -> bool {
        match self.resolved_status_control_location(item_id, legacy_enabled) {
            Some(StatusBarItemLocation::Hidden) => false,
            Some(_) => true,
            None => legacy_enabled,
        }
    }

    /// Active Encoding is the one control whose placement and display rule are
    /// separate questions: placing it must not silently turn on an indicator the
    /// user asked to see only for non-UTF-8 files, and must not leave a placed
    /// item invisible either.
    pub fn encoding_display_option(&self, item_id: &str) -> EncodingDisplayOptions {
        match self.explicit_location(item_id) {
            Some(StatusBarItemLocation::Hidden) => EncodingDisplayOptions::Disabled,
            Some(_) if self.active_encoding_button == EncodingDisplayOptions::Disabled => {
                EncodingDisplayOptions::NonUtf8
            }
            _ => self.active_encoding_button,
        }
    }
}

fn is_workspace_component(item_id: &str) -> bool {
    crate::layout_registry::get(item_id)
        .is_some_and(|item| item.kind == crate::layout_registry::LayoutItemKind::WorkspaceComponent)
}

/// The zone a status control belongs in — a position, never `Hidden`. Whether it
/// is currently visible is a separate question, answered by an explicit entry or
/// by the item's own legacy setting. Folding `default_hidden` in here would mean
/// `show_active_file: true` could never bring the file name back, because a
/// registered default would outrank the setting the user just changed.
fn registered_status_control_location(item_id: &str) -> Option<StatusBarItemLocation> {
    use crate::layout_registry::LayoutPosition;

    let item = crate::layout_registry::get(item_id)?;
    match item.registered_position {
        LayoutPosition::Left => Some(StatusBarItemLocation::Left),
        LayoutPosition::Middle => Some(StatusBarItemLocation::Middle),
        LayoutPosition::Right => Some(StatusBarItemLocation::Right),
        // No status control registers at Bottom; that is a dock, not a bar zone.
        LayoutPosition::Bottom => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq, RegisterSetting)]
pub struct WorkspaceLayoutSettings {
    pub component_positions: collections::HashMap<String, WorkspaceComponentPosition>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorkspaceComponentPosition {
    pub position: Option<settings::DockPosition>,
    pub hidden: bool,
    pub pinned: bool,
}

impl WorkspaceLayoutSettings {
    pub fn component(&self, panel_key: &str) -> WorkspaceComponentPosition {
        self.component_positions
            .get(panel_key)
            .copied()
            .unwrap_or_default()
    }
}

impl Settings for WorkspaceLayoutSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let layout = content.workspace_layout.clone().unwrap_or_default();
        WorkspaceLayoutSettings {
            component_positions: layout
                .component_positions
                .into_iter()
                .map(|(key, entry)| {
                    (
                        key,
                        WorkspaceComponentPosition {
                            position: entry.position,
                            hidden: entry.hidden.unwrap_or(false),
                            pinned: entry.pinned.unwrap_or(false),
                        },
                    )
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod status_bar_settings_tests {
    use super::*;

    fn settings_with(
        entries: &[(&str, StatusBarItemLocation)],
        active_encoding_button: EncodingDisplayOptions,
    ) -> StatusBarSettings {
        StatusBarSettings {
            show: true,
            show_active_file: false,
            active_language_button: true,
            cursor_position_button: true,
            line_endings_button: false,
            active_encoding_button,
            item_locations: entries
                .iter()
                .map(|(id, location)| (id.to_string(), *location))
                .collect(),
        }
    }

    #[test]
    fn an_entry_naming_a_workspace_component_is_ignored() {
        // Honouring this would move GitPanel's icon while the panel stayed put —
        // the icon-only model human Check 3 disproved. Components are placed
        // through `workspace_layout.component_positions` instead.
        let settings = settings_with(&[("GitPanel", StatusBarItemLocation::Left)], NON_UTF8);

        assert_eq!(settings.status_control_location("GitPanel"), None);
        assert!(
            !settings.item_enabled("GitPanel", false),
            "a component entry granted a status control's enabled state"
        );
        assert!(
            settings.item_enabled("GitPanel", true),
            "a component entry overrode the legacy value it should have ignored"
        );
    }

    #[test]
    fn hiding_a_component_through_the_status_bar_setting_does_not_hide_it() {
        // The discriminating half of the ignore rule: `Hidden` is the one value
        // that would still have an effect if the entry leaked through.
        let settings = settings_with(&[("ProjectPanel", StatusBarItemLocation::Hidden)], NON_UTF8);

        assert_eq!(settings.status_control_location("ProjectPanel"), None);
        assert!(
            settings.item_enabled("ProjectPanel", true),
            "a stale `hidden` entry reached a component"
        );
    }

    #[test]
    fn an_unrecognised_id_keeps_its_placement() {
        // An extension's control is unknown to the registry while its crate is
        // absent. Preserving the entry is what lets the placement survive.
        let settings = settings_with(
            &[("some_extension_control", StatusBarItemLocation::Middle)],
            NON_UTF8,
        );

        assert_eq!(
            settings.status_control_location("some_extension_control"),
            Some(StatusBarItemLocation::Middle)
        );
        assert_eq!(
            settings.status_control_location("another_unknown_control"),
            None,
            "an unknown id with no entry should not be given an invented placement"
        );
    }

    #[test]
    fn an_explicit_placement_beats_the_legacy_setting_in_both_directions() {
        let placed = settings_with(
            &[("project_search", StatusBarItemLocation::Right)],
            NON_UTF8,
        );
        assert!(
            placed.item_enabled("project_search", false),
            "placing an item did not enable it over a legacy off"
        );

        let hidden = settings_with(
            &[("project_search", StatusBarItemLocation::Hidden)],
            NON_UTF8,
        );
        assert!(
            !hidden.item_enabled("project_search", true),
            "hiding an item did not disable it over a legacy on"
        );
    }

    #[test]
    fn with_no_entry_the_legacy_setting_decides() {
        let settings = settings_with(&[], NON_UTF8);
        assert!(settings.item_enabled("project_search", true));
        assert!(!settings.item_enabled("project_search", false));
    }

    #[test]
    fn registered_defaults_come_from_the_registry() {
        let settings = settings_with(&[], NON_UTF8);

        assert_eq!(
            settings.status_control_location("project_search"),
            Some(StatusBarItemLocation::Left)
        );
        assert_eq!(
            settings.status_control_location("cursor_position"),
            Some(StatusBarItemLocation::Right)
        );
        // A factory-hidden control still *belongs* somewhere. Whether it shows is
        // a separate question — folding it in here would mean the setting that
        // turns it back on could never outrank its own registered default.
        assert_eq!(
            settings.status_control_location("active_file"),
            Some(StatusBarItemLocation::Left)
        );
        assert_eq!(
            settings.status_control_location("line_endings"),
            Some(StatusBarItemLocation::Right)
        );
    }

    #[test]
    fn a_legacy_setting_turned_on_can_still_show_a_factory_hidden_control() {
        // `active_file` and `line_endings` ship hidden, and `show_active_file` /
        // `line_endings_button` are how a user asks for them back. A registered
        // default that outranked those would make the settings inert.
        let settings = settings_with(&[], NON_UTF8);

        assert_eq!(
            settings.resolved_status_control_location("active_file", true),
            Some(StatusBarItemLocation::Left),
            "a factory-hidden control could not be turned back on"
        );
        assert_eq!(
            settings.resolved_status_control_location("active_file", false),
            Some(StatusBarItemLocation::Hidden)
        );
        assert!(settings.item_enabled("active_file", true));
        assert!(!settings.item_enabled("active_file", false));
    }

    #[test]
    fn the_registry_and_the_legacy_defaults_agree_on_what_starts_hidden() {
        // Two sources of truth for one visible state is a defect; these are the two
        // that could drift apart silently, so pin them against each other.
        let defaults = settings_with(&[], NON_UTF8);
        assert!(!defaults.show_active_file);
        assert!(!defaults.line_endings_button);

        // The registry says these two ship hidden; `StatusBarSettings` says their
        // legacy booleans default off. Those are the two halves that have to agree,
        // and each lives in a different file. Resolution with the legacy value at
        // its default must land on Hidden for exactly these two.
        for (id, legacy_default) in [
            ("active_file", defaults.show_active_file),
            ("line_endings", defaults.line_endings_button),
            ("cursor_position", defaults.cursor_position_button),
            ("active_language", defaults.active_language_button),
        ] {
            let item = crate::layout_registry::get(id).expect("registered");
            let hidden = defaults.resolved_status_control_location(id, legacy_default)
                == Some(StatusBarItemLocation::Hidden);
            assert_eq!(
                hidden, item.default_hidden,
                "{id}: registry default_hidden is {}, but resolving with its legacy default gives hidden = {hidden}",
                item.default_hidden
            );
        }

        // The legacy booleans for the remaining controls live in other crates, so
        // this crate cannot read them. What it can pin is that no *other* status
        // control claims to ship hidden — the list above is the whole of it.
        let factory_hidden: Vec<_> = crate::layout_registry::all()
            .iter()
            .filter(|item| {
                item.kind == crate::layout_registry::LayoutItemKind::StatusControl
                    && item.default_hidden
            })
            .map(|item| item.id)
            .collect();
        assert_eq!(
            factory_hidden,
            vec!["active_file", "line_endings"],
            "a status control started shipping hidden without its legacy default being checked here"
        );
    }

    #[test]
    fn placing_active_encoding_preserves_its_display_rule() {
        // Placement must not silently turn on an indicator the user asked to see
        // only for non-UTF-8 files.
        let placed_while_always_on = settings_with(
            &[("active_encoding", StatusBarItemLocation::Left)],
            EncodingDisplayOptions::Enabled,
        );
        assert_eq!(
            placed_while_always_on.encoding_display_option("active_encoding"),
            EncodingDisplayOptions::Enabled
        );

        // ...nor leave a deliberately placed item invisible.
        let placed_while_disabled = settings_with(
            &[("active_encoding", StatusBarItemLocation::Left)],
            EncodingDisplayOptions::Disabled,
        );
        assert_eq!(
            placed_while_disabled.encoding_display_option("active_encoding"),
            EncodingDisplayOptions::NonUtf8,
            "placing a disabled encoding indicator left it invisible"
        );

        let hidden = settings_with(
            &[("active_encoding", StatusBarItemLocation::Hidden)],
            EncodingDisplayOptions::Enabled,
        );
        assert_eq!(
            hidden.encoding_display_option("active_encoding"),
            EncodingDisplayOptions::Disabled
        );
    }

    const NON_UTF8: EncodingDisplayOptions = EncodingDisplayOptions::NonUtf8;
}
