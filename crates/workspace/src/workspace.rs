pub mod active_file_name;
pub mod dock;
pub mod history_manager;
pub mod invalid_item_view;
pub mod item;
pub mod layout_registry;
mod modal_layer;
mod multi_workspace;
#[cfg(test)]
mod multi_workspace_tests;
pub mod notifications;
pub mod pane;
pub mod pane_group;
pub(crate) mod sequence_order;
pub mod path_list {
    pub use util::path_list::{PathList, SerializedPathList};
}
pub mod path_link;
mod persistence;
pub mod searchable;
pub mod security_modal;
pub mod shared_screen;
pub use shared_screen::SharedScreen;
pub mod focus_follows_mouse;
mod status_bar;
pub mod tasks;
mod theme_preview;
mod toast_layer;
mod toolbar;
pub mod welcome;
pub mod workspace_error;
mod workspace_settings;
mod workspace_tabs;

pub use dock::{ComponentAdapter, Panel};
pub use multi_workspace::{
    CloseWorkspaceSidebar, DraggedSidebar, FocusWorkspaceSidebar, MoveProjectToNewWindow,
    MultiWorkspace, MultiWorkspaceEvent, NewThread, NextProject, NextThread, PreviousProject,
    PreviousThread, ProjectGroup, ProjectGroupKey, RemovalIntent, SaveWorkspaceConfigurationAs,
    SerializedProjectGroupState, Sidebar, SidebarEvent, SidebarHandle, SidebarRenderState,
    SidebarSide, SwitchWorkspaceConfiguration, ToggleWorkspaceSidebar, sidebar_side_context_menu,
};
pub use path_list::{PathList, SerializedPathList};
pub use remote::{
    RemoteConnectionIdentity, remote_connection_identity, same_remote_connection_identity,
};
pub use toast_layer::{ToastAction, ToastLayer, ToastView};

use anyhow::{Context as _, Result, anyhow};
use client::{
    ChannelId, Client, ErrorExt, ParticipantIndex, Status, TypedEnvelope, User, UserStore,
    proto::{self, ErrorCode, PanelId, PeerId},
};
use collections::{HashMap, HashSet, TypeIdHashMap, hash_map};
use dock::{Dock, DockPosition, PanelButtons, PanelHandle, RESIZE_HANDLE_SIZE};
use fs::Fs;
use futures::{
    Future, FutureExt, StreamExt,
    channel::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    future::{Shared, try_join_all},
};
use gpui::{
    Action, AnyEntity, AnyView, AnyWeakView, App, AsyncApp, AsyncWindowContext, Axis, Bounds,
    Context, CursorStyle, Decorations, DisplayId, DragMoveEvent, Entity, EntityId, EventEmitter,
    FocusHandle, Focusable, Global, HitboxBehavior, Hsla, KeyContext, Keystroke, ManagedView,
    MouseButton, PathPromptOptions, Point, PromptLevel, Render, ResizeEdge, Size, Stateful,
    Subscription, SystemWindowTabController, Task, TaskExt, Tiling, WeakEntity, WindowBounds,
    WindowHandle, WindowId, WindowOptions, actions, canvas, point, relative, size,
    transparent_black,
};
pub use history_manager::*;
pub use item::{
    FollowableItem, FollowableItemHandle, Item, ItemHandle, ItemSettings, PreviewTabsSettings,
    ProjectItem, SerializableItem, SerializableItemHandle, WeakItemHandle,
};
use itertools::Itertools;
use language::{Buffer, LanguageRegistry, Rope, language_settings::all_language_settings};
pub use modal_layer::*;
use node_runtime::NodeRuntime;
use notifications::{
    DetachAndPromptErr, Notifications, dismiss_app_notification,
    simple_message_notification::MessageNotification,
};
pub use pane::*;
pub use pane_group::{
    ActivePaneDecorator, HANDLE_HITBOX_SIZE, Member, PaneAxis, PaneGroup, PaneRenderContext,
    SplitDirection,
};
pub use persistence::{
    RecentWorkspace, WorkspaceConfigurationStore, WorkspaceDb, delete_unloaded_items,
    model::{
        DockData, DockStructure, ItemId, MultiWorkspaceState, SerializedMultiWorkspace,
        SerializedProjectGroup, SerializedWorkspaceLocation, SessionWorkspace,
    },
    read_serialized_multi_workspaces, workspace_configuration_referenced_workspace_ids,
};
use persistence::{SerializedWindowBounds, model::SerializedWorkspace};
use postage::stream::Stream;
use project::{
    DirectoryLister, Project, ProjectEntryId, ProjectPath, ResolvedPath, Worktree, WorktreeId,
    WorktreeSettings,
    debugger::{breakpoint_store::BreakpointStoreEvent, session::ThreadStatus},
    project_settings::ProjectSettings,
    toolchain_store::ToolchainStoreEvent,
    trusted_worktrees::{RemoteHostLocation, TrustedWorktrees, TrustedWorktreesEvent},
};
use release_channel::RbfVersion;
use remote::{
    RemoteClientDelegate, RemoteConnection, RemoteConnectionOptions,
    remote_client::ConnectionIdentifier,
};
use schemars::JsonSchema;
use serde::Deserialize;
use session::AppSession;
use settings::{
    CenteredPaddingSettings, DefaultOpenBehavior, Settings, SettingsLocation, SettingsStore,
    update_settings_file,
};

pub use layout_registry::CustomizeWorkspaceLayout;
use sqlez::{
    bindable::{Bind, Column, StaticColumnCount},
    statement::Statement,
};
use status_bar::StatusBar;
pub use status_bar::{
    HideStatusItem, StatusControlAdapter, StatusItemView, add_hide_button_entry,
    set_status_control_location,
};
use std::{
    any::TypeId,
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp,
    collections::VecDeque,
    env,
    hash::Hash,
    path::{Path, PathBuf},
    process::ExitStatus,
    rc::Rc,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, AtomicUsize},
    },
    time::Duration,
};
use task::{DebugScenario, SharedTaskContext, SpawnInTerminal};
use theme::{ActiveTheme, ClientDecorationsExt, SystemAppearance};
use theme_settings::ThemeSettings;
pub use toolbar::{
    PaneSearchBarCallbacks, Toolbar, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView,
};
pub use ui;
use ui::{Window, prelude::*};
use url::Url;
use util::{
    ResultExt, TryFutureExt,
    paths::{PathStyle, SanitizedPath},
    rel_path::RelPath,
    serde::default_true,
};
use uuid::Uuid;
pub use workspace_settings::{
    AccessibleMode, AutosaveSetting, BottomDockLayout, EncodingDisplayOptions, FocusFollowsMouse,
    RestoreOnStartupBehavior, StatusBarItemLocation, StatusBarSettings, TabBarSettings,
    WorkspaceLayoutSettings, WorkspaceSettings, observe_accessible_mode,
};
use zed_actions::{Spawn, feedback::FileBugReport, theme::ToggleMode};

use crate::{item::ItemBufferKind, notifications::NotificationId};
use crate::{
    persistence::{
        SerializedAxis,
        model::{SerializedItem, SerializedPane, SerializedPaneGroup},
    },
    security_modal::SecurityModal,
};

pub const SERIALIZATION_THROTTLE_TIME: Duration = Duration::from_millis(200);

#[derive(Clone)]
pub(crate) struct ShutdownSerializationSnapshot {
    window_bounds: SerializedWindowBounds,
    display: Option<Uuid>,
    docks: DockStructure,
}

static ZED_WINDOW_SIZE: LazyLock<Option<Size<Pixels>>> = LazyLock::new(|| {
    env::var("ZED_WINDOW_SIZE")
        .ok()
        .as_deref()
        .and_then(parse_pixel_size_env_var)
});

static ZED_WINDOW_POSITION: LazyLock<Option<Point<Pixels>>> = LazyLock::new(|| {
    env::var("ZED_WINDOW_POSITION")
        .ok()
        .as_deref()
        .and_then(parse_pixel_position_env_var)
});

pub trait TerminalProvider {
    fn spawn(
        &self,
        task: SpawnInTerminal,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Option<Result<ExitStatus>>>;
}

pub trait DebuggerProvider {
    // `active_buffer` is used to resolve build task's name against language-specific tasks.
    fn start_session(
        &self,
        definition: DebugScenario,
        task_context: SharedTaskContext,
        active_buffer: Option<Entity<Buffer>>,
        worktree_id: Option<WorktreeId>,
        window: &mut Window,
        cx: &mut App,
    );

    fn spawn_task_or_modal(
        &self,
        workspace: &mut Workspace,
        action: &Spawn,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    );

    fn task_scheduled(&self, cx: &mut App);
    fn debug_scenario_scheduled(&self, cx: &mut App);
    fn debug_scenario_scheduled_last(&self, cx: &App) -> bool;

    fn active_thread_state(&self, cx: &App) -> Option<ThreadStatus>;
}

/// Opens a file or directory.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
pub struct Open {
    /// When true, opens in a new window. When false, adds to the current
    /// window as a new workspace (multi-workspace). When omitted, uses
    /// `default_open_behavior`.
    #[serde(default)]
    pub create_new_window: Option<bool>,
}

impl Open {
    pub const DEFAULT: Self = Self {
        create_new_window: None,
    };
}

impl Default for Open {
    fn default() -> Self {
        Self::DEFAULT
    }
}

actions!(
    workspace,
    [
        /// Activates the next pane in the workspace.
        ActivateNextPane,
        /// Activates the previous pane in the workspace.
        ActivatePreviousPane,
        /// Activates the last pane in the workspace.
        ActivateLastPane,
        /// Moves focus to the next major region of the window (editor, open
        /// panels, status bar), cycling and wrapping around. Intended as a
        /// discoverable, screen-reader-friendly way to navigate the window.
        FocusNextPart,
        /// Moves focus to the previous major region of the window. See
        /// [`FocusNextPart`].
        FocusPreviousPart,
        /// Switches to the next window.
        ActivateNextWindow,
        /// Switches to the previous window.
        ActivatePreviousWindow,
        /// Adds a folder to the current project.
        AddFolderToProject,
        /// Clears all bookmarks in the project.
        ClearBookmarks,
        /// Clears all notifications.
        ClearAllNotifications,
        /// Clears all navigation history, including forward/backward navigation, recently opened files, and recently closed tabs. **This action is irreversible**.
        ClearNavigationHistory,
        /// Closes the active dock.
        CloseActiveDock,
        /// Closes all docks.
        CloseAllDocks,
        /// Toggles all docks.
        ToggleAllDocks,
        /// Closes the current window.
        CloseWindow,
        /// Closes the current project.
        CloseProject,
        /// Opens the feedback dialog.
        Feedback,
        /// Follows the next collaborator in the session.
        FollowNextCollaborator,
        /// Moves the focused panel to the next position.
        MoveFocusedPanelToNextPosition,
        /// Creates a new file.
        NewFile,
        /// Creates a new file in a vertical split.
        NewFileSplitVertical,
        /// Creates a new file in a horizontal split.
        NewFileSplitHorizontal,
        /// Opens a new search.
        NewSearch,
        /// Opens a new window.
        NewWindow,
        /// Opens multiple files.
        OpenFiles,
        /// Opens the current location in terminal.
        OpenInTerminal,
        /// Opens the component preview.
        OpenComponentPreview,
        /// Reloads the active item.
        ReloadActiveItem,
        /// Reopens the most recently dismissed picker in the current window.
        ReopenLastPicker,
        /// Resets the active dock to its default size.
        ResetActiveDockSize,
        /// Resets all open docks to their default sizes.
        ResetOpenDocksSize,
        /// Resets all panes in the center group to equal sizes, preserving the split layout.
        ResetPaneSizes,
        /// Reloads the application
        Reload,
        /// Formats and saves the current file, regardless of the format_on_save setting.
        FormatAndSave,
        /// Saves the current file with a new name.
        SaveAs,
        /// Saves without formatting.
        SaveWithoutFormat,
        /// Shuts down all debug adapters.
        ShutdownDebugAdapters,
        /// Suppresses the current notification.
        SuppressNotification,
        /// Toggles the bottom dock.
        ToggleBottomDock,
        /// Toggles centered layout mode.
        ToggleCenteredLayout,
        /// Toggles edit prediction feature globally for all files.
        ToggleEditPrediction,
        /// Toggles the left dock.
        ToggleLeftDock,
        /// Toggles the right dock.
        ToggleRightDock,
        /// Toggles zoom on the active pane.
        ToggleZoom,
        /// Toggles maximizing the active editor pane within the center area,
        /// hiding other split panes but leaving docks/panels unaffected.
        ToggleEditorZoom,
        /// Toggles read-only mode for the active item (if supported by that item).
        ToggleReadOnlyFile,
        /// Zooms in on the active pane.
        ZoomIn,
        /// Zooms out of the active pane.
        ZoomOut,
        /// If any worktrees are in restricted mode, shows a modal with possible actions.
        /// If the modal is shown already, closes it without trusting any worktree.
        ToggleWorktreeSecurity,
        /// Clears all trusted worktrees, placing them in restricted mode on next open.
        /// Requires restart to take effect on already opened projects.
        ClearTrustedWorktrees,
        /// Stops following a collaborator.
        Unfollow,
        /// Restores the banner.
        RestoreBanner,
        /// Toggles expansion of the selected item.
        ToggleExpandItem,
    ]
);

/// Activates a specific pane by its index.
#[derive(Clone, Deserialize, PartialEq, JsonSchema, Action)]
#[action(namespace = workspace)]
pub struct ActivatePane(pub usize);

/// Moves an item to a specific pane by index.
#[derive(Clone, Deserialize, PartialEq, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct MoveItemToPane {
    #[serde(default = "default_1")]
    pub destination: usize,
    #[serde(default = "default_true")]
    pub focus: bool,
    #[serde(default)]
    pub clone: bool,
}

fn default_1() -> usize {
    1
}

/// Moves an item to a pane in the specified direction.
#[derive(Clone, Deserialize, PartialEq, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct MoveItemToPaneInDirection {
    #[serde(default = "default_right")]
    pub direction: SplitDirection,
    #[serde(default = "default_true")]
    pub focus: bool,
    #[serde(default)]
    pub clone: bool,
}

/// Creates a new file in a split of the desired direction.
#[derive(Clone, Deserialize, PartialEq, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct NewFileSplit(pub SplitDirection);

fn default_right() -> SplitDirection {
    SplitDirection::Right
}

/// Saves all open files in the workspace.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct SaveAll {
    #[serde(default)]
    pub save_intent: Option<SaveIntent>,
}

/// Saves the current file with the specified options.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct Save {
    #[serde(default)]
    pub save_intent: Option<SaveIntent>,
}

/// Moves Focus to the central panes in the workspace.
#[derive(Clone, Debug, PartialEq, Eq, Action)]
#[action(namespace = workspace)]
pub struct FocusCenterPane;

///  Closes all items and panes in the workspace.
#[derive(Clone, PartialEq, Debug, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct CloseAllItemsAndPanes {
    #[serde(default)]
    pub save_intent: Option<SaveIntent>,
}

/// Closes all inactive tabs and panes in the workspace.
#[derive(Clone, PartialEq, Debug, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct CloseInactiveTabsAndPanes {
    #[serde(default)]
    pub save_intent: Option<SaveIntent>,
}

/// Closes the active item across all panes.
#[derive(Clone, PartialEq, Debug, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct CloseItemInAllPanes {
    #[serde(default)]
    pub save_intent: Option<SaveIntent>,
    #[serde(default)]
    pub close_pinned: bool,
}

/// Sends a sequence of keystrokes to the active element.
#[derive(Clone, Deserialize, PartialEq, JsonSchema, Action)]
#[action(namespace = workspace)]
pub struct SendKeystrokes(pub String);

actions!(
    project_symbols,
    [
        /// Toggles the project symbols search.
        #[action(name = "Toggle")]
        ToggleProjectSymbols
    ]
);

/// Toggles the file finder interface.
#[derive(Default, PartialEq, Eq, Clone, Deserialize, JsonSchema, Action)]
#[action(namespace = file_finder, name = "Toggle")]
#[serde(deny_unknown_fields)]
pub struct ToggleFileFinder {
    #[serde(default)]
    pub separate_history: bool,
    #[serde(default)]
    pub include_ignored: Option<bool>,
}

/// Opens a new terminal in the center.
#[derive(Default, PartialEq, Eq, Clone, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct NewCenterTerminal {
    /// If true, creates a local terminal even in remote projects.
    #[serde(default)]
    pub local: bool,
}

/// Opens a new terminal.
#[derive(Default, PartialEq, Eq, Clone, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct NewTerminal {
    /// If true, creates a local terminal even in remote projects.
    #[serde(default)]
    pub local: bool,
}

/// Increases size of a currently focused dock by a given amount of pixels.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct IncreaseActiveDockSize {
    /// For 0px parameter, uses UI font size value.
    #[serde(default)]
    pub px: u32,
}

/// Decreases size of a currently focused dock by a given amount of pixels.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct DecreaseActiveDockSize {
    /// For 0px parameter, uses UI font size value.
    #[serde(default)]
    pub px: u32,
}

/// Increases size of all currently visible docks uniformly, by a given amount of pixels.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct IncreaseOpenDocksSize {
    /// For 0px parameter, uses UI font size value.
    #[serde(default)]
    pub px: u32,
}

/// Decreases size of all currently visible docks uniformly, by a given amount of pixels.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct DecreaseOpenDocksSize {
    /// For 0px parameter, uses UI font size value.
    #[serde(default)]
    pub px: u32,
}

actions!(
    workspace,
    [
        /// Activates the pane to the left.
        ActivatePaneLeft,
        /// Activates the pane to the right.
        ActivatePaneRight,
        /// Activates the pane above.
        ActivatePaneUp,
        /// Activates the pane below.
        ActivatePaneDown,
        /// Swaps the current pane with the one to the left.
        SwapPaneLeft,
        /// Swaps the current pane with the one to the right.
        SwapPaneRight,
        /// Swaps the current pane with the one above.
        SwapPaneUp,
        /// Swaps the current pane with the one below.
        SwapPaneDown,
        // Swaps the current pane with the first available adjacent pane (searching in order: below, above, right, left) and activates that pane.
        SwapPaneAdjacent,
        /// Move the current pane to be at the far left.
        MovePaneLeft,
        /// Move the current pane to be at the far right.
        MovePaneRight,
        /// Move the current pane to be at the very top.
        MovePaneUp,
        /// Move the current pane to be at the very bottom.
        MovePaneDown,
    ]
);

#[derive(PartialEq, Eq, Debug)]
pub enum CloseIntent {
    /// Quit the program entirely.
    Quit,
    /// Close a window.
    CloseWindow,
    /// Replace the workspace in an existing window.
    ReplaceWindow,
}

#[derive(Clone)]
pub struct Toast {
    id: NotificationId,
    message: Cow<'static, str>,
    autohide: bool,
    on_click: Option<(Cow<'static, str>, Arc<dyn Fn(&mut Window, &mut App)>)>,
}

impl Toast {
    pub fn new<I: Into<Cow<'static, str>>>(id: NotificationId, message: I) -> Self {
        Toast {
            id,
            message: message.into(),
            on_click: None,
            autohide: false,
        }
    }

    pub fn on_click<F, M>(mut self, message: M, on_click: F) -> Self
    where
        M: Into<Cow<'static, str>>,
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.on_click = Some((message.into(), Arc::new(on_click)));
        self
    }

    pub fn autohide(mut self) -> Self {
        self.autohide = true;
        self
    }
}

impl PartialEq for Toast {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.message == other.message
            && self.on_click.is_some() == other.on_click.is_some()
    }
}

/// Opens a new terminal with the specified working directory.
#[derive(Debug, Default, Clone, Deserialize, PartialEq, JsonSchema, Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct OpenTerminal {
    pub working_directory: PathBuf,
    /// If true, creates a local terminal even in remote projects.
    #[serde(default)]
    pub local: bool,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct WorkspaceId(i64);

impl WorkspaceId {
    pub fn from_i64(value: i64) -> Self {
        Self(value)
    }
}

impl StaticColumnCount for WorkspaceId {}
impl Bind for WorkspaceId {
    fn bind(&self, statement: &Statement, start_index: i32) -> Result<i32> {
        self.0.bind(statement, start_index)
    }
}
impl Column for WorkspaceId {
    fn column(statement: &mut Statement, start_index: i32) -> Result<(Self, i32)> {
        i64::column(statement, start_index)
            .map(|(i, next_index)| (Self(i), next_index))
            .with_context(|| format!("Failed to read WorkspaceId at index {start_index}"))
    }
}
impl From<WorkspaceId> for i64 {
    fn from(val: WorkspaceId) -> Self {
        val.0
    }
}

fn prompt_and_open_paths(
    app_state: Arc<AppState>,
    options: PathPromptOptions,
    create_new_window: bool,
    cx: &mut App,
) {
    if let Some(workspace_window) =
        workspace_windows_for_location(&SerializedWorkspaceLocation::Local, cx)
            .into_iter()
            .next()
    {
        workspace_window
            .update(cx, |multi_workspace, window, cx| {
                let workspace = multi_workspace.workspace().clone();
                workspace.update(cx, |workspace, cx| {
                    prompt_for_open_path_and_open(
                        workspace,
                        app_state,
                        options,
                        create_new_window,
                        window,
                        cx,
                    );
                });
            })
            .ok();
    } else {
        let task = Workspace::new_local(
            Vec::new(),
            app_state.clone(),
            None,
            None,
            None,
            OpenMode::Activate,
            cx,
        );
        cx.spawn(async move |cx| {
            let OpenResult { window, .. } = task.await?;
            window.update(cx, |multi_workspace, window, cx| {
                window.activate_window();
                let workspace = multi_workspace.workspace().clone();
                workspace.update(cx, |workspace, cx| {
                    prompt_for_open_path_and_open(
                        workspace,
                        app_state,
                        options,
                        create_new_window,
                        window,
                        cx,
                    );
                });
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }
}

pub fn prompt_for_open_path_and_open(
    workspace: &mut Workspace,
    app_state: Arc<AppState>,
    options: PathPromptOptions,
    create_new_window: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let paths = workspace.prompt_for_open_path(
        options,
        DirectoryLister::Local(workspace.project().clone(), app_state.fs.clone()),
        window,
        cx,
    );
    let multi_workspace_handle = window.window_handle().downcast::<MultiWorkspace>();
    cx.spawn_in(window, async move |this, cx| {
        let Some(paths) = paths.await.log_err().flatten() else {
            return;
        };
        if !create_new_window {
            if let Some(handle) = multi_workspace_handle {
                if let Some(task) = handle
                    .update(cx, |multi_workspace, window, cx| {
                        multi_workspace.open_project(paths, OpenMode::Activate, window, cx)
                    })
                    .log_err()
                {
                    task.await.log_err();
                }
                return;
            }
        }
        if let Some(task) = this
            .update_in(cx, |this, window, cx| {
                this.open_workspace_for_paths(OpenMode::NewWindow, paths, window, cx)
            })
            .log_err()
        {
            task.await.log_err();
        }
    })
    .detach();
}

pub fn init(app_state: Arc<AppState>, cx: &mut App) {
    component::init();
    theme_preview::init(cx);
    toast_layer::init(cx);
    history_manager::init(app_state.fs.clone(), cx);
    WorkspaceConfigurationStore::init(cx);

    // The scrub's other trigger. `set_component_hidden` scrubs before it writes,
    // which a settings-file edit cannot do because the write has already
    // happened by the time we see it. Both may fire for one unhide; the scrub is
    // idempotent so that costs nothing.
    workspace_settings::observe_components_unhidden(cx, |panel_keys, cx| {
        let db = WorkspaceDb::global(cx);
        cx.background_spawn(async move {
            for panel_key in panel_keys {
                db.scrub_open_component(panel_key).await.log_err();
            }
        })
        .detach();
    })
    .detach();

    cx.on_action(|_: &CloseWindow, cx| Workspace::close_global(cx))
        .on_action(|_: &Reload, cx| reload(cx))
        .on_action(|action: &Open, cx: &mut App| {
            let app_state = AppState::global(cx);
            prompt_and_open_paths(
                app_state,
                PathPromptOptions {
                    files: true,
                    directories: true,
                    multiple: true,
                    prompt: None,
                },
                action.create_new_window.unwrap_or_else(|| {
                    matches!(
                        WorkspaceSettings::get_global(cx).default_open_behavior,
                        DefaultOpenBehavior::NewWindow
                    )
                }),
                cx,
            );
        })
        .on_action(|_: &OpenFiles, cx: &mut App| {
            let directories = cx.can_select_mixed_files_and_dirs();
            let app_state = AppState::global(cx);
            prompt_and_open_paths(
                app_state,
                PathPromptOptions {
                    files: true,
                    directories,
                    multiple: true,
                    prompt: None,
                },
                true,
                cx,
            );
        });
}

type BuildProjectItemFn =
    fn(AnyEntity, Entity<Project>, Option<&Pane>, &mut Window, &mut App) -> Box<dyn ItemHandle>;

type BuildProjectItemForPathFn =
    fn(
        &Entity<Project>,
        &ProjectPath,
        &mut Window,
        &mut App,
    ) -> Option<Task<Result<(Option<ProjectEntryId>, WorkspaceItemBuilder)>>>;

#[derive(Clone, Default)]
struct ProjectItemRegistry {
    build_project_item_fns_by_type: TypeIdHashMap<BuildProjectItemFn>,
    build_project_item_for_path_fns: Vec<BuildProjectItemForPathFn>,
}

impl ProjectItemRegistry {
    fn register<T: ProjectItem>(&mut self) {
        self.build_project_item_fns_by_type.insert(
            TypeId::of::<T::Item>(),
            |item, project, pane, window, cx| {
                let item = item.downcast().unwrap();
                Box::new(cx.new(|cx| T::for_project_item(project, pane, item, window, cx)))
                    as Box<dyn ItemHandle>
            },
        );
        self.build_project_item_for_path_fns
            .push(|project, project_path, window, cx| {
                let project_path = project_path.clone();
                let is_file = project
                    .read(cx)
                    .entry_for_path(&project_path, cx)
                    .is_some_and(|entry| entry.is_file());
                let entry_abs_path = project.read(cx).absolute_path(&project_path, cx);
                let is_local = project.read(cx).is_local();
                let project_item =
                    <T::Item as project::ProjectItem>::try_open(project, &project_path, cx)?;
                let project = project.clone();
                Some(window.spawn(cx, async move |cx| {
                    match project_item.await.with_context(|| {
                        format!(
                            "opening project path {:?}",
                            entry_abs_path.as_deref().unwrap_or(&project_path.path.as_std_path())
                        )
                    }) {
                        Ok(project_item) => {
                            let project_item = project_item;
                            let project_entry_id: Option<ProjectEntryId> =
                                project_item.read_with(cx, project::ProjectItem::entry_id);
                            let build_workspace_item = Box::new(
                                |pane: &mut Pane, window: &mut Window, cx: &mut Context<Pane>| {
                                    Box::new(cx.new(|cx| {
                                        T::for_project_item(
                                            project,
                                            Some(pane),
                                            project_item,
                                            window,
                                            cx,
                                        )
                                    })) as Box<dyn ItemHandle>
                                },
                            ) as Box<_>;
                            Ok((project_entry_id, build_workspace_item))
                        }
                        Err(e) => {
                            log::warn!("Failed to open a project item: {e:#}");
                            if e.error_code() == ErrorCode::Internal {
                                if let Some(abs_path) =
                                    entry_abs_path.as_deref().filter(|_| is_file)
                                {
                                    if let Some(broken_project_item_view) =
                                        cx.update(|window, cx| {
                                            T::for_broken_project_item(
                                                abs_path, is_local, &e, window, cx,
                                            )
                                        })?
                                    {
                                        let build_workspace_item = Box::new(
                                            move |_: &mut Pane, _: &mut Window, cx: &mut Context<Pane>| {
                                                cx.new(|_| broken_project_item_view).boxed_clone()
                                            },
                                        )
                                        as Box<_>;
                                        return Ok((None, build_workspace_item));
                                    }
                                }
                            }
                            Err(e)
                        }
                    }
                }))
            });
    }

    fn open_path(
        &self,
        project: &Entity<Project>,
        path: &ProjectPath,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<(Option<ProjectEntryId>, WorkspaceItemBuilder)>> {
        let Some(open_project_item) = self
            .build_project_item_for_path_fns
            .iter()
            .rev()
            .find_map(|open_project_item| open_project_item(project, path, window, cx))
        else {
            return Task::ready(Err(anyhow!("cannot open file {:?}", path.path)));
        };
        open_project_item
    }

    fn build_item<T: project::ProjectItem>(
        &self,
        item: Entity<T>,
        project: Entity<Project>,
        pane: Option<&Pane>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Box<dyn ItemHandle>> {
        let build = self
            .build_project_item_fns_by_type
            .get(&TypeId::of::<T>())?;
        Some(build(item.into_any(), project, pane, window, cx))
    }
}

type WorkspaceItemBuilder =
    Box<dyn FnOnce(&mut Pane, &mut Window, &mut Context<Pane>) -> Box<dyn ItemHandle>>;

impl Global for ProjectItemRegistry {}

/// Registers a [ProjectItem] for the app. When opening a file, all the registered
/// items will get a chance to open the file, starting from the project item that
/// was added last.
pub fn register_project_item<I: ProjectItem>(cx: &mut App) {
    cx.default_global::<ProjectItemRegistry>().register::<I>();
}

#[derive(Default)]
pub struct FollowableViewRegistry(TypeIdHashMap<FollowableViewDescriptor>);

struct FollowableViewDescriptor {
    from_state_proto: fn(
        Entity<Workspace>,
        ViewId,
        &mut Option<proto::view::Variant>,
        &mut Window,
        &mut App,
    ) -> Option<Task<Result<Box<dyn FollowableItemHandle>>>>,
    to_followable_view: fn(&AnyView) -> Box<dyn FollowableItemHandle>,
}

impl Global for FollowableViewRegistry {}

impl FollowableViewRegistry {
    pub fn register<I: FollowableItem>(cx: &mut App) {
        cx.default_global::<Self>().0.insert(
            TypeId::of::<I>(),
            FollowableViewDescriptor {
                from_state_proto: |workspace, id, state, window, cx| {
                    I::from_state_proto(workspace, id, state, window, cx).map(|task| {
                        cx.foreground_executor()
                            .spawn(async move { Ok(Box::new(task.await?) as Box<_>) })
                    })
                },
                to_followable_view: |view| Box::new(view.clone().downcast::<I>().unwrap()),
            },
        );
    }

    pub fn from_state_proto(
        workspace: Entity<Workspace>,
        view_id: ViewId,
        mut state: Option<proto::view::Variant>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Task<Result<Box<dyn FollowableItemHandle>>>> {
        cx.update_default_global(|this: &mut Self, cx| {
            this.0.values().find_map(|descriptor| {
                (descriptor.from_state_proto)(workspace.clone(), view_id, &mut state, window, cx)
            })
        })
    }

    pub fn to_followable_view(
        view: impl Into<AnyView>,
        cx: &App,
    ) -> Option<Box<dyn FollowableItemHandle>> {
        let this = cx.try_global::<Self>()?;
        let view = view.into();
        let descriptor = this.0.get(&view.entity_type())?;
        Some((descriptor.to_followable_view)(&view))
    }
}

#[derive(Copy, Clone)]
struct SerializableItemDescriptor {
    deserialize: fn(
        Entity<Project>,
        WeakEntity<Workspace>,
        WorkspaceId,
        ItemId,
        &mut Window,
        &mut Context<Pane>,
    ) -> Task<Result<Box<dyn ItemHandle>>>,
    cleanup: fn(WorkspaceId, Vec<ItemId>, &mut Window, &mut App) -> Task<Result<()>>,
    view_to_serializable_item: fn(AnyView) -> Box<dyn SerializableItemHandle>,
}

#[derive(Default)]
struct SerializableItemRegistry {
    descriptors_by_kind: HashMap<Arc<str>, SerializableItemDescriptor>,
    descriptors_by_type: TypeIdHashMap<SerializableItemDescriptor>,
}

impl Global for SerializableItemRegistry {}

impl SerializableItemRegistry {
    fn deserialize(
        item_kind: &str,
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        workspace_id: WorkspaceId,
        item_item: ItemId,
        window: &mut Window,
        cx: &mut Context<Pane>,
    ) -> Task<Result<Box<dyn ItemHandle>>> {
        let Some(descriptor) = Self::descriptor(item_kind, cx) else {
            return Task::ready(Err(anyhow!(
                "cannot deserialize {}, descriptor not found",
                item_kind
            )));
        };

        (descriptor.deserialize)(project, workspace, workspace_id, item_item, window, cx)
    }

    fn cleanup(
        item_kind: &str,
        workspace_id: WorkspaceId,
        loaded_items: Vec<ItemId>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let Some(descriptor) = Self::descriptor(item_kind, cx) else {
            return Task::ready(Err(anyhow!(
                "cannot cleanup {}, descriptor not found",
                item_kind
            )));
        };

        (descriptor.cleanup)(workspace_id, loaded_items, window, cx)
    }

    fn view_to_serializable_item_handle(
        view: AnyView,
        cx: &App,
    ) -> Option<Box<dyn SerializableItemHandle>> {
        let this = cx.try_global::<Self>()?;
        let descriptor = this.descriptors_by_type.get(&view.entity_type())?;
        Some((descriptor.view_to_serializable_item)(view))
    }

    fn descriptor(item_kind: &str, cx: &App) -> Option<SerializableItemDescriptor> {
        let this = cx.try_global::<Self>()?;
        this.descriptors_by_kind.get(item_kind).copied()
    }
}

pub fn register_serializable_item<I: SerializableItem>(cx: &mut App) {
    let serialized_item_kind = I::serialized_item_kind();

    let registry = cx.default_global::<SerializableItemRegistry>();
    let descriptor = SerializableItemDescriptor {
        deserialize: |project, workspace, workspace_id, item_id, window, cx| {
            let task = I::deserialize(project, workspace, workspace_id, item_id, window, cx);
            cx.foreground_executor()
                .spawn(async { Ok(Box::new(task.await?) as Box<_>) })
        },
        cleanup: |workspace_id, loaded_items, window, cx| {
            I::cleanup(workspace_id, loaded_items, window, cx)
        },
        view_to_serializable_item: |view| Box::new(view.downcast::<I>().unwrap()),
    };
    registry
        .descriptors_by_kind
        .insert(Arc::from(serialized_item_kind), descriptor);
    registry
        .descriptors_by_type
        .insert(TypeId::of::<I>(), descriptor);
}

pub struct AppState {
    pub languages: Arc<LanguageRegistry>,
    pub client: Arc<Client>,
    pub user_store: Entity<UserStore>,
    pub workspace_store: Entity<WorkspaceStore>,
    pub fs: Arc<dyn fs::Fs>,
    pub build_window_options: fn(Option<Uuid>, &mut App) -> WindowOptions,
    pub node_runtime: NodeRuntime,
    pub session: Entity<AppSession>,
}

struct GlobalAppState(Arc<AppState>);

impl Global for GlobalAppState {}

/// Tracks worktree creation progress for the workspace.
/// Read by the title bar to show a loading indicator on the worktree button.
#[derive(Default)]
pub struct ActiveWorktreeCreation {
    pub label: Option<SharedString>,
    pub is_switch: bool,
}

/// Captured workspace state used when switching between worktrees.
/// Stores the layout and open files so they can be restored in the new workspace.
pub struct PreviousWorkspaceState {
    pub dock_structure: DockStructure,
    pub open_file_paths: Vec<PathBuf>,
    pub active_file_path: Option<PathBuf>,
    pub focused_dock: Option<DockPosition>,
}

pub struct WorkspaceStore {
    workspaces: HashSet<(gpui::AnyWindowHandle, WeakEntity<Workspace>)>,
    client: Arc<Client>,
    _subscriptions: Vec<client::Subscription>,
}

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub enum CollaboratorId {
    PeerId(PeerId),
    Agent,
}

impl From<PeerId> for CollaboratorId {
    fn from(peer_id: PeerId) -> Self {
        CollaboratorId::PeerId(peer_id)
    }
}

impl From<&PeerId> for CollaboratorId {
    fn from(peer_id: &PeerId) -> Self {
        CollaboratorId::PeerId(*peer_id)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Follower {
    project_id: Option<u64>,
    peer_id: PeerId,
}

impl AppState {
    #[track_caller]
    pub fn global(cx: &App) -> Arc<Self> {
        cx.global::<GlobalAppState>().0.clone()
    }
    pub fn try_global(cx: &App) -> Option<Arc<Self>> {
        cx.try_global::<GlobalAppState>()
            .map(|state| state.0.clone())
    }
    pub fn set_global(state: Arc<AppState>, cx: &mut App) {
        cx.set_global(GlobalAppState(state));
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test(cx: &mut App) -> Arc<Self> {
        use fs::Fs;
        use node_runtime::NodeRuntime;
        use session::Session;
        use settings::SettingsStore;

        if !cx.has_global::<SettingsStore>() {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
        }

        let fs = fs::FakeFs::new(cx.background_executor().clone());
        <dyn Fs>::set_global(fs.clone(), cx);
        let languages = Arc::new(LanguageRegistry::test(cx.background_executor().clone()));
        let clock = Arc::new(clock::FakeSystemClock::new());
        let http_client = http_client::FakeHttpClient::with_404_response();
        let client = Client::new(clock, http_client, cx);
        let session = cx.new(|cx| AppSession::new(Session::test(), cx));
        let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
        let workspace_store = cx.new(|cx| WorkspaceStore::new(client.clone(), cx));

        theme_settings::init(theme::LoadThemes::JustBase, cx);
        client::init(&client, cx);

        Arc::new(Self {
            client,
            fs,
            languages,
            user_store,
            workspace_store,
            node_runtime: NodeRuntime::unavailable(),
            build_window_options: |_, _| Default::default(),
            session,
        })
    }
}

struct DelayedDebouncedEditAction {
    task: Option<Task<()>>,
    cancel_channel: Option<oneshot::Sender<()>>,
}

impl DelayedDebouncedEditAction {
    fn new() -> DelayedDebouncedEditAction {
        DelayedDebouncedEditAction {
            task: None,
            cancel_channel: None,
        }
    }

    fn fire_new<F>(
        &mut self,
        delay: Duration,
        window: &mut Window,
        cx: &mut Context<Workspace>,
        func: F,
    ) where
        F: 'static
            + Send
            + FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) -> Task<Result<()>>,
    {
        if let Some(channel) = self.cancel_channel.take() {
            _ = channel.send(());
        }

        let (sender, mut receiver) = oneshot::channel::<()>();
        self.cancel_channel = Some(sender);

        let previous_task = self.task.take();
        self.task = Some(cx.spawn_in(window, async move |workspace, cx| {
            let mut timer = cx.background_executor().timer(delay).fuse();
            if let Some(previous_task) = previous_task {
                previous_task.await;
            }

            futures::select_biased! {
                _ = receiver => return,
                    _ = timer => {}
            }

            if let Some(result) = workspace
                .update_in(cx, |workspace, window, cx| (func)(workspace, window, cx))
                .log_err()
            {
                result.await.log_err();
            }
        }));
    }
}

pub enum Event {
    PaneAdded(Entity<Pane>),
    PaneRemoved,
    ItemAdded {
        item: Box<dyn ItemHandle>,
    },
    ActiveItemChanged,
    ItemRemoved {
        item_id: EntityId,
    },
    ItemDirtyStateChanged,
    UserSavedItem {
        pane: WeakEntity<Pane>,
        item: Box<dyn WeakItemHandle>,
        save_intent: SaveIntent,
    },
    ContactRequestedJoin(u64),
    WorkspaceCreated(WeakEntity<Workspace>),
    OpenBundledFile {
        text: Cow<'static, str>,
        title: &'static str,
        language: &'static str,
    },
    ZoomChanged,
    ModalOpened,
    Activate,
    PanelAdded(AnyView),
    WorktreeCreationChanged,
}

/// Controls which types of items should be made visible in the project panel
/// when opened.
#[derive(Debug, Clone)]
pub enum OpenVisible {
    /// Make all opened items visible (both files and directories).
    All,
    /// Don't make any opened items visible.
    None,
    /// Only make opened files visible, not directories.
    OnlyFiles,
    /// Only make opened directories visible, not files.
    OnlyDirectories,
}

enum WorkspaceLocation {
    // Valid local paths or SSH project to serialize
    Location(SerializedWorkspaceLocation, PathList),
    // No valid location found hence clear session id
    DetachFromSession,
    // No valid location found to serialize
    None,
}

type PromptForNewPath = Box<
    dyn Fn(
        &mut Workspace,
        DirectoryLister,
        Option<String>,
        &mut Window,
        &mut Context<Workspace>,
    ) -> oneshot::Receiver<Option<Vec<PathBuf>>>,
>;

type PromptForOpenPath = Box<
    dyn Fn(
        &mut Workspace,
        DirectoryLister,
        &mut Window,
        &mut Context<Workspace>,
    ) -> oneshot::Receiver<Option<Vec<PathBuf>>>,
>;

#[derive(Default)]
struct DispatchingKeystrokes {
    dispatched: HashSet<Vec<Keystroke>>,
    queue: VecDeque<Keystroke>,
    task: Option<Shared<Task<()>>>,
}

/// Collects everything project-related for a certain window opened.
/// In some way, is a counterpart of a window, as the [`WindowHandle`] could be downcast into `Workspace`.
///
/// A `Workspace` usually consists of 1 or more projects, a central pane group, 3 docks and a status bar.
/// The `Workspace` owns everybody's state and serves as a default, "global context",
/// that can be used to register a global action to be triggered from any place in the window.
pub struct Workspace {
    weak_self: WeakEntity<Self>,
    workspace_actions: Vec<Box<dyn Fn(Div, &Workspace, &mut Window, &mut Context<Self>) -> Div>>,
    zoomed: Option<AnyWeakView>,
    previous_dock_drag_coordinates: Option<Point<Pixels>>,
    zoomed_position: Option<DockPosition>,
    maximized_pane: Option<WeakEntity<Pane>>,
    center: PaneGroup,
    left_dock: Entity<Dock>,
    bottom_dock: Entity<Dock>,
    right_dock: Entity<Dock>,
    panes: Vec<Entity<Pane>>,
    panes_by_item: HashMap<EntityId, WeakEntity<Pane>>,
    active_pane: Entity<Pane>,
    last_active_center_pane: Option<WeakEntity<Pane>>,
    last_active_view_id: Option<proto::ViewId>,
    status_bar: Entity<StatusBar>,
    pub(crate) modal_layer: Entity<ModalLayer>,
    toast_layer: Entity<ToastLayer>,
    titlebar_item: Option<AnyView>,
    titlebar_focus_handle: FocusHandle,
    region_focus_handles: RegionFocusHandles,
    notifications: Notifications,
    suppressed_notifications: HashSet<NotificationId>,
    project: Entity<Project>,
    follower_states: HashMap<CollaboratorId, FollowerState>,
    last_leaders_by_pane: HashMap<WeakEntity<Pane>, CollaboratorId>,
    auto_watch: AutoWatch,
    window_edited: bool,
    last_window_title: Option<String>,
    dirty_items: HashMap<EntityId, Subscription>,
    active_call: Option<(GlobalAnyActiveCall, Vec<Subscription>)>,
    leader_updates_tx: mpsc::UnboundedSender<(PeerId, proto::UpdateFollowers)>,
    database_id: Option<WorkspaceId>,
    app_state: Arc<AppState>,
    dispatching_keystrokes: Rc<RefCell<DispatchingKeystrokes>>,
    _subscriptions: Vec<Subscription>,
    _apply_leader_updates: Task<Result<()>>,
    _observe_current_user: Task<Result<()>>,
    _schedule_serialize_workspace: Option<Task<()>>,
    _serialize_workspace_task: Option<Task<()>>,
    _schedule_serialize_ssh_paths: Option<Task<()>>,
    pane_history_timestamp: Arc<AtomicUsize>,
    bounds: Bounds<Pixels>,
    pub centered_layout: bool,
    bounds_save_task_queued: Option<Task<()>>,
    /// The display this workspace's window was on when it was last observed.
    ///
    /// Compared on every bounds change so a drag within one display costs one integer
    /// comparison, and only a genuine move between displays does any work.
    last_display_id: Option<DisplayId>,
    on_prompt_for_new_path: Option<PromptForNewPath>,
    on_prompt_for_open_path: Option<PromptForOpenPath>,
    terminal_provider: Option<Box<dyn TerminalProvider>>,
    debugger_provider: Option<Arc<dyn DebuggerProvider>>,
    serializable_items_tx: UnboundedSender<ItemSerializationCommand>,
    _items_serializer: Task<Result<()>>,
    session_id: Option<String>,
    scheduled_tasks: Vec<Task<()>>,
    last_open_dock_positions: Vec<DockPosition>,
    removing: bool,
    open_in_dev_container: bool,
    _dev_container_task: Option<Task<Result<()>>>,
    _panels_task: Option<Task<Result<()>>>,
    sidebar_focus_handle: Option<FocusHandle>,
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
    detached_restore: bool,
    /// Shared with the parent `MultiWorkspace` and any sibling workspaces: holds
    /// the id of the single workspace currently presented in this OS window.
    /// `MultiWorkspace` is the only writer; workspaces only read it to decide
    /// whether they may write the shared window's title and edited indicator. We
    /// use this instead of going through the `multi_workspace` field to avoid
    /// reading it as we might end up in a double lease otherwise.
    active_workspace_id: Option<Rc<Cell<EntityId>>>,
    active_worktree_creation: ActiveWorktreeCreation,
    deferred_save_items: Vec<Box<dyn WeakItemHandle>>,
}

enum ItemSerializationCommand {
    Serialize(Box<dyn SerializableItemHandle>),
    SerializeWorkspace(oneshot::Sender<()>),
    SaveWindowBounds(oneshot::Sender<()>),
    CheckpointExact(oneshot::Sender<Result<()>>),
}

impl EventEmitter<Event> for Workspace {}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ViewId {
    pub creator: CollaboratorId,
    pub id: u64,
}

pub struct FollowerState {
    center_pane: Entity<Pane>,
    dock_pane: Option<Entity<Pane>>,
    active_view_id: Option<ViewId>,
    items_by_leader_view_id: HashMap<ViewId, FollowerView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoWatch {
    Off,
    Active { watched_peer: Option<PeerId> },
    Paused,
}

impl AutoWatch {
    pub fn enabled(&self) -> bool {
        matches!(self, AutoWatch::Active { .. } | AutoWatch::Paused)
    }
}

struct FollowerView {
    view: Box<dyn FollowableItemHandle>,
    location: Option<proto::PanelId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenMode {
    /// Open the workspace in a new window.
    NewWindow,
    /// Add to the window's multi workspace without activating it (used during deserialization).
    Add,
    /// Add to the window's multi workspace and activate it.
    #[default]
    Activate,
}

/// The geometry a mixed side dock is laid out from, gathered once so
/// `dock_size` and `dock_flex_for_size` cannot disagree about it.
///
/// Upstream a dock held one panel, so it was *either* wholly fixed *or* wholly
/// flexible, and `fixed` was zero whenever flex mattered. The multi-component
/// model puts fixed and flexible columns on the same side, which is why both
/// directions now have to name `fixed` explicitly.
pub(crate) struct DockGeometry {
    /// The workspace's own width — everything below is measured out of it.
    workspace_width: Pixels,
    /// The opposite side's fixed columns, summed. Not available to this dock's flex.
    opposite_fixed: Pixels,
    /// Flex competing with this dock's: the centre's columns plus the opposite side's.
    competing_flex: f32,
}

impl DockGeometry {
    /// Width available to flex once every fixed column on both sides is paid for.
    ///
    /// `fixed` is a parameter rather than stored state because the callers that
    /// need this most are the ones that *change* it: a column turning flexible
    /// releases its width, and a seam beside a flexible column moves width
    /// between the fixed and flexible halves of the same side.
    fn available(&self, fixed: Pixels) -> Pixels {
        (self.workspace_width - fixed - self.opposite_fixed).max(RESIZE_HANDLE_SIZE)
    }

    /// Width this dock's flexible columns occupy for a given total flex factor.
    fn flex_share(&self, fixed: Pixels, flex: f32) -> Pixels {
        (flex / (flex + self.competing_flex) * self.available(fixed)).max(RESIZE_HANDLE_SIZE)
    }

    /// `flex_share`, but a side with no flexible column occupies nothing rather
    /// than the handle's 6px — the floor exists to keep a *live* flexible column
    /// grabbable, and there is none to keep.
    fn flex_share_or_zero(&self, fixed: Pixels, flex: f32) -> Pixels {
        if flex <= 0.0 {
            return Pixels::ZERO;
        }
        self.flex_share(fixed, flex)
    }

    /// The inverse of `flex_share`: the flex factor whose share is `share` when
    /// the dock's fixed columns total `fixed`.
    fn flex_for_share(&self, fixed: Pixels, share: Pixels) -> f32 {
        let share = share.max(Pixels::ZERO);
        let headroom = (self.available(fixed) - share).max(px(1.));
        (share * self.competing_flex / headroom).max(0.0)
    }

    /// The flex factor that lays the dock out at `size` overall, its fixed
    /// columns included. Subtracting them is the step the pre-mixed math had no
    /// reason to take.
    fn flex_for_size(&self, fixed: Pixels, size: Pixels) -> f32 {
        self.flex_for_share(fixed, size - fixed)
    }
}

impl Workspace {
    pub fn new(
        workspace_id: Option<WorkspaceId>,
        project: Entity<Project>,
        app_state: Arc<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(trusted_worktrees) = TrustedWorktrees::try_get_global(cx) {
            cx.subscribe(&trusted_worktrees, |_, worktrees_store, e, cx| {
                if let TrustedWorktreesEvent::Trusted(..) = e {
                    // Do not persist auto trusted worktrees
                    if !ProjectSettings::get_global(cx).session.trust_all_worktrees {
                        worktrees_store.update(cx, |worktrees_store, cx| {
                            worktrees_store.schedule_serialization(
                                cx,
                                |new_trusted_worktrees, cx| {
                                    let timeout =
                                        cx.background_executor().timer(SERIALIZATION_THROTTLE_TIME);
                                    let db = WorkspaceDb::global(cx);
                                    cx.background_spawn(async move {
                                        timeout.await;
                                        db.save_trusted_worktrees(new_trusted_worktrees)
                                            .await
                                            .log_err();
                                    })
                                },
                            )
                        });
                    }
                }
            })
            .detach();

            cx.observe_global::<SettingsStore>(|_, cx| {
                if ProjectSettings::get_global(cx).session.trust_all_worktrees {
                    if let Some(trusted_worktrees) = TrustedWorktrees::try_get_global(cx) {
                        trusted_worktrees.update(cx, |trusted_worktrees, cx| {
                            trusted_worktrees.auto_trust_all(cx);
                        })
                    }
                }
            })
            .detach();
        }

        cx.subscribe_in(&project, window, move |this, _, event, window, cx| {
            match event {
                project::Event::RemoteIdChanged(_) => {
                    this.update_window_title(window, cx);
                }

                project::Event::CollaboratorLeft(peer_id) => {
                    this.collaborator_left(*peer_id, window, cx);
                }

                &project::Event::WorktreeRemoved(_) => {
                    this.update_window_title(window, cx);
                    this.serialize_workspace(window, cx);
                    this.update_history(cx);
                }

                &project::Event::WorktreeAdded(id) => {
                    this.update_window_title(window, cx);
                    if this
                        .project()
                        .read(cx)
                        .worktree_for_id(id, cx)
                        .is_some_and(|wt| wt.read(cx).is_visible())
                    {
                        this.serialize_workspace(window, cx);
                        this.update_history(cx);
                    }
                }
                project::Event::WorktreeUpdatedEntries(..) => {
                    this.update_window_title(window, cx);
                    this.serialize_workspace(window, cx);
                }

                project::Event::DisconnectedFromHost => {
                    this.update_window_edited(window, cx);
                    let leaders_to_unfollow =
                        this.follower_states.keys().copied().collect::<Vec<_>>();
                    for leader_id in leaders_to_unfollow {
                        this.unfollow(leader_id, window, cx);
                    }
                }

                project::Event::DisconnectedFromRemote {
                    server_not_running: _,
                } => {
                    this.update_window_edited(window, cx);
                }

                project::Event::Closed => {
                    window.remove_window();
                }

                project::Event::DeletedEntry(_, entry_id) => {
                    for pane in this.panes.iter() {
                        pane.update(cx, |pane, cx| {
                            pane.handle_deleted_project_item(*entry_id, window, cx)
                        });
                    }
                }

                project::Event::Toast {
                    notification_id,
                    message,
                    link,
                } => this.show_notification(
                    NotificationId::named(notification_id.clone()),
                    cx,
                    |cx| {
                        let mut notification = MessageNotification::new(message.clone(), cx);
                        if let Some(link) = link {
                            notification = notification
                                .more_info_message(link.label)
                                .more_info_url(link.url);
                        }

                        cx.new(|_| notification)
                    },
                ),

                project::Event::HideToast { notification_id } => {
                    this.dismiss_notification(&NotificationId::named(notification_id.clone()), cx)
                }

                project::Event::LanguageServerPrompt(request) => {
                    struct LanguageServerPrompt;

                    this.show_notification(
                        NotificationId::composite::<LanguageServerPrompt>(request.id),
                        cx,
                        |cx| {
                            cx.new(|cx| {
                                notifications::LanguageServerPrompt::new(request.clone(), cx)
                            })
                        },
                    );
                }

                project::Event::AgentLocationChanged => {
                    this.handle_agent_location_changed(window, cx)
                }

                _ => {}
            }
            cx.notify()
        })
        .detach();

        cx.subscribe_in(
            &project.read(cx).breakpoint_store(),
            window,
            |workspace, _, event, window, cx| match event {
                BreakpointStoreEvent::BreakpointsUpdated(_, _)
                | BreakpointStoreEvent::BreakpointsCleared(_) => {
                    workspace.serialize_workspace(window, cx);
                }
                BreakpointStoreEvent::SetDebugLine | BreakpointStoreEvent::ClearDebugLines => {}
            },
        )
        .detach();
        if let Some(toolchain_store) = project.read(cx).toolchain_store() {
            cx.subscribe_in(
                &toolchain_store,
                window,
                |workspace, _, event, window, cx| match event {
                    ToolchainStoreEvent::CustomToolchainsModified => {
                        workspace.serialize_workspace(window, cx);
                    }
                    _ => {}
                },
            )
            .detach();
        }

        cx.on_focus_lost(window, |this, window, cx| {
            let focus_handle = this.focus_handle(cx);
            window.focus(&focus_handle, cx);
        })
        .detach();

        let weak_handle = cx.entity().downgrade();
        let pane_history_timestamp = Arc::new(AtomicUsize::new(0));

        let center_pane = cx.new(|cx| {
            let mut center_pane = Pane::new(
                weak_handle.clone(),
                project.clone(),
                pane_history_timestamp.clone(),
                None,
                NewFile.boxed_clone(),
                true,
                window,
                cx,
            );
            center_pane.set_can_split(Some(Arc::new(|_, _, _, _| true)));
            center_pane.set_should_display_welcome_page(true);
            center_pane
        });
        cx.subscribe_in(&center_pane, window, Self::handle_pane_event)
            .detach();

        window.focus(&center_pane.focus_handle(cx), cx);

        cx.emit(Event::PaneAdded(center_pane.clone()));

        let any_window_handle = window.window_handle();
        app_state.workspace_store.update(cx, |store, _| {
            store
                .workspaces
                .insert((any_window_handle, weak_handle.clone()));
        });

        let mut current_user = app_state.user_store.read(cx).watch_current_user();
        let mut connection_status = app_state.client.status();
        let _observe_current_user = cx.spawn_in(window, async move |this, cx| {
            current_user.next().await;
            connection_status.next().await;
            let mut stream =
                Stream::map(current_user, drop).merge(Stream::map(connection_status, drop));

            while stream.recv().await.is_some() {
                this.update(cx, |_, cx| cx.notify())?;
            }
            anyhow::Ok(())
        });

        // All leader updates are enqueued and then processed in a single task, so
        // that each asynchronous operation can be run in order.
        let (leader_updates_tx, mut leader_updates_rx) =
            mpsc::unbounded::<(PeerId, proto::UpdateFollowers)>();
        let _apply_leader_updates = cx.spawn_in(window, async move |this, cx| {
            while let Some((leader_id, update)) = leader_updates_rx.next().await {
                Self::process_leader_update(&this, leader_id, update, cx)
                    .await
                    .log_err();
            }

            Ok(())
        });

        cx.emit(Event::WorkspaceCreated(weak_handle.clone()));
        let modal_layer = cx.new(|_| ModalLayer::new());
        let toast_layer = cx.new(|_| ToastLayer::new());
        cx.subscribe(
            &modal_layer,
            |_, _, _: &modal_layer::ModalOpenedEvent, cx| {
                cx.emit(Event::ModalOpened);
            },
        )
        .detach();

        let left_dock = Dock::new(DockPosition::Left, modal_layer.clone(), window, cx);
        let bottom_dock = Dock::new(DockPosition::Bottom, modal_layer.clone(), window, cx);
        let right_dock = Dock::new(DockPosition::Right, modal_layer.clone(), window, cx);
        let left_dock_buttons = cx.new(|cx| PanelButtons::new(left_dock.clone(), cx));
        let bottom_dock_buttons = cx.new(|cx| PanelButtons::new(bottom_dock.clone(), cx));
        let right_dock_buttons = cx.new(|cx| PanelButtons::new(right_dock.clone(), cx));
        let multi_workspace = window
            .root::<MultiWorkspace>()
            .flatten()
            .map(|mw| mw.downgrade());
        let status_bar = cx.new(|cx| {
            let mut status_bar =
                StatusBar::new(&center_pane.clone(), multi_workspace.clone(), window, cx);
            status_bar.add_left_item(left_dock_buttons, window, cx);
            status_bar.add_right_item(right_dock_buttons, window, cx);
            status_bar.add_right_item(bottom_dock_buttons, window, cx);
            // The threads sidebar's icon is placed by the setting that moves the
            // sidebar itself, so it goes in as a component-backed control rather
            // than being appended beside a zone the way it was before `v0.45.1`.
            // `available` carries the one thing its position cannot: the sidebar
            // has a side whether or not the agent is enabled, and with the agent
            // off there is nothing to open.
            let threads_sidebar_button =
                cx.new(|_| status_bar::ThreadsSidebarButton::new(multi_workspace.clone()));
            status_bar.add_status_control(
                threads_sidebar_button,
                status_bar::StatusControlAdapter::for_component(
                    crate::layout_registry::ids::THREADS_SIDEBAR,
                    MultiWorkspace::sidebar_ui_enabled_from_settings,
                ),
                window,
                cx,
            );
            status_bar
        });

        let session_id = app_state.session.read(cx).id().to_owned();

        let mut active_call = None;
        if let Some(call) = GlobalAnyActiveCall::try_global(cx).cloned() {
            let subscriptions =
                vec![
                    call.0
                        .subscribe(window, cx, Box::new(Self::on_active_call_event)),
                ];
            active_call = Some((call, subscriptions));
        }

        let (serializable_items_tx, serializable_items_rx) =
            mpsc::unbounded::<ItemSerializationCommand>();
        let _items_serializer = cx.spawn_in(window, async move |this, cx| {
            Self::serialize_items(&this, serializable_items_rx, cx).await
        });

        let subscriptions = vec![
            cx.observe_window_activation(window, Self::on_window_activation_changed),
            cx.observe_window_bounds(window, move |this, window, cx| {
                // Runs before the active-window guard: a window moved to another
                // display must re-resolve its font size even while unfocused, which
                // is the whole point of per-display profiles. The handler itself
                // no-ops unless the display identity actually changed.
                this.handle_possible_display_change(window, cx);

                if !window.is_window_active() {
                    return;
                }
                if this.bounds_save_task_queued.is_some() {
                    return;
                }
                this.bounds_save_task_queued = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    this.update_in(cx, |this, _window, cx| {
                        this.save_window_bounds(cx).detach();
                        this.bounds_save_task_queued.take();
                    })
                    .ok();
                }));
                cx.notify();
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                let window_appearance = window.appearance();

                *SystemAppearance::global_mut(cx) = SystemAppearance(window_appearance.into());

                theme_settings::reload_theme(cx);
                theme_settings::reload_icon_theme(cx);
            }),
            cx.on_release({
                let weak_handle = weak_handle.clone();
                move |this, cx| {
                    this.app_state.workspace_store.update(cx, move |store, _| {
                        store.workspaces.retain(|(_, weak)| weak != &weak_handle);
                    })
                }
            }),
        ];

        cx.defer_in(window, move |this, window, cx| {
            this.update_window_title(window, cx);
            this.show_initial_notifications(cx);
        });

        let mut center = PaneGroup::new(center_pane.clone());
        center.set_is_center(true);
        center.mark_positions(cx);

        Workspace {
            weak_self: weak_handle.clone(),
            zoomed: None,
            zoomed_position: None,
            maximized_pane: None,
            previous_dock_drag_coordinates: None,
            center,
            panes: vec![center_pane.clone()],
            panes_by_item: Default::default(),
            active_pane: center_pane.clone(),
            last_active_center_pane: Some(center_pane.downgrade()),
            last_active_view_id: None,
            status_bar,
            modal_layer,
            toast_layer,
            titlebar_item: None,
            titlebar_focus_handle: cx.focus_handle(),
            region_focus_handles: RegionFocusHandles::new(cx),
            notifications: Notifications::default(),
            suppressed_notifications: HashSet::default(),
            left_dock,
            bottom_dock,
            right_dock,
            _panels_task: None,
            project: project.clone(),
            follower_states: Default::default(),
            last_leaders_by_pane: Default::default(),
            auto_watch: AutoWatch::Off,
            dispatching_keystrokes: Default::default(),
            window_edited: false,
            last_window_title: None,
            dirty_items: Default::default(),
            active_call,
            database_id: workspace_id,
            app_state,
            _observe_current_user,
            _apply_leader_updates,
            _schedule_serialize_workspace: None,
            _serialize_workspace_task: None,
            _schedule_serialize_ssh_paths: None,
            leader_updates_tx,
            _subscriptions: subscriptions,
            pane_history_timestamp,
            workspace_actions: Default::default(),
            // This data will be incorrect, but it will be overwritten by the time it needs to be used.
            bounds: Default::default(),
            centered_layout: false,
            bounds_save_task_queued: None,
            last_display_id: window.display_id(),
            on_prompt_for_new_path: None,
            on_prompt_for_open_path: None,
            terminal_provider: None,
            debugger_provider: None,
            serializable_items_tx,
            _items_serializer,
            session_id: Some(session_id),

            scheduled_tasks: Vec::new(),
            last_open_dock_positions: Vec::new(),
            removing: false,
            sidebar_focus_handle: None,
            multi_workspace,
            detached_restore: false,
            active_workspace_id: None,
            active_worktree_creation: ActiveWorktreeCreation::default(),
            open_in_dev_container: false,
            _dev_container_task: None,
            deferred_save_items: Vec::new(),
        }
    }

    pub fn new_local(
        abs_paths: Vec<PathBuf>,
        app_state: Arc<AppState>,
        requesting_window: Option<WindowHandle<MultiWorkspace>>,
        env: Option<HashMap<String, String>>,
        init: Option<Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send>>,
        open_mode: OpenMode,
        cx: &mut App,
    ) -> Task<anyhow::Result<OpenResult>> {
        let project_handle = Project::local(
            app_state.client.clone(),
            app_state.node_runtime.clone(),
            app_state.user_store.clone(),
            app_state.languages.clone(),
            app_state.fs.clone(),
            env,
            Default::default(),
            cx,
        );

        let db = WorkspaceDb::global(cx);
        let kvp = db::kvp::KeyValueStore::global(cx);
        cx.spawn(async move |cx| {
            let mut paths_to_open = Vec::with_capacity(abs_paths.len());
            for path in abs_paths.into_iter() {
                if let Some(canonical) = app_state.fs.canonicalize(&path).await.ok() {
                    paths_to_open.push(canonical)
                } else {
                    paths_to_open.push(path)
                }
            }

            let serialized_workspace = db.workspace_for_roots(paths_to_open.as_slice());

            if let Some(paths) = serialized_workspace.as_ref().map(|ws| &ws.paths) {
                paths_to_open = paths.ordered_paths().cloned().collect();
            }

            // Get project paths for all of the abs_paths
            let mut project_paths: Vec<(PathBuf, Option<ProjectPath>)> =
                Vec::with_capacity(paths_to_open.len());

            for path in paths_to_open.into_iter() {
                if let Some((_, project_entry)) = cx
                    .update(|cx| {
                        Workspace::project_path_for_path(project_handle.clone(), &path, true, cx)
                    })
                    .await
                    .log_err()
                {
                    project_paths.push((path, Some(project_entry)));
                } else {
                    project_paths.push((path, None));
                }
            }

            let workspace_id = if let Some(serialized_workspace) = serialized_workspace.as_ref() {
                serialized_workspace.id
            } else {
                db.next_id().await.unwrap_or_else(|_| Default::default())
            };

            let toolchains = db.toolchains(workspace_id).await?;

            for (toolchain, worktree_path, path) in toolchains {
                let toolchain_path = PathBuf::from(toolchain.path.clone().to_string());
                let Some(worktree_id) = project_handle.read_with(cx, |this, cx| {
                    this.find_worktree(&worktree_path, cx)
                        .and_then(|(worktree, rel_path)| {
                            if rel_path.is_empty() {
                                Some(worktree.read(cx).id())
                            } else {
                                None
                            }
                        })
                }) else {
                    // We did not find a worktree with a given path, but that's whatever.
                    continue;
                };
                if !app_state.fs.is_file(toolchain_path.as_path()).await {
                    continue;
                }

                project_handle
                    .update(cx, |this, cx| {
                        this.activate_toolchain(ProjectPath { worktree_id, path }, toolchain, cx)
                    })
                    .await;
            }
            if let Some(workspace) = serialized_workspace.as_ref() {
                project_handle.update(cx, |this, cx| {
                    for (scope, toolchains) in &workspace.user_toolchains {
                        for toolchain in toolchains {
                            this.add_toolchain(toolchain.clone(), scope.clone(), cx);
                        }
                    }
                });
            }

            let window_to_replace = match open_mode {
                OpenMode::NewWindow => None,
                _ => requesting_window,
            };

            let (window, workspace): (WindowHandle<MultiWorkspace>, Entity<Workspace>) =
                if let Some(window) = window_to_replace {
                    let centered_layout = serialized_workspace
                        .as_ref()
                        .map(|w| w.centered_layout)
                        .unwrap_or(false);

                    let workspace = window.update(cx, |multi_workspace, window, cx| {
                        // `#zed-66.4`: captured *before* the activate below, because this
                        // is the only moment that identifies the workspace the open
                        // displaced. Scanning afterwards for an empty, undisplayed row
                        // would also find a workspace the user emptied by hand and clicked
                        // away from, which has identical state and must keep its row.
                        let displaced = multi_workspace.workspace().clone();
                        let workspace = cx.new(|cx| {
                            let mut workspace = Workspace::new(
                                Some(workspace_id),
                                project_handle.clone(),
                                app_state.clone(),
                                window,
                                cx,
                            );

                            workspace.centered_layout = centered_layout;

                            // Call init callback to add items before window renders
                            if let Some(init) = init {
                                init(&mut workspace, window, cx);
                            }

                            workspace
                        });
                        match open_mode {
                            OpenMode::Activate => {
                                multi_workspace.activate(workspace.clone(), None, window, cx);
                                // `#zed-66.4`: this arm is the seam every same-window open
                                // converges on, `MultiWorkspace::open_project` included.
                                // Clearing the displaced empty here rather than in each
                                // caller is what stops the next route from regenerating
                                // the defect — the project panel's empty-state drop and
                                // `zed <dir>` were instances six and seven.
                                //
                                // The empty snapshot is deliberate: callers arriving here
                                // directly never ran `prepare_to_close`, so nothing has
                                // consented to losing unsaved work and only a clean
                                // workspace may be dropped. `open_project` runs its own
                                // consented pass afterwards.
                                multi_workspace
                                    .detach_replaced_empty_workspace_if_clean(&displaced, cx);
                            }
                            OpenMode::Add => {
                                // Untouched by `#zed-66.4`: `Add` means keep what is
                                // already there.
                                multi_workspace.add(workspace.clone(), window, cx);
                            }
                            OpenMode::NewWindow => {
                                unreachable!()
                            }
                        }
                        workspace
                    })?;
                    (window, workspace)
                } else {
                    let window_bounds_override = window_bounds_env_override();

                    let (window_bounds, display) = if let Some(bounds) = window_bounds_override {
                        (Some(WindowBounds::Windowed(bounds)), None)
                    } else if let Some(workspace) = serialized_workspace.as_ref()
                        && let Some(display) = workspace.display
                        && let Some(bounds) = workspace.window_bounds.as_ref()
                    {
                        // Reopening an existing workspace - restore its saved bounds
                        (Some(bounds.0), Some(display))
                    } else if let Some((display, bounds)) =
                        persistence::read_default_window_bounds(&kvp)
                    {
                        // New or empty workspace - use the last known window bounds
                        (Some(bounds), Some(display))
                    } else {
                        // New window - let GPUI's default_bounds() handle cascading
                        (None, None)
                    };

                    // Use the serialized workspace to construct the new window
                    let mut options = cx.update(|cx| (app_state.build_window_options)(display, cx));
                    options.window_bounds = window_bounds;
                    let centered_layout = serialized_workspace
                        .as_ref()
                        .map(|w| w.centered_layout)
                        .unwrap_or(false);
                    let window = cx.open_window(options, {
                        let app_state = app_state.clone();
                        let project_handle = project_handle.clone();
                        move |window, cx| {
                            let workspace = cx.new(|cx| {
                                let mut workspace = Workspace::new(
                                    Some(workspace_id),
                                    project_handle,
                                    app_state,
                                    window,
                                    cx,
                                );
                                workspace.centered_layout = centered_layout;

                                // Call init callback to add items before window renders
                                if let Some(init) = init {
                                    init(&mut workspace, window, cx);
                                }

                                workspace
                            });
                            cx.new(|cx| MultiWorkspace::new(workspace, window, cx))
                        }
                    })?;
                    let workspace =
                        window.update(cx, |multi_workspace: &mut MultiWorkspace, _, _cx| {
                            multi_workspace.workspace().clone()
                        })?;
                    (window, workspace)
                };

            notify_if_database_failed(window, cx);
            // Check if this is an empty workspace (no paths to open)
            // An empty workspace is one where project_paths is empty
            let is_empty_workspace = project_paths.is_empty();
            // Check if serialized workspace has paths before it's moved
            let serialized_workspace_has_paths = serialized_workspace
                .as_ref()
                .map(|ws| !ws.paths.is_empty())
                .unwrap_or(false);

            let opened_items = window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |_workspace: &mut Workspace, cx| {
                        open_items(serialized_workspace, project_paths, window, cx)
                    })
                })?
                .await
                .unwrap_or_default();

            // Restore default dock state for empty workspaces
            // Only restore if:
            // 1. This is an empty workspace (no paths), AND
            // 2. The serialized workspace either doesn't exist or has no paths
            if is_empty_workspace && !serialized_workspace_has_paths {
                if let Some(default_docks) = persistence::read_default_dock_state(&kvp) {
                    window
                        .update(cx, |_, window, cx| {
                            workspace.update(cx, |workspace, cx| {
                                for (dock, serialized_dock) in [
                                    (&workspace.right_dock, &default_docks.right),
                                    (&workspace.left_dock, &default_docks.left),
                                    (&workspace.bottom_dock, &default_docks.bottom),
                                ] {
                                    dock.update(cx, |dock, cx| {
                                        dock.set_serialized_state(serialized_dock.clone(), None);
                                        dock.restore_state(window, cx);
                                    });
                                }
                                cx.notify();
                            });
                        })
                        .log_err();
                }
            }

            window
                .update(cx, |_, _window, cx| {
                    workspace.update(cx, |this: &mut Workspace, cx| {
                        this.update_history(cx);
                    });
                })
                .log_err();
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |workspace, cx| {
                        workspace.flush_serialization(window, cx)
                    })
                })
                .map(|task| task.detach())
                .log_err();

            if open_mode == OpenMode::NewWindow || open_mode == OpenMode::Activate {
                window
                    .update(cx, |_, window, _cx| {
                        window.activate_window();
                    })
                    .log_err();
            }

            // Auto-show the security modal if the project has restricted worktrees
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |workspace, cx| {
                        workspace.show_worktree_trust_security_modal(false, window, cx);
                    });
                })
                .log_err();

            Ok(OpenResult {
                window,
                workspace,
                opened_items,
            })
        })
    }

    pub(crate) fn prepare_local_strict(
        serialized_workspace: SerializedWorkspace,
        app_state: Arc<AppState>,
        window: WindowHandle<MultiWorkspace>,
        cx: &mut App,
    ) -> Task<Result<Entity<Workspace>>> {
        let project = Project::local(
            app_state.client.clone(),
            app_state.node_runtime.clone(),
            app_state.user_store.clone(),
            app_state.languages.clone(),
            app_state.fs.clone(),
            None,
            Default::default(),
            cx,
        );

        cx.spawn(async move |cx| {
            anyhow::ensure!(
                serialized_workspace.location == SerializedWorkspaceLocation::Local,
                "workspace configurations currently support local workspaces only"
            );
            for path in serialized_workspace.paths.paths() {
                anyhow::ensure!(
                    app_state.fs.metadata(path).await?.is_some(),
                    "workspace path {} is unavailable",
                    path.display()
                );
                cx.update(|cx| Workspace::project_path_for_path(project.clone(), path, true, cx))
                    .await
                    .with_context(|| format!("preparing workspace path {}", path.display()))?;
            }

            project.update(cx, |project, cx| {
                for (scope, toolchains) in &serialized_workspace.user_toolchains {
                    for toolchain in toolchains {
                        project.add_toolchain(toolchain.clone(), scope.clone(), cx);
                    }
                }
            });

            let workspace_id = serialized_workspace.id;
            let centered_layout = serialized_workspace.centered_layout;
            let workspace = window.update(cx, |_multi_workspace, window, cx| {
                cx.new(|cx| {
                    let mut workspace =
                        Workspace::new(Some(workspace_id), project, app_state, window, cx);
                    workspace.centered_layout = centered_layout;
                    workspace.detached_restore = true;
                    workspace
                })
            })?;

            let restore = window.update(cx, |_multi_workspace, window, cx| {
                workspace.update(cx, |_workspace, cx| {
                    Workspace::load_workspace_strict(serialized_workspace, window, cx)
                })
            })?;
            restore.await?;
            Ok(workspace)
        })
    }

    pub fn project_group_key(&self, cx: &App) -> ProjectGroupKey {
        self.project.read(cx).project_group_key(cx)
    }

    pub fn weak_handle(&self) -> WeakEntity<Self> {
        self.weak_self.clone()
    }

    pub fn left_dock(&self) -> &Entity<Dock> {
        &self.left_dock
    }

    pub fn bottom_dock(&self) -> &Entity<Dock> {
        &self.bottom_dock
    }

    pub fn set_bottom_dock_layout(
        &mut self,
        layout: BottomDockLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fs = self.project().read(cx).fs();
        settings::update_settings_file(fs.clone(), cx, move |content, _cx| {
            content.workspace.bottom_dock_layout = Some(layout);
        });

        cx.notify();
        self.serialize_workspace(window, cx);
    }

    pub fn right_dock(&self) -> &Entity<Dock> {
        &self.right_dock
    }

    pub fn all_docks(&self) -> [&Entity<Dock>; 3] {
        [&self.left_dock, &self.bottom_dock, &self.right_dock]
    }

    pub fn capture_dock_state(&self, _window: &Window, cx: &App) -> DockStructure {
        let left_dock = self.left_dock.read(cx);
        let left_visible = left_dock.is_open();
        let left_active_panel = left_dock
            .active_panel()
            .map(|panel| panel.persistent_name().to_string());
        // `zoomed_position` is kept in sync with individual panel zoom state
        // by the dock code in `Dock::new` and `Dock::add_panel`.
        let left_dock_zoom = self.zoomed_position == Some(DockPosition::Left);

        let right_dock = self.right_dock.read(cx);
        let right_visible = right_dock.is_open();
        let right_active_panel = right_dock
            .active_panel()
            .map(|panel| panel.persistent_name().to_string());
        let right_dock_zoom = self.zoomed_position == Some(DockPosition::Right);

        let bottom_dock = self.bottom_dock.read(cx);
        let bottom_visible = bottom_dock.is_open();
        let bottom_active_panel = bottom_dock
            .active_panel()
            .map(|panel| panel.persistent_name().to_string());
        let bottom_dock_zoom = self.zoomed_position == Some(DockPosition::Bottom);

        DockStructure {
            left: DockData {
                visible: left_visible,
                active_panel: left_active_panel,
                zoom: left_dock_zoom,
            },
            right: DockData {
                visible: right_visible,
                active_panel: right_active_panel,
                zoom: right_dock_zoom,
            },
            bottom: DockData {
                visible: bottom_visible,
                active_panel: bottom_active_panel,
                zoom: bottom_dock_zoom,
            },
        }
    }

    /// Every open component's stable key, across all three sides. This is the
    /// per-workspace open set that the legacy single `active_panel` column
    /// cannot express.
    pub fn capture_open_components(&self, cx: &App) -> Vec<String> {
        [&self.left_dock, &self.right_dock, &self.bottom_dock]
            .into_iter()
            .flat_map(|dock| {
                dock.read(cx)
                    .open_panels()
                    .into_iter()
                    .map(|panel| panel.panel_key().to_string())
            })
            .collect()
    }

    pub fn set_dock_structure(
        &self,
        docks: DockStructure,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (dock, data) in [
            (&self.left_dock, docks.left),
            (&self.bottom_dock, docks.bottom),
            (&self.right_dock, docks.right),
        ] {
            dock.update(cx, |dock, cx| {
                dock.set_serialized_state(data, None);
                dock.restore_state(window, cx);
            });
        }
    }

    /// Returns which dock currently has focus, or `None` if focus is in the
    /// center pane or elsewhere. Does NOT fall back to any global state.
    pub fn focused_dock_position(&self, window: &Window, cx: &App) -> Option<DockPosition> {
        [
            (DockPosition::Left, &self.left_dock),
            (DockPosition::Right, &self.right_dock),
            (DockPosition::Bottom, &self.bottom_dock),
        ]
        .into_iter()
        .find(|(_, dock)| {
            dock.read(cx).is_open() && dock.focus_handle(cx).contains_focused(window, cx)
        })
        .map(|(position, _)| position)
    }

    pub fn active_worktree_creation(&self) -> &ActiveWorktreeCreation {
        &self.active_worktree_creation
    }

    pub fn set_active_worktree_creation(
        &mut self,
        label: Option<SharedString>,
        is_switch: bool,
        cx: &mut Context<Self>,
    ) {
        self.active_worktree_creation.label = label;
        self.active_worktree_creation.is_switch = is_switch;
        cx.emit(Event::WorktreeCreationChanged);
        cx.notify();
    }

    /// Captures the current workspace state for restoring after a worktree switch.
    /// This includes dock layout, open file paths, and the active file path.
    pub fn capture_state_for_worktree_switch(
        &self,
        window: &Window,
        fallback_focused_dock: Option<DockPosition>,
        cx: &App,
    ) -> PreviousWorkspaceState {
        let dock_structure = self.capture_dock_state(window, cx);
        let open_file_paths = self.open_item_abs_paths(cx);
        let active_file_path = self
            .active_item(cx)
            .and_then(|item| item.project_path(cx))
            .and_then(|pp| self.project().read(cx).absolute_path(&pp, cx));

        let focused_dock = self
            .focused_dock_position(window, cx)
            .or(fallback_focused_dock);

        PreviousWorkspaceState {
            dock_structure,
            open_file_paths,
            active_file_path,
            focused_dock,
        }
    }

    pub fn open_item_abs_paths(&self, cx: &App) -> Vec<PathBuf> {
        self.items(cx)
            .filter_map(|item| {
                let project_path = item.project_path(cx)?;
                self.project.read(cx).absolute_path(&project_path, cx)
            })
            .collect()
    }

    pub fn dock_at_position(&self, position: DockPosition) -> &Entity<Dock> {
        match position {
            DockPosition::Left => &self.left_dock,
            DockPosition::Bottom => &self.bottom_dock,
            DockPosition::Right => &self.right_dock,
        }
    }

    pub fn agent_panel_position(&self, cx: &App) -> Option<DockPosition> {
        self.all_docks().into_iter().find_map(|dock| {
            let dock = dock.read(cx);
            dock.has_agent_panel(cx).then_some(dock.position())
        })
    }

    pub fn panel_size_state<T: Panel>(&self, cx: &App) -> Option<dock::PanelSizeState> {
        self.all_docks().into_iter().find_map(|dock| {
            let dock = dock.read(cx);
            let panel = dock.panel::<T>()?;
            dock.stored_panel_size_state(&panel)
        })
    }

    pub fn persisted_panel_size_state(
        &self,
        panel_key: &'static str,
        cx: &App,
    ) -> Option<dock::PanelSizeState> {
        dock::Dock::load_persisted_size_state(self, panel_key, cx)
    }

    /// Returns every open component on every side to its default width, and
    /// persists that so the reset survives a restart.
    ///
    /// This used to reset `visible_panel` — one component per side, which was the
    /// whole side back when a side held one. A side now holds several columns, so
    /// resetting one left every other column at exactly the width the user was
    /// reaching for this command to undo. Nothing was persisted either, so what
    /// little did reset came back on the next launch.
    pub fn reset_open_docks_size(&mut self, cx: &mut Context<Self>) {
        let mut size_states_to_persist = Vec::new();
        for dock in self.all_docks() {
            let panels = dock.read(cx).open_panels();
            dock.update(cx, |dock, cx| {
                for panel in panels {
                    if dock.set_panel_size_state(
                        panel.as_ref(),
                        dock::PanelSizeState::default(),
                        cx,
                    ) {
                        size_states_to_persist
                            .push((panel.panel_key(), dock::PanelSizeState::default()));
                    }
                }
            });
        }
        for (panel_key, size_state) in size_states_to_persist {
            self.persist_panel_size_state(panel_key, size_state, cx);
        }
    }

    pub fn persist_panel_size_state(
        &self,
        panel_key: &str,
        size_state: dock::PanelSizeState,
        cx: &mut App,
    ) {
        let Some(workspace_id) = self
            .database_id()
            .map(|id| i64::from(id).to_string())
            .or(self.session_id())
        else {
            return;
        };

        let kvp = db::kvp::KeyValueStore::global(cx);
        let panel_key = panel_key.to_string();
        cx.background_spawn(async move {
            let scope = kvp.scoped(dock::PANEL_SIZE_STATE_KEY);
            scope
                .write(
                    format!("{workspace_id}:{panel_key}"),
                    serde_json::to_string(&size_state)?,
                )
                .await
        })
        .detach_and_log_err(cx);
    }

    pub fn set_panel_size_state<T: Panel>(
        &mut self,
        size_state: dock::PanelSizeState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(panel) = self.panel::<T>(cx) else {
            return false;
        };

        let dock = self.dock_at_position(panel.position(window, cx));
        let did_set = dock.update(cx, |dock, cx| {
            dock.set_panel_size_state(&panel, size_state, cx)
        });

        if did_set {
            self.persist_panel_size_state(T::panel_key(), size_state, cx);
        }

        did_set
    }

    pub fn toggle_dock_panel_flexible_size(
        &self,
        dock: &Entity<Dock>,
        panel: &dyn PanelHandle,
        window: &mut Window,
        cx: &mut App,
    ) {
        let position = dock.read(cx).position();
        let current_size = self.dock_size(&dock.read(cx), window, cx);
        // A panel going fixed -> flexible needs *its own* flex factor, and two
        // things make that more than one inversion. It is still counted in
        // `fixed` here, so `dock_flex_for_size` would hand back a share of zero;
        // and `flex_for_share` inverts the dock's *total* flex, so any flexible
        // sibling already on this side is part of the answer. Solve for the total
        // the side should reach, then subtract what the siblings already supply.
        // Upstream needed neither step: one panel per dock means no siblings and
        // nothing else fixed.
        let current_flex = {
            let read = dock.read(cx);
            self.dock_geometry(position, window, cx).map(|geometry| {
                let fixed = read.open_fixed_size(window, cx);
                let width = read
                    .stored_panel_size_state(panel)
                    .and_then(|state| state.size)
                    .unwrap_or_else(|| panel.default_size(window, cx));
                let siblings = read
                    .open_flex(self.default_dock_flex(position), window, cx)
                    .unwrap_or(0.0);
                let target = geometry.flex_share_or_zero(fixed, siblings) + width;
                // Releasing the column is just a smaller `fixed` now.
                let total = geometry.flex_for_share((fixed - width).max(Pixels::ZERO), target);
                (total - siblings).max(0.0)
            })
        };
        dock.update(cx, |dock, cx| {
            dock.toggle_panel_flexible_size(panel, current_size, current_flex, window, cx);
        });
    }

    pub(crate) fn dock_geometry(
        &self,
        position: DockPosition,
        window: &Window,
        cx: &App,
    ) -> Option<DockGeometry> {
        let workspace_width = self.bounds.size.width;
        if workspace_width <= Pixels::ZERO {
            return None;
        }
        let (opposite_fixed, opposite_flex) = self.opposite_dock_size_summary(position, window, cx);
        Some(DockGeometry {
            workspace_width,
            opposite_fixed,
            competing_flex: self.center_full_height_column_count() + opposite_flex.unwrap_or(0.0),
        })
    }

    fn dock_size(&self, dock: &Dock, window: &Window, cx: &App) -> Option<Pixels> {
        if !dock.is_open() {
            return None;
        }
        let position = dock.position();
        if position.axis() != Axis::Horizontal {
            return dock.open_row_size(window, cx);
        }
        let fixed = dock.open_fixed_size(window, cx);
        let Some(flex) = dock.open_flex(self.default_dock_flex(position), window, cx) else {
            return Some(fixed);
        };
        let geometry = self.dock_geometry(position, window, cx)?;
        let flex = flex.max(0.001);
        // One share for both cases. An opposite side with no flexible column
        // contributes zero competing flex, which reproduces the fixed-opposite
        // branch this replaced; what changes is that the flexible-opposite case
        // now shares `available` rather than the whole workspace width, so a
        // mixed side stops claiming its own fixed columns' pixels twice.
        Some(fixed + geometry.flex_share(fixed, flex))
    }

    /// The flex factor that lays this dock out at `size` overall — the inverse of
    /// the share `dock_size` computes, and derived from the same `DockGeometry`
    /// so the two cannot drift apart again.
    ///
    /// `size` is the whole dock, fixed columns included. Subtracting them is the
    /// step upstream never needed: with one panel per dock a flexible dock *was*
    /// its flex share, so `size` and the share were the same number.
    pub fn dock_flex_for_size(
        &self,
        position: DockPosition,
        size: Pixels,
        window: &Window,
        cx: &App,
    ) -> Option<f32> {
        if position.axis() != Axis::Horizontal {
            return None;
        }
        let fixed = self
            .dock_at_position(position)
            .read(cx)
            .open_fixed_size(window, cx);
        let geometry = self.dock_geometry(position, window, cx)?;
        Some(geometry.flex_for_size(fixed, size))
    }

    /// The opposite side's open panels as one width: fixed px plus total flex.
    fn opposite_dock_size_summary(
        &self,
        position: DockPosition,
        window: &Window,
        cx: &App,
    ) -> (Pixels, Option<f32>) {
        let opposite_position = match position {
            DockPosition::Left => DockPosition::Right,
            DockPosition::Right => DockPosition::Left,
            DockPosition::Bottom => return (Pixels::ZERO, None),
        };
        let opposite_dock = self.dock_at_position(opposite_position).read(cx);
        if !opposite_dock.is_open() {
            return (Pixels::ZERO, None);
        }
        (
            opposite_dock.open_fixed_size(window, cx),
            opposite_dock.open_flex(self.default_dock_flex(opposite_position), window, cx),
        )
    }

    fn center_full_height_column_count(&self) -> f32 {
        self.center.full_height_column_count().max(1) as f32
    }

    pub fn default_dock_flex(&self, position: DockPosition) -> Option<f32> {
        if position.axis() != Axis::Horizontal {
            return None;
        }

        Some(1.0)
    }

    pub fn is_edited(&self) -> bool {
        self.window_edited
    }

    pub fn add_panel<T: Panel>(
        &mut self,
        panel: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus_handle = panel.panel_focus_handle(cx);
        cx.on_focus_in(&focus_handle, window, Self::handle_panel_focused)
            .detach();

        let dock_position = panel.position(window, cx);
        let dock = self.dock_at_position(dock_position);
        let any_panel = panel.to_any();
        let persisted_size_state =
            self.persisted_panel_size_state(T::panel_key(), cx)
                .or_else(|| {
                    load_legacy_panel_size(T::panel_key(), dock_position, self, cx).map(|size| {
                        let state = dock::PanelSizeState {
                            size: Some(size),
                            flex: None,
                            weight: None,
                        };
                        self.persist_panel_size_state(T::panel_key(), state, cx);
                        state
                    })
                });

        dock.update(cx, |dock, cx| {
            let index = dock.add_panel(panel.clone(), self.weak_self.clone(), window, cx);
            if let Some(size_state) = persisted_size_state {
                dock.set_panel_size_state(&panel, size_state, cx);
            }
            index
        });

        cx.emit(Event::PanelAdded(any_panel));
    }

    pub fn remove_panel<T: Panel>(
        &mut self,
        panel: &Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for dock in [&self.left_dock, &self.bottom_dock, &self.right_dock] {
            dock.update(cx, |dock, cx| dock.remove_panel(panel, window, cx));
        }
    }

    pub fn status_bar(&self) -> &Entity<StatusBar> {
        &self.status_bar
    }

    pub fn set_sidebar_focus_handle(&mut self, handle: Option<FocusHandle>) {
        self.sidebar_focus_handle = handle;
    }

    pub fn status_bar_visible(&self, cx: &App) -> bool {
        StatusBarSettings::get_global(cx).show
    }

    pub fn multi_workspace(&self) -> Option<&WeakEntity<MultiWorkspace>> {
        self.multi_workspace.as_ref()
    }

    pub fn set_multi_workspace(
        &mut self,
        multi_workspace: WeakEntity<MultiWorkspace>,
        active_workspace_id: Rc<Cell<EntityId>>,
        cx: &mut App,
    ) {
        self.status_bar.update(cx, |status_bar, cx| {
            status_bar.set_multi_workspace(multi_workspace.clone(), cx);
        });
        self.multi_workspace = Some(multi_workspace);
        self.detached_restore = false;
        self.active_workspace_id = Some(active_workspace_id);
    }

    pub fn app_state(&self) -> &Arc<AppState> {
        &self.app_state
    }

    pub fn set_panels_task(&mut self, task: Task<Result<()>>) {
        self._panels_task = Some(task);
    }

    pub fn take_panels_task(&mut self) -> Option<Task<Result<()>>> {
        self._panels_task.take()
    }

    pub fn user_store(&self) -> &Entity<UserStore> {
        &self.app_state.user_store
    }

    pub fn project(&self) -> &Entity<Project> {
        &self.project
    }

    pub fn path_style(&self, cx: &App) -> PathStyle {
        self.project.read(cx).path_style(cx)
    }

    pub fn recently_activated_items(&self, cx: &App) -> HashMap<EntityId, usize> {
        let mut history: HashMap<EntityId, usize> = HashMap::default();

        for pane_handle in &self.panes {
            let pane = pane_handle.read(cx);

            for entry in pane.activation_history() {
                history.insert(
                    entry.entity_id,
                    history
                        .get(&entry.entity_id)
                        .cloned()
                        .unwrap_or(0)
                        .max(entry.timestamp),
                );
            }
        }

        history
    }

    pub fn recent_active_item_by_type<T: 'static>(&self, cx: &App) -> Option<Entity<T>> {
        let mut recent_item: Option<Entity<T>> = None;
        let mut recent_timestamp = 0;
        for pane_handle in &self.panes {
            let pane = pane_handle.read(cx);
            let item_map: HashMap<EntityId, &Box<dyn ItemHandle>> =
                pane.items().map(|item| (item.item_id(), item)).collect();
            for entry in pane.activation_history() {
                if entry.timestamp > recent_timestamp
                    && let Some(&item) = item_map.get(&entry.entity_id)
                    && let Some(typed_item) = item.act_as::<T>(cx)
                {
                    recent_timestamp = entry.timestamp;
                    recent_item = Some(typed_item);
                }
            }
        }
        recent_item
    }

    pub fn recent_navigation_history_iter(
        &self,
        cx: &App,
    ) -> impl Iterator<Item = (ProjectPath, Option<PathBuf>)> + use<> {
        let mut abs_paths_opened: HashMap<PathBuf, HashSet<ProjectPath>> = HashMap::default();
        let mut history: HashMap<ProjectPath, (Option<PathBuf>, usize)> = HashMap::default();

        for pane in &self.panes {
            let pane = pane.read(cx);

            pane.nav_history()
                .for_each_entry(cx, &mut |entry, (project_path, fs_path)| {
                    if let Some(fs_path) = &fs_path {
                        abs_paths_opened
                            .entry(fs_path.clone())
                            .or_default()
                            .insert(project_path.clone());
                    }
                    let timestamp = entry.timestamp;
                    match history.entry(project_path) {
                        hash_map::Entry::Occupied(mut entry) => {
                            let (_, old_timestamp) = entry.get();
                            if &timestamp > old_timestamp {
                                entry.insert((fs_path, timestamp));
                            }
                        }
                        hash_map::Entry::Vacant(entry) => {
                            entry.insert((fs_path, timestamp));
                        }
                    }
                });

            if let Some(item) = pane.active_item()
                && let Some(project_path) = item.project_path(cx)
            {
                let fs_path = self.project.read(cx).absolute_path(&project_path, cx);

                if let Some(fs_path) = &fs_path {
                    abs_paths_opened
                        .entry(fs_path.clone())
                        .or_default()
                        .insert(project_path.clone());
                }

                history.insert(project_path, (fs_path, std::usize::MAX));
            }
        }

        history
            .into_iter()
            .sorted_by_key(|(_, (_, order))| *order)
            .map(|(project_path, (fs_path, _))| (project_path, fs_path))
            .rev()
            .filter(move |(history_path, abs_path)| {
                let latest_project_path_opened = abs_path
                    .as_ref()
                    .and_then(|abs_path| abs_paths_opened.get(abs_path))
                    .and_then(|project_paths| {
                        project_paths
                            .iter()
                            .max_by(|b1, b2| b1.worktree_id.cmp(&b2.worktree_id))
                    });

                latest_project_path_opened.is_none_or(|path| path == history_path)
            })
    }

    pub fn recent_navigation_history(
        &self,
        limit: Option<usize>,
        cx: &App,
    ) -> Vec<(ProjectPath, Option<PathBuf>)> {
        self.recent_navigation_history_iter(cx)
            .take(limit.unwrap_or(usize::MAX))
            .collect()
    }

    pub fn clear_navigation_history(&mut self, _window: &mut Window, cx: &mut Context<Workspace>) {
        for pane in &self.panes {
            pane.update(cx, |pane, cx| pane.nav_history_mut().clear(cx));
        }
    }

    fn navigate_history(
        &mut self,
        pane: WeakEntity<Pane>,
        mode: NavigationMode,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        self.navigate_history_impl(
            pane,
            mode,
            window,
            &mut |history, cx| history.pop(mode, cx),
            cx,
        )
    }

    fn navigate_tag_history(
        &mut self,
        pane: WeakEntity<Pane>,
        mode: TagNavigationMode,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        self.navigate_history_impl(
            pane,
            NavigationMode::Normal,
            window,
            &mut |history, _cx| history.pop_tag(mode),
            cx,
        )
    }

    fn navigate_history_impl(
        &mut self,
        pane: WeakEntity<Pane>,
        mode: NavigationMode,
        window: &mut Window,
        cb: &mut dyn FnMut(&mut NavHistory, &mut App) -> Option<NavigationEntry>,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        let to_load = if let Some(pane) = pane.upgrade() {
            pane.update(cx, |pane, cx| {
                window.focus(&pane.focus_handle(cx), cx);
                loop {
                    // Retrieve the weak item handle from the history.
                    let entry = cb(pane.nav_history_mut(), cx)?;

                    // If the item is still present in this pane, then activate it.
                    if let Some(index) = entry
                        .item
                        .upgrade()
                        .and_then(|v| pane.index_for_item(v.as_ref()))
                    {
                        let prev_active_item_index = pane.active_item_index();
                        pane.nav_history_mut().set_mode(mode);
                        pane.activate_item(index, true, true, window, cx);
                        pane.nav_history_mut().set_mode(NavigationMode::Normal);

                        let mut navigated = prev_active_item_index != pane.active_item_index();
                        if let Some(data) = entry.data {
                            navigated |= pane.active_item()?.navigate(data, window, cx);
                        }

                        if navigated {
                            break None;
                        }
                    } else {
                        // If the item is no longer present in this pane, then retrieve its
                        // path info in order to reopen it.
                        if let Some((project_path, abs_path)) =
                            pane.nav_history().path_for_item(entry.item.id())
                        {
                            break Some((project_path, abs_path, entry));
                        }
                    }
                }
            })
        } else {
            None
        };

        if let Some((project_path, abs_path, entry)) = to_load {
            // If the item was no longer present, then load it again from its previous path, first try the local path
            let open_by_project_path = self.load_path(project_path.clone(), window, cx);

            cx.spawn_in(window, async move  |workspace, cx| {
                let open_by_project_path = open_by_project_path.await;
                let mut navigated = false;
                match open_by_project_path
                    .with_context(|| format!("Navigating to {project_path:?}"))
                {
                    Ok((project_entry_id, build_item)) => {
                        let prev_active_item_id = pane.update(cx, |pane, _| {
                            pane.nav_history_mut().set_mode(mode);
                            pane.active_item().map(|p| p.item_id())
                        })?;

                        pane.update_in(cx, |pane, window, cx| {
                            let item = pane.open_item(
                                project_entry_id,
                                project_path,
                                true,
                                entry.is_preview,
                                true,
                                None,
                                window, cx,
                                build_item,
                            );
                            navigated |= Some(item.item_id()) != prev_active_item_id;
                            pane.nav_history_mut().set_mode(NavigationMode::Normal);
                            if let Some(data) = entry.data {
                                navigated |= item.navigate(data, window, cx);
                            }
                        })?;
                    }
                    Err(open_by_project_path_e) => {
                        // Fall back to opening by abs path, in case an external file was opened and closed,
                        // and its worktree is now dropped
                        if let Some(abs_path) = abs_path {
                            let prev_active_item_id = pane.update(cx, |pane, _| {
                                pane.nav_history_mut().set_mode(mode);
                                pane.active_item().map(|p| p.item_id())
                            })?;
                            let open_by_abs_path = workspace.update_in(cx, |workspace, window, cx| {
                                workspace.open_abs_path(abs_path.clone(), OpenOptions { visible: Some(OpenVisible::None), ..Default::default() }, window, cx)
                            })?;
                            match open_by_abs_path
                                .await
                                .with_context(|| format!("Navigating to {abs_path:?}"))
                            {
                                Ok(item) => {
                                    pane.update_in(cx, |pane, window, cx| {
                                        navigated |= Some(item.item_id()) != prev_active_item_id;
                                        pane.nav_history_mut().set_mode(NavigationMode::Normal);
                                        if let Some(data) = entry.data {
                                            navigated |= item.navigate(data, window, cx);
                                        }
                                    })?;
                                }
                                Err(open_by_abs_path_e) => {
                                    log::error!("Failed to navigate history: {open_by_project_path_e:#} and {open_by_abs_path_e:#}");
                                }
                            }
                        }
                    }
                }

                if !navigated {
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            Self::navigate_history(workspace, pane, mode, window, cx)
                        })?
                        .await?;
                }

                Ok(())
            })
        } else {
            Task::ready(Ok(()))
        }
    }

    pub fn go_back(
        &mut self,
        pane: WeakEntity<Pane>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        self.navigate_history(pane, NavigationMode::GoingBack, window, cx)
    }

    pub fn go_forward(
        &mut self,
        pane: WeakEntity<Pane>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        self.navigate_history(pane, NavigationMode::GoingForward, window, cx)
    }

    pub fn reopen_closed_item(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        self.navigate_history(
            self.active_pane().downgrade(),
            NavigationMode::ReopeningClosedItem,
            window,
            cx,
        )
    }

    pub fn client(&self) -> &Arc<Client> {
        &self.app_state.client
    }

    pub fn set_titlebar_item(&mut self, item: AnyView, _: &mut Window, cx: &mut Context<Self>) {
        self.titlebar_item = Some(item);
        cx.notify();
    }

    pub fn set_prompt_for_new_path(&mut self, prompt: PromptForNewPath) {
        self.on_prompt_for_new_path = Some(prompt)
    }

    pub fn set_prompt_for_open_path(&mut self, prompt: PromptForOpenPath) {
        self.on_prompt_for_open_path = Some(prompt)
    }

    pub fn set_terminal_provider(&mut self, provider: impl TerminalProvider + 'static) {
        self.terminal_provider = Some(Box::new(provider));
    }

    pub fn set_debugger_provider(&mut self, provider: impl DebuggerProvider + 'static) {
        self.debugger_provider = Some(Arc::new(provider));
    }

    pub fn set_open_in_dev_container(&mut self, value: bool) {
        self.open_in_dev_container = value;
    }

    pub fn open_in_dev_container(&self) -> bool {
        self.open_in_dev_container
    }

    pub fn set_dev_container_task(&mut self, task: Task<Result<()>>) {
        self._dev_container_task = Some(task);
    }

    pub fn debugger_provider(&self) -> Option<Arc<dyn DebuggerProvider>> {
        self.debugger_provider.clone()
    }

    pub fn prompt_for_open_path(
        &mut self,
        path_prompt_options: PathPromptOptions,
        lister: DirectoryLister,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Option<Vec<PathBuf>>> {
        // TODO: If `on_prompt_for_open_path` is set, we should always use it
        // rather than gating on `use_system_path_prompts`. This would let tests
        // inject a mock without also having to disable the setting.
        if !lister.is_local(cx) || !WorkspaceSettings::get_global(cx).use_system_path_prompts {
            let prompt = self.on_prompt_for_open_path.take().unwrap();
            let rx = prompt(self, lister, window, cx);
            self.on_prompt_for_open_path = Some(prompt);
            rx
        } else {
            let (tx, rx) = oneshot::channel();
            let abs_path = cx.prompt_for_paths(path_prompt_options);

            cx.spawn_in(window, async move |workspace, cx| {
                // A dropped channel means the platform never fired the dialog's completion
                // handler, so the failure carries no message of its own. Route it through the
                // same fallback as a reported failure — returning early here would leave the
                // action looking like it simply did nothing.
                let reported_error = match abs_path.await {
                    Ok(Ok(selection)) => {
                        tx.send(selection).ok();
                        return anyhow::Ok(());
                    }
                    Ok(Err(err)) => Some(err),
                    Err(oneshot::Canceled) => None,
                };

                let rx = workspace.update_in(cx, |workspace, window, cx| {
                    match reported_error {
                        Some(err) => workspace
                            .show_error(workspace_error::PortalError::new(err.to_string()), cx),
                        None => workspace.show_error(workspace_error::SystemPathPromptError, cx),
                    }
                    let prompt = workspace.on_prompt_for_open_path.take().unwrap();
                    let rx = prompt(workspace, lister, window, cx);
                    workspace.on_prompt_for_open_path = Some(prompt);
                    rx
                })?;
                if let Ok(path) = rx.await {
                    tx.send(path).ok();
                }
                anyhow::Ok(())
            })
            .detach();

            rx
        }
    }

    pub fn prompt_for_new_path(
        &mut self,
        lister: DirectoryLister,
        suggested_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Option<Vec<PathBuf>>> {
        if self.project.read(cx).is_via_collab()
            || self.project.read(cx).is_via_remote_server()
            || !WorkspaceSettings::get_global(cx).use_system_path_prompts
        {
            let prompt = self.on_prompt_for_new_path.take().unwrap();
            let rx = prompt(self, lister, suggested_name, window, cx);
            self.on_prompt_for_new_path = Some(prompt);
            return rx;
        }

        let (tx, rx) = oneshot::channel();
        cx.spawn_in(window, async move |workspace, cx| {
            let abs_path = workspace.update(cx, |workspace, cx| {
                let relative_to = workspace
                    .most_recent_active_path(cx)
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .or_else(|| {
                        let project = workspace.project.read(cx);
                        project.visible_worktrees(cx).find_map(|worktree| {
                            Some(worktree.read(cx).as_local()?.abs_path().to_path_buf())
                        })
                    })
                    .or_else(std::env::home_dir)
                    .unwrap_or_else(|| PathBuf::from(""));
                cx.prompt_for_new_path(&relative_to, suggested_name.as_deref())
            })?;
            // A dropped channel means the platform never fired the dialog's completion
            // handler, so the failure carries no message of its own. Route it through the
            // same fallback as a reported failure; propagating it with `?` would only bury
            // it in a detached task, where nothing surfaces it.
            let reported_error = match abs_path.await {
                Ok(Ok(abs_path)) => {
                    tx.send(abs_path.map(|path| vec![path])).ok();
                    return anyhow::Ok(());
                }
                Ok(Err(err)) => Some(err),
                Err(oneshot::Canceled) => None,
            };

            let rx = workspace.update_in(cx, |workspace, window, cx| {
                match reported_error {
                    Some(err) => {
                        workspace.show_error(workspace_error::PortalError::new(err.to_string()), cx)
                    }
                    None => workspace.show_error(workspace_error::SystemPathPromptError, cx),
                }

                let prompt = workspace.on_prompt_for_new_path.take().unwrap();
                let rx = prompt(workspace, lister, suggested_name, window, cx);
                workspace.on_prompt_for_new_path = Some(prompt);
                rx
            })?;
            if let Ok(path) = rx.await {
                tx.send(path).ok();
            }
            anyhow::Ok(())
        })
        .detach();

        rx
    }

    pub fn titlebar_item(&self) -> Option<AnyView> {
        self.titlebar_item.clone()
    }

    /// Call the given callback with a workspace whose project is local or remote via WSL (allowing host access).
    ///
    /// If the given workspace has a local project, then it will be passed
    /// to the callback. Otherwise, a new empty window will be created.
    pub fn with_local_workspace<T, F>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        callback: F,
    ) -> Task<Result<T>>
    where
        T: 'static,
        F: 'static + FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) -> T,
    {
        if self.project.read(cx).is_local() {
            Task::ready(Ok(callback(self, window, cx)))
        } else {
            let env = self.project.read(cx).cli_environment(cx);
            let task = Self::new_local(
                Vec::new(),
                self.app_state.clone(),
                None,
                env,
                None,
                OpenMode::Activate,
                cx,
            );
            cx.spawn_in(window, async move |_vh, cx| {
                let OpenResult {
                    window: multi_workspace_window,
                    ..
                } = task.await?;
                multi_workspace_window.update(cx, |multi_workspace, window, cx| {
                    let workspace = multi_workspace.workspace().clone();
                    workspace.update(cx, |workspace, cx| callback(workspace, window, cx))
                })
            })
        }
    }

    /// Call the given callback with a workspace whose project is local or remote via WSL (allowing host access).
    ///
    /// If the given workspace has a local project, then it will be passed
    /// to the callback. Otherwise, a new empty window will be created.
    pub fn with_local_or_wsl_workspace<T, F>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        callback: F,
    ) -> Task<Result<T>>
    where
        T: 'static,
        F: 'static + FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) -> T,
    {
        let project = self.project.read(cx);
        if project.is_local() || project.is_via_wsl_with_host_interop(cx) {
            Task::ready(Ok(callback(self, window, cx)))
        } else {
            let env = self.project.read(cx).cli_environment(cx);
            let task = Self::new_local(
                Vec::new(),
                self.app_state.clone(),
                None,
                env,
                None,
                OpenMode::Activate,
                cx,
            );
            cx.spawn_in(window, async move |_vh, cx| {
                let OpenResult {
                    window: multi_workspace_window,
                    ..
                } = task.await?;
                multi_workspace_window.update(cx, |multi_workspace, window, cx| {
                    let workspace = multi_workspace.workspace().clone();
                    workspace.update(cx, |workspace, cx| callback(workspace, window, cx))
                })
            })
        }
    }

    pub fn worktrees<'a>(&self, cx: &'a App) -> impl 'a + Iterator<Item = Entity<Worktree>> {
        self.project.read(cx).worktrees(cx)
    }

    pub fn visible_worktrees<'a>(
        &self,
        cx: &'a App,
    ) -> impl 'a + Iterator<Item = Entity<Worktree>> {
        self.project.read(cx).visible_worktrees(cx)
    }

    pub fn worktree_scans_complete(&self, cx: &App) -> impl Future<Output = ()> + 'static + use<> {
        let futures = self
            .worktrees(cx)
            .filter_map(|worktree| worktree.read(cx).as_local())
            .map(|worktree| worktree.scan_complete())
            .collect::<Vec<_>>();
        async move {
            for future in futures {
                future.await;
            }
        }
    }

    pub fn close_global(cx: &mut App) {
        cx.defer(|cx| {
            cx.windows().iter().find(|window| {
                window
                    .update(cx, |_, window, _| {
                        if window.is_window_active() {
                            //This can only get called when the window's project connection has been lost
                            //so we don't need to prompt the user for anything and instead just close the window
                            window.remove_window();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false)
            });
        });
    }

    pub fn move_focused_panel_to_next_position(
        &mut self,
        _: &MoveFocusedPanelToNextPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let docks = self.all_docks();
        let active_dock = docks
            .into_iter()
            .find(|dock| dock.focus_handle(cx).contains_focused(window, cx));

        if let Some(dock) = active_dock {
            dock.update(cx, |dock, cx| {
                let active_panel = dock
                    .active_panel()
                    .filter(|panel| panel.panel_focus_handle(cx).contains_focused(window, cx));

                if let Some(panel) = active_panel {
                    panel.move_to_next_position(window, cx);
                }
            })
        }
    }

    pub fn prepare_to_close(
        &mut self,
        close_intent: CloseIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool>> {
        let active_call = self.active_global_call();

        cx.spawn_in(window, async move |this, cx| {
            this.update(cx, |this, _| {
                if close_intent == CloseIntent::CloseWindow {
                    this.removing = true;
                }
            })?;

            let workspace_count = cx.update(|_window, cx| {
                cx.windows()
                    .iter()
                    .filter(|window| window.downcast::<MultiWorkspace>().is_some())
                    .count()
            })?;

            #[cfg(target_os = "macos")]
            let save_last_workspace = false;

            // On Linux and Windows, closing the last window should restore the last workspace.
            #[cfg(not(target_os = "macos"))]
            let save_last_workspace = {
                let remaining_workspaces = cx.update(|_window, cx| {
                    cx.windows()
                        .iter()
                        .filter_map(|window| window.downcast::<MultiWorkspace>())
                        .filter_map(|multi_workspace| {
                            multi_workspace
                                .update(cx, |multi_workspace, _, cx| {
                                    multi_workspace.workspace().read(cx).removing
                                })
                                .ok()
                        })
                        .filter(|removing| !removing)
                        .count()
                })?;

                close_intent != CloseIntent::ReplaceWindow && remaining_workspaces == 0
            };

            if let Some(active_call) = active_call
                && workspace_count == 1
                && cx
                    .update(|_window, cx| active_call.0.is_in_room(cx))
                    .unwrap_or(false)
            {
                if close_intent == CloseIntent::CloseWindow {
                    this.update(cx, |_, cx| cx.emit(Event::Activate))?;
                    let answer = cx.update(|window, cx| {
                        window.prompt(
                            PromptLevel::Warning,
                            "Do you want to leave the current call?",
                            None,
                            &["Close window and hang up", "Cancel"],
                            cx,
                        )
                    })?;

                    if answer.await.log_err() == Some(1) {
                        return anyhow::Ok(false);
                    } else {
                        if let Ok(task) = cx.update(|_window, cx| active_call.0.hang_up(cx)) {
                            task.await.log_err();
                        }
                    }
                }
                if close_intent == CloseIntent::ReplaceWindow {
                    _ = cx.update(|_window, cx| {
                        let multi_workspace = cx
                            .windows()
                            .iter()
                            .filter_map(|window| window.downcast::<MultiWorkspace>())
                            .next()
                            .unwrap();
                        let project = multi_workspace
                            .read(cx)?
                            .workspace()
                            .read(cx)
                            .project
                            .clone();
                        if project.read(cx).is_shared() {
                            active_call.0.unshare_project(project, cx)?;
                        }
                        Ok::<_, anyhow::Error>(())
                    });
                }
            }

            // Hot-exit silently writes dirty buffers to the DB; only allow it
            // if the workspace will be reachable again, either via session
            // restore or by reopening its folder paths. Otherwise prompt, so
            // we don't orphan the buffers.
            let allow_hot_exit_serialization = close_intent == CloseIntent::Quit
                || save_last_workspace
                || this
                    .read_with(cx, |workspace, cx| {
                        workspace
                            .project
                            .read(cx)
                            .visible_worktrees(cx)
                            .next()
                            .is_some()
                    })
                    .unwrap_or(false);
            let save_result = this
                .update_in(cx, |this, window, cx| {
                    this.save_all_internal(
                        SaveIntent::Close,
                        allow_hot_exit_serialization,
                        window,
                        cx,
                    )
                })?
                .await;

            // If we're not quitting, but closing, we remove the workspace from
            // the current session.
            if close_intent != CloseIntent::Quit
                && !save_last_workspace
                && save_result.as_ref().is_ok_and(|&res| res)
            {
                this.update_in(cx, |this, window, cx| this.remove_from_session(window, cx))?
                    .await;
            }

            save_result
        })
    }

    fn save_all(&mut self, action: &SaveAll, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all_internal(
            action.save_intent.unwrap_or(SaveIntent::SaveAll),
            true,
            window,
            cx,
        )
        .detach_and_log_err(cx);
    }

    fn send_keystrokes(
        &mut self,
        action: &SendKeystrokes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystrokes: Vec<Keystroke> = action
            .0
            .split(' ')
            .flat_map(|k| Keystroke::parse(k).log_err())
            .map(|k| {
                cx.keyboard_mapper()
                    .map_key_equivalent(k, false)
                    .inner()
                    .clone()
            })
            .collect();
        let _ = self.send_keystrokes_impl(keystrokes, window, cx);
    }

    pub fn send_keystrokes_impl(
        &mut self,
        keystrokes: Vec<Keystroke>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Shared<Task<()>> {
        let mut state = self.dispatching_keystrokes.borrow_mut();
        if !state.dispatched.insert(keystrokes.clone()) {
            cx.propagate();
            return state.task.clone().unwrap();
        }

        state.queue.extend(keystrokes);

        let keystrokes = self.dispatching_keystrokes.clone();
        if state.task.is_none() {
            state.task = Some(
                window
                    .spawn(cx, async move |cx| {
                        // limit to 100 keystrokes to avoid infinite recursion.
                        for _ in 0..100 {
                            let keystroke = {
                                let mut state = keystrokes.borrow_mut();
                                let Some(keystroke) = state.queue.pop_front() else {
                                    state.dispatched.clear();
                                    state.task.take();
                                    return;
                                };
                                keystroke
                            };
                            let focus_changed = cx
                                .update(|window, cx| {
                                    let focused = window.focused(cx);
                                    window.dispatch_keystroke(keystroke.clone(), cx);
                                    if window.focused(cx) != focused {
                                        // dispatch_keystroke may cause the focus to change.
                                        // draw's side effect is to schedule the FocusChanged events in the current flush effect cycle
                                        // And we need that to happen before the next keystroke to keep vim mode happy...
                                        // (Note that the tests always do this implicitly, so you must manually test with something like:
                                        //   "bindings": { "g z": ["workspace::SendKeystrokes", ": j <enter> u"]}
                                        // )
                                        window.draw(cx).clear(cx);
                                        return true;
                                    }
                                    false
                                })
                                .unwrap_or(false);

                            if focus_changed {
                                futures_lite::future::yield_now().await;
                            }
                        }

                        *keystrokes.borrow_mut() = Default::default();
                        log::error!("over 100 keystrokes passed to send_keystrokes");
                    })
                    .shared(),
            );
        }
        state.task.clone().unwrap()
    }

    /// Prompts the user to save or discard each dirty item, returning
    /// `true` if they confirmed (saved/discarded everything) or `false`
    /// if they cancelled. Used before removing worktree roots during
    /// thread archival.
    pub fn prompt_to_save_or_discard_dirty_items(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool>> {
        self.save_all_internal(SaveIntent::Close, true, window, cx)
    }

    fn save_all_internal(
        &mut self,
        mut save_intent: SaveIntent,
        allow_hot_exit_serialization: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool>> {
        if self.project.read(cx).is_disconnected(cx) {
            return Task::ready(Ok(true));
        }
        let dirty_items = self
            .panes
            .iter()
            .flat_map(|pane| {
                pane.read(cx).items().filter_map(|item| {
                    if item.is_dirty(cx) {
                        item.tab_content_text(0, cx);
                        Some((pane.clone(), item.boxed_clone()))
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();

        let project = self.project.clone();
        cx.spawn_in(window, async move |workspace, cx| {
            let dirty_items = if save_intent == SaveIntent::Close && !dirty_items.is_empty() {
                let mut serialize_tasks = Vec::new();
                let mut remaining_dirty_items = Vec::new();
                if allow_hot_exit_serialization {
                    workspace.update_in(cx, |workspace, window, cx| {
                        for (pane, item) in dirty_items {
                            if let Some(task) = item
                                .to_serializable_item_handle(cx)
                                .and_then(|handle| handle.serialize(workspace, true, window, cx))
                            {
                                serialize_tasks.push((pane, item, task));
                            } else {
                                remaining_dirty_items.push((pane, item));
                            }
                        }
                    })?;

                    for (pane, item, task) in serialize_tasks {
                        if task.await.log_err().is_none() {
                            remaining_dirty_items.push((pane, item));
                        }
                    }
                } else {
                    remaining_dirty_items = dirty_items;
                }

                if !remaining_dirty_items.is_empty() {
                    workspace.update(cx, |_, cx| cx.emit(Event::Activate))?;
                }

                if remaining_dirty_items.len() > 1 {
                    let answer = workspace.update_in(cx, |_, window, cx| {
                        cx.emit(Event::Activate);
                        let detail = Pane::file_names_for_prompt(
                            &mut remaining_dirty_items.iter().map(|(_, handle)| handle),
                            cx,
                        );
                        window.prompt(
                            PromptLevel::Warning,
                            "Do you want to save all changes in the following files?",
                            Some(&detail),
                            &["Save all", "Discard all", "Cancel"],
                            cx,
                        )
                    })?;
                    match answer.await.log_err() {
                        Some(0) => save_intent = SaveIntent::SaveAll,
                        Some(1) => save_intent = SaveIntent::Skip,
                        Some(2) => return Ok(false),
                        _ => {}
                    }
                }

                remaining_dirty_items
            } else {
                dirty_items
            };

            for (pane, item) in dirty_items {
                let (singleton, project_entry_ids) = cx.update(|_, cx| {
                    (
                        item.buffer_kind(cx) == ItemBufferKind::Singleton,
                        item.project_entry_ids(cx),
                    )
                })?;
                if (singleton || !project_entry_ids.is_empty())
                    && !Pane::save_item(project.clone(), pane, &*item, save_intent, cx).await?
                {
                    return Ok(false);
                }
            }
            Ok(true)
        })
    }

    /// Whether this workspace holds nothing a user would lose by reusing its window.
    ///
    /// `#zed-66.3`: opening a project into an empty workspace reuses its window **even
    /// when the setting asks for a new one** — `open_workspace_for_paths` coerces
    /// `OpenMode::NewWindow` to `Activate` here. Anything deciding "does this open into
    /// the current window" has to ask this, not the configured mode, or it will reason
    /// about a new window that never gets created.
    pub fn holds_nothing(&self, cx: &App) -> bool {
        !self.project.read(cx).is_via_collab()
            && self.project.read(cx).worktrees(cx).next().is_none()
            && !self.items(cx).any(|item| item.is_dirty(cx))
    }

    pub fn open_workspace_for_paths(
        &mut self,
        // replace_current_window: bool,
        mut open_mode: OpenMode,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Workspace>>> {
        let requesting_window = window.window_handle().downcast::<MultiWorkspace>();

        if self.holds_nothing(cx) {
            open_mode = OpenMode::Activate;
        }

        let app_state = self.app_state.clone();

        cx.spawn(async move |_, cx| {
            let OpenResult { workspace, .. } = cx
                .update(|cx| {
                    open_paths(
                        &paths,
                        app_state,
                        OpenOptions {
                            requesting_window,
                            open_mode,
                            workspace_matching: if open_mode == OpenMode::NewWindow {
                                WorkspaceMatching::None
                            } else {
                                WorkspaceMatching::default()
                            },
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .await?;
            Ok(workspace)
        })
    }

    #[allow(clippy::type_complexity)]
    pub fn open_paths(
        &mut self,
        mut abs_paths: Vec<PathBuf>,
        options: OpenOptions,
        pane: Option<WeakEntity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Vec<Option<anyhow::Result<Box<dyn ItemHandle>>>>> {
        let fs = self.app_state.fs.clone();

        let caller_ordered_abs_paths = abs_paths.clone();

        // Sort the paths to ensure we add worktrees for parents before their children.
        abs_paths.sort_unstable();
        cx.spawn_in(window, async move |this, cx| {
            let mut tasks = Vec::with_capacity(abs_paths.len());

            for abs_path in &abs_paths {
                let visible = match options.visible.as_ref().unwrap_or(&OpenVisible::None) {
                    OpenVisible::All => Some(true),
                    OpenVisible::None => Some(false),
                    OpenVisible::OnlyFiles => match fs.metadata(abs_path).await.log_err() {
                        Some(Some(metadata)) => Some(!metadata.is_dir),
                        Some(None) => Some(true),
                        None => None,
                    },
                    OpenVisible::OnlyDirectories => match fs.metadata(abs_path).await.log_err() {
                        Some(Some(metadata)) => Some(metadata.is_dir),
                        Some(None) => Some(false),
                        None => None,
                    },
                };
                let project_path = match visible {
                    Some(visible) => match this
                        .update(cx, |this, cx| {
                            Workspace::project_path_for_path(
                                this.project.clone(),
                                abs_path,
                                visible,
                                cx,
                            )
                        })
                        .log_err()
                    {
                        Some(project_path) => project_path.await.log_err(),
                        None => None,
                    },
                    None => None,
                };

                let this = this.clone();
                let abs_path: Arc<Path> = SanitizedPath::new(&abs_path).as_path().into();
                let fs = fs.clone();
                let pane = pane.clone();
                let task = cx.spawn(async move |cx| {
                    let (worktree, project_path) = project_path?;
                    let (entry_is_directory, worktree_is_local) =
                        worktree.read_with(cx, |worktree, _| {
                            let entry = if project_path.path.as_unix_str().is_empty() {
                                worktree.root_entry()
                            } else {
                                worktree.entry_for_path(&project_path.path)
                            };
                            (entry.map(|entry| entry.is_dir()), worktree.is_local())
                        });
                    let is_directory = match entry_is_directory {
                        Some(is_directory) => is_directory,
                        None if worktree_is_local => fs.is_dir(&abs_path).await,
                        None => false,
                    };

                    if is_directory {
                        // Opening a directory should not race to update the active entry.
                        // We'll select/reveal a deterministic final entry after all paths finish opening.
                        None
                    } else {
                        Some(
                            this.update_in(cx, |this, window, cx| {
                                this.open_path(
                                    project_path,
                                    pane,
                                    options.focus.unwrap_or(true),
                                    window,
                                    cx,
                                )
                            })
                            .ok()?
                            .await,
                        )
                    }
                });
                tasks.push(task);
            }

            let results = futures::future::join_all(tasks).await;

            // Determine the winner using the fake/abstract FS metadata, not `Path::is_dir`.
            let mut winner: Option<(PathBuf, bool)> = None;
            for abs_path in caller_ordered_abs_paths.into_iter().rev() {
                if let Some(Some(metadata)) = fs.metadata(&abs_path).await.log_err() {
                    if !metadata.is_dir {
                        winner = Some((abs_path, false));
                        break;
                    }
                    if winner.is_none() {
                        winner = Some((abs_path, true));
                    }
                } else if winner.is_none() {
                    winner = Some((abs_path, false));
                }
            }

            // Compute the winner entry id on the foreground thread and emit once, after all
            // paths finish opening. This avoids races between concurrently-opening paths
            // (directories in particular) and makes the resulting project panel selection
            // deterministic.
            if let Some((winner_abs_path, winner_is_dir)) = winner {
                'emit_winner: {
                    let winner_abs_path: Arc<Path> =
                        SanitizedPath::new(&winner_abs_path).as_path().into();

                    let visible = match options.visible.as_ref().unwrap_or(&OpenVisible::None) {
                        OpenVisible::All => true,
                        OpenVisible::None => false,
                        OpenVisible::OnlyFiles => !winner_is_dir,
                        OpenVisible::OnlyDirectories => winner_is_dir,
                    };

                    let Some(worktree_task) = this
                        .update(cx, |workspace, cx| {
                            workspace.project.update(cx, |project, cx| {
                                project.find_or_create_worktree(
                                    winner_abs_path.as_ref(),
                                    visible,
                                    cx,
                                )
                            })
                        })
                        .ok()
                    else {
                        break 'emit_winner;
                    };

                    let Ok((worktree, _)) = worktree_task.await else {
                        break 'emit_winner;
                    };

                    let Ok(Some(entry_id)) = this.update(cx, |_, cx| {
                        let worktree = worktree.read(cx);
                        let worktree_abs_path = worktree.abs_path();
                        let entry = if winner_abs_path.as_ref() == worktree_abs_path.as_ref() {
                            worktree.root_entry()
                        } else {
                            winner_abs_path
                                .strip_prefix(worktree_abs_path.as_ref())
                                .ok()
                                .and_then(|relative_path| {
                                    let relative_path =
                                        RelPath::new(relative_path, PathStyle::local())
                                            .log_err()?;
                                    worktree.entry_for_path(&relative_path)
                                })
                        }?;
                        Some(entry.id)
                    }) else {
                        break 'emit_winner;
                    };

                    this.update(cx, |workspace, cx| {
                        workspace.project.update(cx, |_, cx| {
                            cx.emit(project::Event::ActiveEntryChanged(Some(entry_id)));
                        });
                    })
                    .ok();
                }
            }

            results
        })
    }

    pub fn open_resolved_path(
        &mut self,
        path: ResolvedPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        match path {
            ResolvedPath::ProjectPath { project_path, .. } => {
                self.open_path(project_path, None, true, window, cx)
            }
            ResolvedPath::AbsPath { path, .. } => self.open_abs_path(
                PathBuf::from(path),
                OpenOptions {
                    visible: Some(OpenVisible::None),
                    ..Default::default()
                },
                window,
                cx,
            ),
        }
    }

    pub fn absolute_path_of_worktree(
        &self,
        worktree_id: WorktreeId,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        self.project
            .read(cx)
            .worktree_for_id(worktree_id, cx)
            // TODO: use `abs_path` or `root_dir`
            .map(|wt| wt.read(cx).abs_path().as_ref().to_path_buf())
    }

    pub fn add_folder_to_project(
        &mut self,
        _: &AddFolderToProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project.read(cx);
        if project.is_via_collab() {
            self.show_error("You cannot add folders to someone else's project", cx);
            return;
        }
        let paths = self.prompt_for_open_path(
            PathPromptOptions {
                files: false,
                directories: true,
                multiple: true,
                prompt: None,
            },
            DirectoryLister::Project(self.project.clone()),
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Some(paths) = paths.await.log_err().flatten() {
                let results = this
                    .update_in(cx, |this, window, cx| {
                        this.open_paths(
                            paths,
                            OpenOptions {
                                visible: Some(OpenVisible::All),
                                ..Default::default()
                            },
                            None,
                            window,
                            cx,
                        )
                    })?
                    .await;
                for result in results.into_iter().flatten() {
                    result.log_err();
                }
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    pub fn project_path_for_path(
        project: Entity<Project>,
        abs_path: &Path,
        visible: bool,
        cx: &mut App,
    ) -> Task<Result<(Entity<Worktree>, ProjectPath)>> {
        let entry = project.update(cx, |project, cx| {
            project.find_or_create_worktree(abs_path, visible, cx)
        });
        cx.spawn(async move |cx| {
            let (worktree, path) = entry.await?;
            let worktree_id = worktree.read_with(cx, |t, _| t.id());
            Ok((worktree, ProjectPath { worktree_id, path }))
        })
    }

    pub fn items<'a>(&'a self, cx: &'a App) -> impl 'a + Iterator<Item = &'a Box<dyn ItemHandle>> {
        self.panes.iter().flat_map(|pane| pane.read(cx).items())
    }

    pub fn item_of_type<T: Item>(&self, cx: &App) -> Option<Entity<T>> {
        self.items_of_type(cx).max_by_key(|item| item.item_id())
    }

    pub fn items_of_type<'a, T: Item>(
        &'a self,
        cx: &'a App,
    ) -> impl 'a + Iterator<Item = Entity<T>> {
        self.panes
            .iter()
            .flat_map(|pane| pane.read(cx).items_of_type())
    }

    pub fn active_item(&self, cx: &App) -> Option<Box<dyn ItemHandle>> {
        self.active_pane().read(cx).active_item()
    }

    pub fn active_item_as<I: 'static>(&self, cx: &App) -> Option<Entity<I>> {
        let item = self.active_item(cx)?;
        item.to_any_view().downcast::<I>().ok()
    }

    fn active_project_path(&self, cx: &App) -> Option<ProjectPath> {
        self.active_item(cx).and_then(|item| item.project_path(cx))
    }

    pub fn most_recent_active_path(&self, cx: &App) -> Option<PathBuf> {
        self.recent_navigation_history_iter(cx)
            .filter_map(|(path, abs_path)| {
                let worktree = self
                    .project
                    .read(cx)
                    .worktree_for_id(path.worktree_id, cx)?;
                if !worktree.read(cx).is_visible() {
                    return None;
                }
                let settings_location = SettingsLocation {
                    worktree_id: path.worktree_id,
                    path: &path.path,
                };
                if WorktreeSettings::get(Some(settings_location), cx).is_path_read_only(&path.path)
                {
                    return None;
                }
                abs_path
            })
            .next()
    }

    pub fn save_active_item(
        &mut self,
        save_intent: SaveIntent,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let project = self.project.clone();
        let pane = self.active_pane().clone();
        let item = pane.read(cx).active_item();

        window.spawn(cx, async move |cx| {
            if let Some(item) = item {
                Pane::save_item(project, pane, item.as_ref(), save_intent, cx)
                    .await
                    .map(|_| ())
            } else {
                Ok(())
            }
        })
    }

    pub fn close_inactive_items_and_panes(
        &mut self,
        action: &CloseInactiveTabsAndPanes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(task) = self.close_all_internal(
            true,
            action.save_intent.unwrap_or(SaveIntent::Close),
            window,
            cx,
        ) {
            task.detach_and_log_err(cx)
        }
    }

    pub fn close_all_items_and_panes(
        &mut self,
        action: &CloseAllItemsAndPanes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(task) = self.close_all_internal(
            false,
            action.save_intent.unwrap_or(SaveIntent::Close),
            window,
            cx,
        ) {
            task.detach_and_log_err(cx)
        }
    }

    /// Closes the active item across all panes.
    pub fn close_item_in_all_panes(
        &mut self,
        action: &CloseItemInAllPanes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(active_item) = self.active_pane().read(cx).active_item() else {
            return;
        };

        let save_intent = action.save_intent.unwrap_or(SaveIntent::Close);
        let close_pinned = action.close_pinned;

        if let Some(project_path) = active_item.project_path(cx) {
            self.close_items_with_project_path(
                &project_path,
                save_intent,
                close_pinned,
                window,
                cx,
            );
        } else if close_pinned || !self.active_pane().read(cx).is_active_item_pinned() {
            let item_id = active_item.item_id();
            self.active_pane().update(cx, |pane, cx| {
                pane.close_item_by_id(item_id, save_intent, window, cx)
                    .detach_and_log_err(cx);
            });
        }
    }

    /// Closes all items with the given project path across all panes.
    pub fn close_items_with_project_path(
        &mut self,
        project_path: &ProjectPath,
        save_intent: SaveIntent,
        close_pinned: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panes = self.panes().to_vec();
        for pane in panes {
            pane.update(cx, |pane, cx| {
                pane.close_items_for_project_path(
                    project_path,
                    save_intent,
                    close_pinned,
                    window,
                    cx,
                )
                .detach_and_log_err(cx);
            });
        }
    }

    fn close_all_internal(
        &mut self,
        retain_active_pane: bool,
        save_intent: SaveIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        let current_pane = self.active_pane();

        let mut tasks = Vec::new();

        if retain_active_pane {
            let current_pane_close = current_pane.update(cx, |pane, cx| {
                pane.close_other_items(
                    &CloseOtherItems {
                        save_intent: None,
                        close_pinned: false,
                    },
                    None,
                    window,
                    cx,
                )
            });

            tasks.push(current_pane_close);
        }

        for pane in self.panes() {
            if retain_active_pane && pane.entity_id() == current_pane.entity_id() {
                continue;
            }

            let close_pane_items = pane.update(cx, |pane: &mut Pane, cx| {
                pane.close_all_items(
                    &CloseAllItems {
                        save_intent: Some(save_intent),
                        close_pinned: false,
                    },
                    window,
                    cx,
                )
            });

            tasks.push(close_pane_items)
        }

        if tasks.is_empty() {
            None
        } else {
            Some(cx.spawn_in(window, async move |_, _| {
                for task in tasks {
                    task.await?
                }
                Ok(())
            }))
        }
    }

    pub fn is_dock_at_position_open(&self, position: DockPosition, cx: &mut Context<Self>) -> bool {
        self.dock_at_position(position).read(cx).is_open()
    }

    pub fn toggle_dock(
        &mut self,
        dock_side: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut focus_center = false;
        let mut reveal_dock = false;

        let other_is_zoomed = self.zoomed.is_some() && self.zoomed_position != Some(dock_side);
        let was_visible = self.is_dock_at_position_open(dock_side, cx) && !other_is_zoomed;

        if let Some(panel) = self.dock_at_position(dock_side).read(cx).active_panel() {
            telemetry::event!(
                "Panel Button Clicked",
                name = panel.persistent_name(),
                toggle_state = !was_visible
            );
        }
        if was_visible {
            self.save_open_dock_positions(cx);
        }

        let dock = self.dock_at_position(dock_side);
        dock.update(cx, |dock, cx| {
            dock.set_open(!was_visible, window, cx);

            if dock.active_panel().is_none() {
                let Some(panel_ix) = dock
                    .first_enabled_panel_idx(cx)
                    .log_with_level(log::Level::Info)
                else {
                    return;
                };
                dock.activate_panel(panel_ix, window, cx);
            }

            if let Some(active_panel) = dock.active_panel() {
                if was_visible {
                    if active_panel
                        .panel_focus_handle(cx)
                        .contains_focused(window, cx)
                    {
                        focus_center = true;
                    }
                } else {
                    let focus_handle = active_panel.activation_focus_handle(cx);
                    window.focus(&focus_handle, cx);
                    reveal_dock = true;
                }
            }
        });

        if reveal_dock {
            self.dismiss_zoomed_items_to_reveal(Some(dock_side), window, cx);
        }

        if focus_center {
            self.active_pane
                .update(cx, |pane, cx| window.focus(&pane.focus_handle(cx), cx))
        }

        cx.notify();
        self.serialize_workspace(window, cx);
    }

    fn active_dock(&self, window: &Window, cx: &Context<Self>) -> Option<&Entity<Dock>> {
        self.all_docks().into_iter().find(|&dock| {
            dock.read(cx).is_open() && dock.focus_handle(cx).contains_focused(window, cx)
        })
    }

    fn close_active_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(dock) = self.active_dock(window, cx).cloned() {
            self.save_open_dock_positions(cx);
            dock.update(cx, |dock, cx| {
                dock.set_open(false, window, cx);
            });
            return true;
        }
        false
    }

    pub fn close_all_docks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_open_dock_positions(cx);
        for dock in self.all_docks() {
            dock.update(cx, |dock, cx| {
                dock.set_open(false, window, cx);
            });
        }

        cx.focus_self(window);
        cx.notify();
        self.serialize_workspace(window, cx);
    }

    fn get_open_dock_positions(&self, cx: &Context<Self>) -> Vec<DockPosition> {
        self.all_docks()
            .into_iter()
            .filter_map(|dock| {
                let dock_ref = dock.read(cx);
                if dock_ref.is_open() {
                    Some(dock_ref.position())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Saves the positions of currently open docks.
    ///
    /// Updates `last_open_dock_positions` with positions of all currently open
    /// docks, to later be restored by the 'Toggle All Docks' action.
    fn save_open_dock_positions(&mut self, cx: &mut Context<Self>) {
        let open_dock_positions = self.get_open_dock_positions(cx);
        if !open_dock_positions.is_empty() {
            self.last_open_dock_positions = open_dock_positions;
        }
    }

    /// Toggles all docks between open and closed states.
    ///
    /// If any docks are open, closes all and remembers their positions. If all
    /// docks are closed, restores the last remembered dock configuration.
    fn toggle_all_docks(
        &mut self,
        _: &ToggleAllDocks,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open_dock_positions = self.get_open_dock_positions(cx);

        if !open_dock_positions.is_empty() {
            self.close_all_docks(window, cx);
        } else if !self.last_open_dock_positions.is_empty() {
            self.restore_last_open_docks(window, cx);
        }
    }

    /// Reopens docks from the most recently remembered configuration.
    ///
    /// Opens all docks whose positions are stored in `last_open_dock_positions`
    /// and clears the stored positions.
    fn restore_last_open_docks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let positions_to_open = std::mem::take(&mut self.last_open_dock_positions);

        for position in positions_to_open {
            let dock = self.dock_at_position(position);
            dock.update(cx, |dock, cx| dock.set_open(true, window, cx));
        }

        cx.focus_self(window);
        cx.notify();
        self.serialize_workspace(window, cx);
    }

    /// Transfer focus to the panel of the given type.
    /// Bottom-bar icon behavior: closed → open and focus; open but unfocused →
    /// focus; open and focused → close it in this workspace, leaving siblings.
    pub(crate) fn handle_panel_icon_click(
        &mut self,
        dock: &Entity<Dock>,
        panel_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = dock.read(cx).panel_at(panel_ix).cloned() else {
            return;
        };
        let position = dock.read(cx).position();
        // A plain toggle: open closes, closed opens and takes focus. Focus is a
        // consequence of opening rather than a state of its own, so closing a
        // visible component never costs a second click. A component that is
        // already on screen can be focused by clicking the component itself.
        let is_open = dock.read(cx).is_panel_open(panel.panel_id());
        telemetry::event!(
            "Panel Button Clicked",
            name = panel.persistent_name(),
            toggle_state = !is_open
        );
        if is_open {
            dock.update(cx, |dock, cx| {
                dock.close_panel_at(panel_ix, window, cx);
            });
            self.active_pane
                .update(cx, |pane, cx| window.focus(&pane.focus_handle(cx), cx));
        } else {
            dock.update(cx, |dock, cx| dock.activate_panel(panel_ix, window, cx));
            window.focus(&panel.activation_focus_handle(cx), cx);
            self.dismiss_zoomed_items_to_reveal(Some(position), window, cx);
        }
        self.serialize_workspace(window, cx);
        cx.notify();
    }

    pub fn focus_panel<T: Panel>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<T>> {
        let panel = self.focus_or_unfocus_panel::<T>(window, cx, &mut |_, _, _| true)?;
        panel.to_any().downcast().ok()
    }

    /// Focus the panel of the given type if it isn't already focused. If it is
    /// already focused, then transfer focus back to the workspace center.
    /// When the `close_panel_on_toggle` setting is enabled, also closes the
    /// panel when transferring focus back to the center.
    pub fn toggle_panel_focus<T: Panel>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut did_focus_panel = false;
        self.focus_or_unfocus_panel::<T>(window, cx, &mut |panel, window, cx| {
            did_focus_panel = !panel.panel_focus_handle(cx).contains_focused(window, cx);
            did_focus_panel
        });

        if !did_focus_panel && WorkspaceSettings::get_global(cx).close_panel_on_toggle {
            self.close_panel::<T>(window, cx);
        }

        telemetry::event!(
            "Panel Button Clicked",
            name = T::persistent_name(),
            toggle_state = did_focus_panel
        );

        did_focus_panel
    }

    pub fn focus_center_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.active_item(cx) {
            item.item_focus_handle(cx).focus(window, cx);
        } else {
            log::error!("Could not find a focus target when switching focus to the center panes",);
        }
    }

    pub fn activate_panel_for_proto_id(
        &mut self,
        panel_id: PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Arc<dyn PanelHandle>> {
        let mut panel = None;
        for dock in self.all_docks() {
            if let Some(panel_index) = dock.read(cx).panel_index_for_proto_id(panel_id) {
                panel = dock.update(cx, |dock, cx| {
                    dock.activate_panel(panel_index, window, cx);
                    dock.set_open(true, window, cx);
                    dock.active_panel().cloned()
                });
                break;
            }
        }

        if panel.is_some() {
            cx.notify();
            self.serialize_workspace(window, cx);
        }

        panel
    }

    /// Focus or unfocus the given panel type, depending on the given callback.
    fn focus_or_unfocus_panel<T: Panel>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        should_focus: &mut dyn FnMut(&dyn PanelHandle, &mut Window, &mut Context<Dock>) -> bool,
    ) -> Option<Arc<dyn PanelHandle>> {
        let mut result_panel = None;
        let mut serialize = false;
        for dock in self.all_docks() {
            if let Some(panel_index) = dock.read(cx).panel_index_for_type::<T>() {
                let mut focus_center = false;
                let panel = dock.update(cx, |dock, cx| {
                    dock.activate_panel(panel_index, window, cx);

                    let panel = dock.active_panel().cloned();
                    if let Some(panel) = panel.as_ref() {
                        if should_focus(&**panel, window, cx) {
                            dock.set_open(true, window, cx);
                            panel.activation_focus_handle(cx).focus(window, cx);
                        } else {
                            focus_center = true;
                        }
                    }
                    panel
                });

                if focus_center {
                    self.active_pane
                        .update(cx, |pane, cx| window.focus(&pane.focus_handle(cx), cx))
                }

                result_panel = panel;
                serialize = true;
                break;
            }
        }

        if serialize {
            self.serialize_workspace(window, cx);
        }

        cx.notify();
        result_panel
    }

    /// Open the panel of the given type
    pub fn open_panel<T: Panel>(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for dock in self.all_docks() {
            if let Some(panel_index) = dock.read(cx).panel_index_for_type::<T>() {
                dock.update(cx, |dock, cx| {
                    dock.activate_panel(panel_index, window, cx);
                    dock.set_open(true, window, cx);
                });
            }
        }
    }

    /// Open the panel of the given type, dismissing any zoomed items that
    /// would obscure it (e.g. a zoomed terminal).
    pub fn reveal_panel<T: Panel>(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dock_position = self.all_docks().iter().find_map(|dock| {
            let dock = dock.read(cx);
            dock.panel_index_for_type::<T>().map(|_| dock.position())
        });
        self.dismiss_zoomed_items_to_reveal(dock_position, window, cx);
        self.open_panel::<T>(window, cx);
    }

    pub fn close_panel<T: Panel>(&self, window: &mut Window, cx: &mut Context<Self>) {
        for dock in self.all_docks().iter() {
            dock.update(cx, |dock, cx| {
                if let Some(panel_ix) = dock.panel_index_for_type::<T>() {
                    dock.close_panel_at(panel_ix, window, cx);
                }
            })
        }
    }

    pub fn panel<T: Panel>(&self, cx: &App) -> Option<Entity<T>> {
        self.all_docks()
            .iter()
            .find_map(|dock| dock.read(cx).panel::<T>())
    }

    /// The live view of a registered workspace component, by its layout-registry
    /// id.
    ///
    /// `None` means no panel has registered under that id in this workspace —
    /// the id names a status control, an extension whose crate is absent, or a
    /// panel whose async load has not landed yet. A caller that gets `None` still
    /// has the static registry row, which is what lets a surface offer to
    /// un-hide something that is not running.
    pub fn component_adapter(&self, panel_key: &str, cx: &App) -> Option<ComponentAdapter> {
        self.all_docks().iter().find_map(|dock| {
            let dock = dock.read(cx);
            let panel = dock.panel_for_key(panel_key)?;
            let panel_id = panel.panel_id();
            Some(ComponentAdapter::new(
                panel.clone(),
                dock.is_panel_open(panel_id),
                dock.focused_panel_id() == Some(panel_id),
            ))
        })
    }

    fn dismiss_zoomed_items_to_reveal(
        &mut self,
        dock_to_reveal: Option<DockPosition>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If a center pane is zoomed, unzoom it.
        for pane in &self.panes {
            if pane != &self.active_pane || dock_to_reveal.is_some() {
                pane.update(cx, |pane, cx| pane.set_zoomed(false, cx));
            }
        }

        // If another dock is zoomed, hide it.
        let mut focus_center = false;
        for dock in self.all_docks() {
            dock.update(cx, |dock, cx| {
                if Some(dock.position()) != dock_to_reveal
                    && let Some(panel) = dock.active_panel()
                    && panel.is_zoomed(window, cx)
                {
                    focus_center |= panel.panel_focus_handle(cx).contains_focused(window, cx);
                    dock.set_open(false, window, cx);
                }
            });
        }

        if focus_center {
            self.active_pane
                .update(cx, |pane, cx| window.focus(&pane.focus_handle(cx), cx))
        }

        if self.zoomed_position != dock_to_reveal {
            self.zoomed = None;
            self.zoomed_position = None;
            cx.emit(Event::ZoomChanged);
        }

        cx.notify();
    }

    fn add_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Pane> {
        let pane = cx.new(|cx| {
            let mut pane = Pane::new(
                self.weak_handle(),
                self.project.clone(),
                self.pane_history_timestamp.clone(),
                None,
                NewFile.boxed_clone(),
                true,
                window,
                cx,
            );
            pane.set_can_split(Some(Arc::new(|_, _, _, _| true)));
            pane
        });
        cx.subscribe_in(&pane, window, Self::handle_pane_event)
            .detach();
        self.panes.push(pane.clone());

        window.focus(&pane.focus_handle(cx), cx);

        cx.emit(Event::PaneAdded(pane.clone()));
        pane
    }

    pub fn add_item_to_center(
        &mut self,
        item: Box<dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(center_pane) = self.last_active_center_pane.clone() {
            if let Some(center_pane) = center_pane.upgrade() {
                center_pane.update(cx, |pane, cx| {
                    pane.add_item(item, true, true, None, window, cx)
                });
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn add_item_to_active_pane(
        &mut self,
        item: Box<dyn ItemHandle>,
        destination_index: Option<usize>,
        focus_item: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.add_item(
            self.active_pane.clone(),
            item,
            destination_index,
            false,
            focus_item,
            window,
            cx,
        )
    }

    pub fn add_item(
        &mut self,
        pane: Entity<Pane>,
        item: Box<dyn ItemHandle>,
        destination_index: Option<usize>,
        activate_pane: bool,
        focus_item: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        pane.update(cx, |pane, cx| {
            pane.add_item(
                item,
                activate_pane,
                focus_item,
                destination_index,
                window,
                cx,
            )
        });
    }

    pub fn split_item(
        &mut self,
        split_direction: SplitDirection,
        item: Box<dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_pane = self.split_pane(self.active_pane.clone(), split_direction, window, cx);
        self.add_item(new_pane, item, None, true, true, window, cx);
    }

    pub fn open_abs_path(
        &mut self,
        abs_path: PathBuf,
        options: OpenOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        cx.spawn_in(window, async move |workspace, cx| {
            let open_paths_task_result = workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.open_paths(vec![abs_path.clone()], options, None, window, cx)
                })
                .with_context(|| format!("open abs path {abs_path:?} task spawn"))?
                .await;
            anyhow::ensure!(
                open_paths_task_result.len() == 1,
                "open abs path {abs_path:?} task returned incorrect number of results"
            );
            match open_paths_task_result
                .into_iter()
                .next()
                .expect("ensured single task result")
            {
                Some(open_result) => {
                    open_result.with_context(|| format!("open abs path {abs_path:?} task join"))
                }
                None => anyhow::bail!("open abs path {abs_path:?} task returned None"),
            }
        })
    }

    pub fn split_abs_path(
        &mut self,
        abs_path: PathBuf,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let project_path_task =
            Workspace::project_path_for_path(self.project.clone(), &abs_path, visible, cx);
        cx.spawn_in(window, async move |this, cx| {
            let (_, path) = project_path_task.await?;
            this.update_in(cx, |this, window, cx| this.split_path(path, window, cx))?
                .await
        })
    }

    pub fn open_path(
        &mut self,
        path: impl Into<ProjectPath>,
        pane: Option<WeakEntity<Pane>>,
        focus_item: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        self.open_path_preview(path, pane, focus_item, false, true, window, cx)
    }

    pub fn open_path_preview(
        &mut self,
        path: impl Into<ProjectPath>,
        pane: Option<WeakEntity<Pane>>,
        focus_item: bool,
        allow_preview: bool,
        activate: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let pane = pane.unwrap_or_else(|| {
            self.last_active_center_pane.clone().unwrap_or_else(|| {
                self.panes
                    .first()
                    .expect("There must be an active pane")
                    .downgrade()
            })
        });

        let project_path = path.into();
        let task = self.load_path(project_path.clone(), window, cx);
        window.spawn(cx, async move |cx| {
            let (project_entry_id, build_item) = task.await?;

            pane.update_in(cx, |pane, window, cx| {
                pane.open_item(
                    project_entry_id,
                    project_path,
                    focus_item,
                    allow_preview,
                    activate,
                    None,
                    window,
                    cx,
                    build_item,
                )
            })
        })
    }

    /// Opens a URL or file path, intelligently routing to the appropriate handler:
    ///
    /// - `http://` and `https://` URLs are opened in the system's default browser
    /// - `file://` URIs are opened as files in the editor
    /// - Paths without a URL scheme are treated as file paths:
    ///   - Absolute paths are opened directly
    ///   - Relative paths are first resolved against `base_path` (if provided),
    ///     then against visible project worktrees
    /// - Other URI schemes (e.g., `mailto:`, `vscode:`) are passed to the system handler
    ///
    /// # Arguments
    /// * `url_or_path` - The URL or file path to open
    /// * `base_path` - Optional base directory for resolving relative paths (e.g., the
    ///   directory containing a markdown file). If not provided, relative paths are
    ///   resolved against project worktrees.
    ///
    /// This method provides a unified way to handle links that may be either URLs
    /// or file paths, such as those found in markdown documents, terminal output,
    /// or LSP responses.
    pub fn open_url_or_file(
        &mut self,
        url_or_path: &str,
        base_path: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut open_abs_path = |this: &mut Self, path, cx: &mut _| {
            let url_or_path = url_or_path.to_owned();
            let task = this.open_abs_path(
                path,
                OpenOptions {
                    visible: Some(OpenVisible::None),
                    ..Default::default()
                },
                window,
                cx,
            );
            (**cx)
                .spawn(async move |cx| {
                    if task.await.is_err() {
                        cx.update(|cx| cx.open_url(&url_or_path));
                    }
                })
                .detach();
        };

        if let Ok(url) = Url::parse(url_or_path) {
            match url.scheme() {
                "http" | "https" => cx.open_url(url_or_path),
                "file" => open_abs_path(self, PathBuf::from(url.path()), cx),
                _ => cx.open_url(url_or_path),
            }
            return;
        }

        // Not a valid URL - treat as a file path
        let project = self.project();
        let path_style = project.read(cx).path_style(cx);
        let project_is_local = project.read(cx).is_local();

        // If it's an absolute path, open it directly
        if path_style.is_absolute(url_or_path) {
            open_abs_path(self, PathBuf::from(url_or_path), cx);
            return;
        }

        let path = Path::new(url_or_path);
        // Try to resolve relative path against base_path first
        let path_from_base = if let Some(base) = base_path {
            let resolved_path = path_style.join(base, path).map(PathBuf::from);
            if project_is_local {
                if let Some(resolved_path) = resolved_path
                    && resolved_path.exists()
                {
                    open_abs_path(self, resolved_path, cx);
                    return;
                }
                None
            } else {
                resolved_path
            }
        } else {
            None
        };

        // Try to resolve against project worktrees
        let project_path = project.update(cx, |project, cx| {
            path_from_base
                .as_deref()
                .and_then(|base_path| project.find_project_path(base_path, cx))
                .or_else(|| project.find_project_path(path, cx))
        });
        if let Some(project_path) = project_path {
            let url_or_path = url_or_path.to_owned();
            let task = self.open_path(project_path, None, true, window, cx);
            (**cx)
                .spawn(async move |cx| {
                    if task.await.is_err() {
                        cx.update(|cx| cx.open_url(&url_or_path));
                    }
                })
                .detach();
            return;
        }

        // Couldn't resolve as a file path - try opening as URL anyway
        // (the OS might be able to handle it)
        cx.open_url(url_or_path);
    }

    pub fn split_path(
        &mut self,
        path: impl Into<ProjectPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        self.split_path_preview(path, false, None, window, cx)
    }

    pub fn split_path_preview(
        &mut self,
        path: impl Into<ProjectPath>,
        allow_preview: bool,
        split_direction: Option<SplitDirection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let pane = self.last_active_center_pane.clone().unwrap_or_else(|| {
            self.panes
                .first()
                .expect("There must be an active pane")
                .downgrade()
        });

        if let Member::Pane(center_pane) = &self.center.root
            && center_pane.read(cx).items_len() == 0
        {
            return self.open_path(path, Some(pane), true, window, cx);
        }

        let project_path = path.into();
        let task = self.load_path(project_path.clone(), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let (project_entry_id, build_item) = task.await?;
            this.update_in(cx, move |this, window, cx| -> Option<_> {
                let pane = pane.upgrade()?;
                let new_pane = this.split_pane(
                    pane,
                    split_direction.unwrap_or(SplitDirection::Right),
                    window,
                    cx,
                );
                new_pane.update(cx, |new_pane, cx| {
                    Some(new_pane.open_item(
                        project_entry_id,
                        project_path,
                        true,
                        allow_preview,
                        true,
                        None,
                        window,
                        cx,
                        build_item,
                    ))
                })
            })
            .map(|option| option.context("pane was dropped"))?
        })
    }

    fn load_path(
        &mut self,
        path: ProjectPath,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<(Option<ProjectEntryId>, WorkspaceItemBuilder)>> {
        let registry = cx.default_global::<ProjectItemRegistry>().clone();
        registry.open_path(self.project(), &path, window, cx)
    }

    pub fn find_project_item<T>(
        &self,
        pane: &Entity<Pane>,
        project_item: &Entity<T::Item>,
        cx: &App,
    ) -> Option<Entity<T>>
    where
        T: ProjectItem,
    {
        use project::ProjectItem as _;
        let project_item = project_item.read(cx);
        let entry_id = project_item.entry_id(cx);
        let project_path = project_item.project_path(cx);

        let mut item = None;
        if let Some(entry_id) = entry_id {
            item = pane.read(cx).item_for_entry(entry_id, cx);
        }
        if item.is_none()
            && let Some(project_path) = project_path
        {
            item = pane.read(cx).item_for_path(project_path, cx);
        }

        item.and_then(|item| item.downcast::<T>())
    }

    pub fn is_project_item_open<T>(
        &self,
        pane: &Entity<Pane>,
        project_item: &Entity<T::Item>,
        cx: &App,
    ) -> bool
    where
        T: ProjectItem,
    {
        self.find_project_item::<T>(pane, project_item, cx)
            .is_some()
    }

    pub fn open_project_item<T>(
        &mut self,
        pane: Entity<Pane>,
        project_item: Entity<T::Item>,
        activate_pane: bool,
        focus_item: bool,
        keep_old_preview: bool,
        allow_new_preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<T>
    where
        T: ProjectItem,
    {
        let old_item_id = pane.read(cx).active_item().map(|item| item.item_id());

        if let Some(item) = self.find_project_item(&pane, &project_item, cx) {
            if !keep_old_preview
                && let Some(old_id) = old_item_id
                && old_id != item.item_id()
            {
                // switching to a different item, so unpreview old active item
                pane.update(cx, |pane, _| {
                    pane.unpreview_item_if_preview(old_id);
                });
            }

            self.activate_item(&item, activate_pane, focus_item, window, cx);
            if !allow_new_preview {
                pane.update(cx, |pane, _| {
                    pane.unpreview_item_if_preview(item.item_id());
                });
            }
            return item;
        }

        let item = pane.update(cx, |pane, cx| {
            cx.new(|cx| {
                T::for_project_item(self.project().clone(), Some(pane), project_item, window, cx)
            })
        });
        let mut destination_index = None;
        pane.update(cx, |pane, cx| {
            if !keep_old_preview && let Some(old_id) = old_item_id {
                pane.unpreview_item_if_preview(old_id);
            }
            if allow_new_preview {
                destination_index = pane.replace_preview_item_id(item.item_id(), window, cx);
            }
        });

        self.add_item(
            pane,
            Box::new(item.clone()),
            destination_index,
            activate_pane,
            focus_item,
            window,
            cx,
        );
        item
    }

    pub fn open_shared_screen(
        &mut self,
        peer_id: PeerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(shared_screen) =
            self.shared_screen_for_peer(peer_id, &self.active_pane, window, cx)
        {
            self.active_pane.update(cx, |pane, cx| {
                pane.add_item(Box::new(shared_screen), false, true, None, window, cx)
            });
        }
    }

    pub fn auto_watch_state(&self) -> &AutoWatch {
        &self.auto_watch
    }

    fn next_watched_peer(&self, cx: &App) -> Option<PeerId> {
        self.active_call()
            .and_then(|call| call.peer_ids_with_video_tracks(cx).first().copied())
    }

    pub fn toggle_auto_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.auto_watch.enabled() {
            self.auto_watch = AutoWatch::Off;
            cx.notify();
            return;
        }

        let active_pane = self.active_pane.clone();
        self.unfollow_in_pane(&active_pane, window, cx);

        let local_is_sharing = self
            .active_call()
            .map_or(false, |call| call.is_sharing_screen(cx));

        if local_is_sharing {
            self.auto_watch = AutoWatch::Paused;
        } else {
            let watched_peer = self.next_watched_peer(cx);
            self.auto_watch = AutoWatch::Active { watched_peer };

            if let Some(peer_id) = watched_peer {
                self.open_shared_screen(peer_id, window, cx);
            }
        }

        cx.notify();
    }

    fn handle_auto_watch_video_tracks_changed(
        &mut self,
        peer_id: PeerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let AutoWatch::Active { watched_peer } = self.auto_watch else {
            return;
        };

        let peer_is_sharing = self.active_call().map_or(false, |call| {
            call.peer_ids_with_video_tracks(cx).contains(&peer_id)
        });
        let should_watch_peer = peer_is_sharing && watched_peer.is_none();
        let watched_peer_stopped_sharing = watched_peer == Some(peer_id) && !peer_is_sharing;

        if should_watch_peer || watched_peer_stopped_sharing {
            let next_watched_peer = if should_watch_peer {
                Some(peer_id)
            } else {
                self.next_watched_peer(cx)
            };

            self.auto_watch = AutoWatch::Active {
                watched_peer: next_watched_peer,
            };

            if let Some(next_watched_peer) = next_watched_peer {
                self.open_shared_screen(next_watched_peer, window, cx);
            }
        }
    }

    fn handle_auto_watch_local_share_stopped(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let AutoWatch::Paused = self.auto_watch else {
            return;
        };

        let watched_peer = self.next_watched_peer(cx);
        self.auto_watch = AutoWatch::Active { watched_peer };

        if let Some(peer_id) = watched_peer {
            self.open_shared_screen(peer_id, window, cx);
        }
    }

    pub fn activate_item(
        &mut self,
        item: &dyn ItemHandle,
        activate_pane: bool,
        focus_item: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let result = self.panes.iter().find_map(|pane| {
            pane.read(cx)
                .index_for_item(item)
                .map(|ix| (pane.clone(), ix))
        });
        if let Some((pane, ix)) = result {
            pane.update(cx, |pane, cx| {
                pane.activate_item(ix, activate_pane, focus_item, window, cx)
            });
            true
        } else {
            false
        }
    }

    fn activate_pane_at_index(
        &mut self,
        action: &ActivatePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panes = self.center.panes();
        if let Some(pane) = panes.get(action.0).map(|p| (*p).clone()) {
            window.focus(&pane.focus_handle(cx), cx);
        } else {
            self.split_and_clone(self.active_pane.clone(), SplitDirection::Right, window, cx)
                .detach();
        }
    }

    fn move_item_to_pane_at_index(
        &mut self,
        action: &MoveItemToPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panes = self.center.panes();
        let destination = match panes.get(action.destination) {
            Some(&destination) => destination.clone(),
            None => {
                if !action.clone && self.active_pane.read(cx).items_len() < 2 {
                    return;
                }
                let direction = SplitDirection::Right;
                let split_off_pane = self
                    .find_pane_in_direction(direction, cx)
                    .unwrap_or_else(|| self.active_pane.clone());
                let new_pane = self.add_pane(window, cx);
                self.center.split(&split_off_pane, &new_pane, direction, cx);
                new_pane
            }
        };

        if action.clone {
            if self
                .active_pane
                .read(cx)
                .active_item()
                .is_some_and(|item| item.can_split(cx))
            {
                clone_active_item(
                    self.database_id(),
                    &self.active_pane,
                    &destination,
                    action.focus,
                    window,
                    cx,
                );
                return;
            }
        }
        move_active_item(
            &self.active_pane,
            &destination,
            action.focus,
            true,
            window,
            cx,
        )
    }

    pub fn activate_next_pane(&mut self, window: &mut Window, cx: &mut App) {
        let panes = self.center.panes();
        if let Some(ix) = panes.iter().position(|pane| **pane == self.active_pane) {
            let next_ix = (ix + 1) % panes.len();
            let next_pane = panes[next_ix].clone();
            window.focus(&next_pane.focus_handle(cx), cx);
        }
    }

    pub fn activate_previous_pane(&mut self, window: &mut Window, cx: &mut App) {
        let panes = self.center.panes();
        if let Some(ix) = panes.iter().position(|pane| **pane == self.active_pane) {
            let prev_ix = cmp::min(ix.wrapping_sub(1), panes.len() - 1);
            let prev_pane = panes[prev_ix].clone();
            window.focus(&prev_pane.focus_handle(cx), cx);
        }
    }

    pub fn activate_last_pane(&mut self, window: &mut Window, cx: &mut App) {
        let last_pane = self.center.last_pane();
        window.focus(&last_pane.focus_handle(cx), cx);
    }

    pub fn activate_pane_in_direction(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut App,
    ) {
        use ActivateInDirectionTarget as Target;
        enum Origin {
            Sidebar,
            LeftDock,
            RightDock,
            BottomDock,
            Center,
        }

        let origin: Origin = if self
            .sidebar_focus_handle
            .as_ref()
            .is_some_and(|h| h.contains_focused(window, cx))
        {
            Origin::Sidebar
        } else {
            [
                (&self.left_dock, Origin::LeftDock),
                (&self.right_dock, Origin::RightDock),
                (&self.bottom_dock, Origin::BottomDock),
            ]
            .into_iter()
            .find_map(|(dock, origin)| {
                if dock.focus_handle(cx).contains_focused(window, cx) && dock.read(cx).is_open() {
                    Some(origin)
                } else {
                    None
                }
            })
            .unwrap_or(Origin::Center)
        };

        let get_last_active_pane = || {
            let pane = self
                .last_active_center_pane
                .clone()
                .unwrap_or_else(|| {
                    self.panes
                        .first()
                        .expect("There must be an active pane")
                        .downgrade()
                })
                .upgrade()?;
            (pane.read(cx).items_len() != 0).then_some(pane)
        };

        let try_dock =
            |dock: &Entity<Dock>| dock.read(cx).is_open().then(|| Target::Dock(dock.clone()));

        let sidebar_target = self
            .sidebar_focus_handle
            .as_ref()
            .map(|h| Target::Sidebar(h.clone()));

        let sidebar_on_right = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map_or(false, |mw| {
                mw.read(cx).sidebar_side(cx) == SidebarSide::Right
            });

        let away_from_sidebar = if sidebar_on_right {
            SplitDirection::Left
        } else {
            SplitDirection::Right
        };

        let (near_dock, far_dock) = if sidebar_on_right {
            (&self.right_dock, &self.left_dock)
        } else {
            (&self.left_dock, &self.right_dock)
        };

        let target = match (origin, direction) {
            (Origin::Sidebar, dir) if dir == away_from_sidebar => try_dock(near_dock)
                .or_else(|| get_last_active_pane().map(Target::Pane))
                .or_else(|| try_dock(&self.bottom_dock))
                .or_else(|| try_dock(far_dock)),

            (Origin::Sidebar, _) => None,

            // We're in the center, so we first try to go to a different pane,
            // otherwise try to go to a dock.
            (Origin::Center, direction) => {
                if let Some(pane) = self.find_pane_in_direction(direction, cx) {
                    Some(Target::Pane(pane))
                } else {
                    match direction {
                        SplitDirection::Up => None,
                        SplitDirection::Down => try_dock(&self.bottom_dock),
                        SplitDirection::Left => {
                            let dock_target = try_dock(&self.left_dock);
                            if sidebar_on_right {
                                dock_target
                            } else {
                                dock_target.or(sidebar_target)
                            }
                        }
                        SplitDirection::Right => {
                            let dock_target = try_dock(&self.right_dock);
                            if sidebar_on_right {
                                dock_target.or(sidebar_target)
                            } else {
                                dock_target
                            }
                        }
                    }
                }
            }

            (Origin::LeftDock, SplitDirection::Right) => {
                if let Some(last_active_pane) = get_last_active_pane() {
                    Some(Target::Pane(last_active_pane))
                } else {
                    try_dock(&self.bottom_dock).or_else(|| try_dock(&self.right_dock))
                }
            }

            (Origin::LeftDock, SplitDirection::Left) => {
                if sidebar_on_right {
                    None
                } else {
                    sidebar_target
                }
            }

            (Origin::LeftDock, SplitDirection::Down)
            | (Origin::RightDock, SplitDirection::Down) => try_dock(&self.bottom_dock),

            (Origin::BottomDock, SplitDirection::Up) => get_last_active_pane().map(Target::Pane),
            (Origin::BottomDock, SplitDirection::Left) => {
                let dock_target = try_dock(&self.left_dock);
                if sidebar_on_right {
                    dock_target
                } else {
                    dock_target.or(sidebar_target)
                }
            }
            (Origin::BottomDock, SplitDirection::Right) => {
                let dock_target = try_dock(&self.right_dock);
                if sidebar_on_right {
                    dock_target.or(sidebar_target)
                } else {
                    dock_target
                }
            }

            (Origin::RightDock, SplitDirection::Left) => {
                if let Some(last_active_pane) = get_last_active_pane() {
                    Some(Target::Pane(last_active_pane))
                } else {
                    try_dock(&self.bottom_dock).or_else(|| try_dock(&self.left_dock))
                }
            }

            (Origin::RightDock, SplitDirection::Right) => {
                if sidebar_on_right {
                    sidebar_target
                } else {
                    None
                }
            }

            _ => None,
        };

        match target {
            Some(ActivateInDirectionTarget::Pane(pane)) => {
                let pane = pane.read(cx);
                if let Some(item) = pane.active_item() {
                    item.item_focus_handle(cx).focus(window, cx);
                } else {
                    log::error!(
                        "Could not find a focus target when in switching focus in {direction} direction for a pane",
                    );
                }
            }
            Some(ActivateInDirectionTarget::Dock(dock)) => {
                // Defer this to avoid a panic when the dock's active panel is already on the stack.
                window.defer(cx, move |window, cx| {
                    let dock = dock.read(cx);
                    if let Some(panel) = dock.active_panel() {
                        panel.activation_focus_handle(cx).focus(window, cx);
                    } else {
                        log::error!("Could not find a focus target when in switching focus in {direction} direction for a {:?} dock", dock.position());
                    }
                })
            }
            Some(ActivateInDirectionTarget::Sidebar(focus_handle)) => {
                focus_handle.focus(window, cx);
            }
            None => {}
        }
    }

    pub fn move_item_to_pane_in_direction(
        &mut self,
        action: &MoveItemToPaneInDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let destination = match self.find_pane_in_direction(action.direction, cx) {
            Some(destination) => destination,
            None => {
                if !action.clone && self.active_pane.read(cx).items_len() < 2 {
                    return;
                }
                let new_pane = self.add_pane(window, cx);
                self.center
                    .split(&self.active_pane, &new_pane, action.direction, cx);
                new_pane
            }
        };

        if action.clone {
            if self
                .active_pane
                .read(cx)
                .active_item()
                .is_some_and(|item| item.can_split(cx))
            {
                clone_active_item(
                    self.database_id(),
                    &self.active_pane,
                    &destination,
                    action.focus,
                    window,
                    cx,
                );
                return;
            }
        }
        move_active_item(
            &self.active_pane,
            &destination,
            action.focus,
            true,
            window,
            cx,
        );
    }

    pub fn bounding_box_for_pane(&self, pane: &Entity<Pane>) -> Option<Bounds<Pixels>> {
        self.center.bounding_box_for_pane(pane)
    }

    pub fn find_pane_in_direction(
        &mut self,
        direction: SplitDirection,
        cx: &App,
    ) -> Option<Entity<Pane>> {
        self.center
            .find_pane_in_direction(&self.active_pane, direction, cx)
            .cloned()
    }

    pub fn swap_pane_in_direction(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        if let Some(to) = self.find_pane_in_direction(direction, cx) {
            self.center.swap(&self.active_pane, &to, cx);
            cx.notify();
        }
    }

    pub fn move_pane_to_border(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        if self
            .center
            .move_to_border(&self.active_pane, direction, cx)
            .unwrap()
        {
            cx.notify();
        }
    }

    pub fn resize_pane(
        &mut self,
        axis: gpui::Axis,
        amount: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let docks = self.all_docks();
        let active_dock = docks
            .into_iter()
            .find(|dock| dock.focus_handle(cx).contains_focused(window, cx));

        if let Some(dock_entity) = active_dock {
            let dock = dock_entity.read(cx);
            let Some(panel_size) = self.dock_size(&dock, window, cx) else {
                return;
            };
            match dock.position() {
                DockPosition::Left => self.resize_left_dock(panel_size + amount, window, cx),
                DockPosition::Bottom => self.resize_bottom_dock(panel_size + amount, window, cx),
                DockPosition::Right => self.resize_right_dock(panel_size + amount, window, cx),
            }
        } else {
            self.center
                .resize(&self.active_pane, axis, amount, &self.bounds, cx);
        }
        cx.notify();
    }

    pub fn reset_pane_sizes(&mut self, cx: &mut Context<Self>) {
        self.center.reset_pane_sizes(cx);
        cx.notify();
    }

    fn handle_pane_focused(
        &mut self,
        pane: Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flush_deferred_saves(window, cx);

        // This is explicitly hoisted out of the following check for pane identity as
        // terminal panel panes are not registered as a center panes.
        self.status_bar.update(cx, |status_bar, cx| {
            status_bar.set_active_pane(&pane, window, cx);
        });
        if self.active_pane != pane {
            self.set_active_pane(&pane, window, cx);
        }

        if self.last_active_center_pane.is_none() {
            self.last_active_center_pane = Some(pane.downgrade());
        }

        // If this pane is in a dock, preserve that dock when dismissing zoomed items.
        // This prevents the dock from closing when focus events fire during window activation.
        // We also preserve any dock whose active panel itself has focus — this covers
        // panels like AgentPanel that don't implement `pane()` but can still be zoomed.
        let dock_to_preserve = self.all_docks().iter().find_map(|dock| {
            let dock_read = dock.read(cx);
            if let Some(panel) = dock_read.active_panel() {
                if panel.pane(cx).is_some_and(|dock_pane| dock_pane == pane)
                    || panel.panel_focus_handle(cx).contains_focused(window, cx)
                {
                    return Some(dock_read.position());
                }
            }
            None
        });

        if let Some(maximized) = &self.maximized_pane {
            let is_center_pane = self.panes.contains(&pane);
            if is_center_pane && maximized.upgrade().as_ref() != Some(&pane) {
                self.maximized_pane = None;
            }
        }

        self.dismiss_zoomed_items_to_reveal(dock_to_preserve, window, cx);
        if pane.read(cx).is_zoomed() {
            self.zoomed = Some(pane.downgrade().into());
        } else {
            self.zoomed = None;
        }
        self.zoomed_position = None;
        cx.emit(Event::ZoomChanged);
        self.update_active_view_for_followers(window, cx);
        pane.update(cx, |pane, _| {
            pane.track_alternate_file_items();
        });

        cx.notify();
    }

    fn set_active_pane(
        &mut self,
        pane: &Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_pane = pane.clone();
        self.active_item_path_changed(true, window, cx);
        self.last_active_center_pane = Some(pane.downgrade());
    }

    fn handle_panel_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_deferred_saves(window, cx);
        self.update_active_view_for_followers(window, cx);
    }

    fn flush_deferred_saves(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let deferred = std::mem::take(&mut self.deferred_save_items);
        for weak_item in deferred {
            let Some(item) = weak_item.upgrade() else {
                continue;
            };
            // Skip if focus returned to this item
            let focus_handle = item.item_focus_handle(cx);
            if focus_handle.contains_focused(window, cx) {
                continue;
            }
            Pane::autosave_item(item.as_ref(), self.project.clone(), window, cx)
                .detach_and_log_err(cx);
        }
    }

    fn handle_pane_event(
        &mut self,
        pane: &Entity<Pane>,
        event: &pane::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut serialize_workspace = true;
        match event {
            pane::Event::AddItem { item } => {
                item.added_to_pane(self, pane.clone(), window, cx);
                cx.emit(Event::ItemAdded {
                    item: item.boxed_clone(),
                });
            }
            pane::Event::Split { direction, mode } => {
                match mode {
                    SplitMode::ClonePane => {
                        self.split_and_clone(pane.clone(), *direction, window, cx)
                            .detach();
                    }
                    SplitMode::EmptyPane => {
                        self.split_pane(pane.clone(), *direction, window, cx);
                    }
                    SplitMode::MovePane => {
                        self.split_and_move(pane.clone(), *direction, window, cx);
                    }
                };
            }
            pane::Event::JoinIntoNext => {
                self.join_pane_into_next(pane.clone(), window, cx);
            }
            pane::Event::JoinAll => {
                self.join_all_panes(window, cx);
            }
            pane::Event::Remove { focus_on_pane } => {
                self.remove_pane(pane.clone(), focus_on_pane.clone(), window, cx);
            }
            pane::Event::ActivateItem {
                local,
                focus_changed,
            } => {
                window.invalidate_character_coordinates();

                pane.update(cx, |pane, _| {
                    pane.track_alternate_file_items();
                });
                if *local {
                    self.unfollow_in_pane(pane, window, cx);
                }
                serialize_workspace = *focus_changed || pane != self.active_pane();
                if pane == self.active_pane() {
                    self.active_item_path_changed(*focus_changed, window, cx);
                    self.update_active_view_for_followers(window, cx);
                } else if *local {
                    self.set_active_pane(pane, window, cx);
                }
            }
            pane::Event::UserSavedItem { item, save_intent } => {
                cx.emit(Event::UserSavedItem {
                    pane: pane.downgrade(),
                    item: item.boxed_clone(),
                    save_intent: *save_intent,
                });
                serialize_workspace = false;
            }
            pane::Event::ChangeItemTitle => {
                if *pane == self.active_pane {
                    self.active_item_path_changed(false, window, cx);
                }
                serialize_workspace = false;
            }
            pane::Event::RemovedItem { item } => {
                cx.emit(Event::ActiveItemChanged);
                self.update_window_edited(window, cx);
                if let hash_map::Entry::Occupied(entry) = self.panes_by_item.entry(item.item_id())
                    && entry.get().entity_id() == pane.entity_id()
                {
                    entry.remove();
                }
                cx.emit(Event::ItemRemoved {
                    item_id: item.item_id(),
                });
            }
            pane::Event::Focus => {
                window.invalidate_character_coordinates();
                self.handle_pane_focused(pane.clone(), window, cx);
            }
            pane::Event::ZoomIn => {
                if *pane == self.active_pane {
                    self.maximized_pane = None;
                    pane.update(cx, |pane, cx| pane.set_zoomed(true, cx));
                    if pane.read(cx).has_focus(window, cx) {
                        self.zoomed = Some(pane.downgrade().into());
                        self.zoomed_position = None;
                        cx.emit(Event::ZoomChanged);
                    }
                    cx.notify();
                }
            }
            pane::Event::ZoomOut => {
                pane.update(cx, |pane, cx| pane.set_zoomed(false, cx));
                if self.zoomed_position.is_none() {
                    self.zoomed = None;
                    cx.emit(Event::ZoomChanged);
                }
                cx.notify();
            }
            pane::Event::ItemPinned | pane::Event::ItemUnpinned => {}
        }

        if serialize_workspace {
            self.serialize_workspace(window, cx);
        }
    }

    pub fn unfollow_in_pane(
        &mut self,
        pane: &Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Option<CollaboratorId> {
        let leader_id = self.leader_for_pane(pane)?;
        self.unfollow(leader_id, window, cx);
        Some(leader_id)
    }

    pub fn split_pane(
        &mut self,
        pane_to_split: Entity<Pane>,
        split_direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let new_pane = self.add_pane(window, cx);
        self.center
            .split(&pane_to_split, &new_pane, split_direction, cx);
        cx.notify();
        new_pane
    }

    pub fn split_and_move(
        &mut self,
        pane: Entity<Pane>,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = pane.update(cx, |pane, cx| pane.take_active_item(window, cx)) else {
            return;
        };
        let new_pane = self.add_pane(window, cx);
        new_pane.update(cx, |pane, cx| {
            pane.add_item(item, true, true, None, window, cx)
        });
        self.center.split(&pane, &new_pane, direction, cx);
        cx.notify();
    }

    pub fn split_and_clone(
        &mut self,
        pane: Entity<Pane>,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Entity<Pane>>> {
        let Some(item) = pane.read(cx).active_item() else {
            return Task::ready(None);
        };
        if !item.can_split(cx) {
            return Task::ready(None);
        }
        let task = item.clone_on_split(self.database_id(), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Some(clone) = task.await {
                this.update_in(cx, |this, window, cx| {
                    let new_pane = this.add_pane(window, cx);
                    let nav_history = pane.read(cx).fork_nav_history();
                    new_pane.update(cx, |pane, cx| {
                        pane.set_nav_history(nav_history, cx);
                        pane.add_item(clone, true, true, None, window, cx)
                    });
                    this.center.split(&pane, &new_pane, direction, cx);
                    cx.notify();
                    new_pane
                })
                .ok()
            } else {
                None
            }
        })
    }

    pub fn join_all_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_item = self.active_pane.read(cx).active_item();
        for pane in &self.panes {
            join_pane_into_active(&self.active_pane, pane, window, cx);
        }
        if let Some(active_item) = active_item {
            self.activate_item(active_item.as_ref(), true, true, window, cx);
        }
        cx.notify();
    }

    pub fn join_pane_into_next(
        &mut self,
        pane: Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next_pane = self
            .find_pane_in_direction(SplitDirection::Right, cx)
            .or_else(|| self.find_pane_in_direction(SplitDirection::Down, cx))
            .or_else(|| self.find_pane_in_direction(SplitDirection::Left, cx))
            .or_else(|| self.find_pane_in_direction(SplitDirection::Up, cx));
        let Some(next_pane) = next_pane else {
            return;
        };
        move_all_items(&pane, &next_pane, window, cx);
        cx.notify();
    }

    fn remove_pane(
        &mut self,
        pane: Entity<Pane>,
        focus_on: Option<Entity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.center.remove(&pane, cx).unwrap() {
            if self
                .maximized_pane
                .as_ref()
                .and_then(|weak| weak.upgrade())
                .as_ref()
                == Some(&pane)
            {
                self.maximized_pane = None;
            }
            self.force_remove_pane(&pane, &focus_on, window, cx);
            self.unfollow_in_pane(&pane, window, cx);
            self.last_leaders_by_pane.remove(&pane.downgrade());
            for removed_item in pane.read(cx).items() {
                self.panes_by_item.remove(&removed_item.item_id());
            }

            cx.notify();
        } else {
            self.active_item_path_changed(true, window, cx);
        }
        cx.emit(Event::PaneRemoved);
    }

    pub fn panes_mut(&mut self) -> &mut [Entity<Pane>] {
        &mut self.panes
    }

    pub fn panes(&self) -> &[Entity<Pane>] {
        &self.panes
    }

    pub fn active_pane(&self) -> &Entity<Pane> {
        &self.active_pane
    }

    pub fn focused_pane(&self, window: &Window, cx: &App) -> Entity<Pane> {
        for dock in self.all_docks() {
            if dock.focus_handle(cx).contains_focused(window, cx)
                && let Some(pane) = dock
                    .read(cx)
                    .active_panel()
                    .and_then(|panel| panel.pane(cx))
            {
                return pane;
            }
        }
        self.active_pane().clone()
    }

    pub fn adjacent_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Pane> {
        self.adjacent_pane_of(&self.active_pane.clone(), window, cx)
    }

    pub fn adjacent_pane_of(
        &mut self,
        origin: &Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        self.center
            .find_pane_in_direction(origin, SplitDirection::Right, cx)
            .cloned()
            .unwrap_or_else(|| self.split_pane(origin.clone(), SplitDirection::Right, window, cx))
    }

    pub fn pane_for(&self, handle: &dyn ItemHandle) -> Option<Entity<Pane>> {
        self.pane_for_item_id(handle.item_id())
    }

    pub fn pane_for_item_id(&self, item_id: EntityId) -> Option<Entity<Pane>> {
        let weak_pane = self.panes_by_item.get(&item_id)?;
        weak_pane.upgrade()
    }

    pub fn pane_for_entity_id(&self, entity_id: EntityId) -> Option<Entity<Pane>> {
        self.panes
            .iter()
            .find(|pane| pane.entity_id() == entity_id)
            .cloned()
    }

    fn collaborator_left(&mut self, peer_id: PeerId, window: &mut Window, cx: &mut Context<Self>) {
        self.follower_states.retain(|leader_id, state| {
            if *leader_id == CollaboratorId::PeerId(peer_id) {
                for item in state.items_by_leader_view_id.values() {
                    item.view.set_leader_id(None, window, cx);
                }
                false
            } else {
                true
            }
        });
        cx.notify();
    }

    pub fn start_following(
        &mut self,
        leader_id: impl Into<CollaboratorId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        let leader_id = leader_id.into();
        let pane = self.active_pane().clone();

        self.last_leaders_by_pane
            .insert(pane.downgrade(), leader_id);
        self.unfollow(leader_id, window, cx);
        self.unfollow_in_pane(&pane, window, cx);
        self.auto_watch = AutoWatch::Off;
        self.follower_states.insert(
            leader_id,
            FollowerState {
                center_pane: pane.clone(),
                dock_pane: None,
                active_view_id: None,
                items_by_leader_view_id: Default::default(),
            },
        );
        cx.notify();

        match leader_id {
            CollaboratorId::PeerId(leader_peer_id) => {
                let room_id = self.active_call()?.room_id(cx)?;
                let project_id = self.project.read(cx).remote_id();
                let request = self.app_state.client.request(proto::Follow {
                    room_id,
                    project_id,
                    leader_id: Some(leader_peer_id),
                });

                Some(cx.spawn_in(window, async move |this, cx| {
                    let response = request.await?;
                    this.update(cx, |this, _| {
                        let state = this
                            .follower_states
                            .get_mut(&leader_id)
                            .context("following interrupted")?;
                        state.active_view_id = response
                            .active_view
                            .as_ref()
                            .and_then(|view| ViewId::from_proto(view.id.clone()?).ok());
                        anyhow::Ok(())
                    })??;
                    if let Some(view) = response.active_view {
                        Self::add_view_from_leader(this.clone(), leader_peer_id, &view, cx).await?;
                    }
                    this.update_in(cx, |this, window, cx| {
                        this.leader_updated(leader_id, window, cx)
                    })?;
                    Ok(())
                }))
            }
            CollaboratorId::Agent => {
                self.leader_updated(leader_id, window, cx)?;
                Some(Task::ready(Ok(())))
            }
        }
    }

    pub fn follow_next_collaborator(
        &mut self,
        _: &FollowNextCollaborator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let collaborators = self.project.read(cx).collaborators();
        let next_leader_id = if let Some(leader_id) = self.leader_for_pane(&self.active_pane) {
            let mut collaborators = collaborators.keys().copied();
            for peer_id in collaborators.by_ref() {
                if CollaboratorId::PeerId(peer_id) == leader_id {
                    break;
                }
            }
            collaborators.next().map(CollaboratorId::PeerId)
        } else if let Some(last_leader_id) =
            self.last_leaders_by_pane.get(&self.active_pane.downgrade())
        {
            match last_leader_id {
                CollaboratorId::PeerId(peer_id) => {
                    if collaborators.contains_key(peer_id) {
                        Some(*last_leader_id)
                    } else {
                        None
                    }
                }
                CollaboratorId::Agent => Some(CollaboratorId::Agent),
            }
        } else {
            None
        };

        let pane = self.active_pane.clone();
        let Some(leader_id) = next_leader_id.or_else(|| {
            Some(CollaboratorId::PeerId(
                collaborators.keys().copied().next()?,
            ))
        }) else {
            return;
        };
        if self.unfollow_in_pane(&pane, window, cx) == Some(leader_id) {
            return;
        }
        if let Some(task) = self.start_following(leader_id, window, cx) {
            task.detach_and_log_err(cx)
        }
    }

    pub fn follow(
        &mut self,
        leader_id: impl Into<CollaboratorId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leader_id = leader_id.into();

        if let CollaboratorId::PeerId(peer_id) = leader_id {
            let Some(active_call) = GlobalAnyActiveCall::try_global(cx) else {
                return;
            };
            let Some(remote_participant) =
                active_call.0.remote_participant_for_peer_id(peer_id, cx)
            else {
                return;
            };

            let project = self.project.read(cx);

            let other_project_id = match remote_participant.location {
                ParticipantLocation::External => None,
                ParticipantLocation::UnsharedProject => None,
                ParticipantLocation::SharedProject { project_id } => {
                    if Some(project_id) == project.remote_id() {
                        None
                    } else {
                        Some(project_id)
                    }
                }
            };

            // if they are active in another project, follow there.
            if let Some(project_id) = other_project_id {
                let app_state = self.app_state.clone();
                crate::join_in_room_project(
                    project_id,
                    remote_participant.user.legacy_id,
                    app_state,
                    cx,
                )
                .detach_and_prompt_err(
                    "Failed to join project",
                    window,
                    cx,
                    |error, _, _| Some(format!("{error:#}")),
                );
            }
        }

        // if you're already following, find the right pane and focus it.
        if let Some(follower_state) = self.follower_states.get(&leader_id) {
            window.focus(&follower_state.pane().focus_handle(cx), cx);

            return;
        }

        // Otherwise, follow.
        if let Some(task) = self.start_following(leader_id, window, cx) {
            task.detach_and_log_err(cx)
        }
    }

    pub fn unfollow(
        &mut self,
        leader_id: impl Into<CollaboratorId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<()> {
        cx.notify();

        let leader_id = leader_id.into();
        let state = self.follower_states.remove(&leader_id)?;
        for (_, item) in state.items_by_leader_view_id {
            item.view.set_leader_id(None, window, cx);
        }

        if let CollaboratorId::PeerId(leader_peer_id) = leader_id {
            let project_id = self.project.read(cx).remote_id();
            let room_id = self.active_call()?.room_id(cx)?;
            self.app_state
                .client
                .send(proto::Unfollow {
                    room_id,
                    project_id,
                    leader_id: Some(leader_peer_id),
                })
                .log_err();
        }

        Some(())
    }

    pub fn is_being_followed(&self, id: impl Into<CollaboratorId>) -> bool {
        self.follower_states.contains_key(&id.into())
    }

    pub(crate) fn active_item_path_changed(
        &mut self,
        focus_changed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(Event::ActiveItemChanged);
        let active_entry = self.active_project_path(cx);
        self.project.update(cx, |project, cx| {
            project.set_active_path(active_entry.clone(), cx)
        });

        if focus_changed && let Some(project_path) = &active_entry {
            let git_store_entity = self.project.read(cx).git_store().clone();
            git_store_entity.update(cx, |git_store, cx| {
                git_store.set_active_repo_for_path(project_path, cx);
            });
        }

        self.update_window_title(window, cx);
    }

    /// Whether this workspace may write the platform window's title and edited
    /// indicator.
    ///
    /// In a multi-workspace window several workspaces share one platform
    /// window, so only the active one is allowed to write that chrome —
    /// otherwise a background workspace's project/item events would clobber the
    /// active workspace's title. `MultiWorkspace` publishes the active
    /// workspace's id into the shared `active_workspace_id` cell, which we
    /// simply compare against our own id. A workspace with no shared cell (e.g.
    /// a plain test window) owns its window unconditionally.
    fn owns_window_chrome(&self) -> bool {
        if self.detached_restore {
            return false;
        }
        match &self.active_workspace_id {
            Some(active_workspace_id) => active_workspace_id.get() == self.weak_self.entity_id(),
            None => true,
        }
    }

    fn update_window_title(&mut self, window: &mut Window, cx: &mut App) {
        if !self.owns_window_chrome() {
            return;
        }
        self.apply_window_title(window, cx);
    }

    fn apply_window_title(&mut self, window: &mut Window, cx: &mut App) {
        let project = self.project().read(cx);
        let mut title = String::new();

        for (i, worktree) in project.visible_worktrees(cx).enumerate() {
            let name = worktree.read(cx).root_name_str();

            if i > 0 {
                title.push_str(", ");
            }
            title.push_str(name);
        }

        if title.is_empty() {
            title = "empty project".to_string();
        }

        let active_project_path = self.active_item(cx).and_then(|item| item.project_path(cx));

        if let Some(path) = active_project_path.as_ref() {
            let filename = path.path.file_name().or_else(|| {
                Some(
                    project
                        .worktree_for_id(path.worktree_id, cx)?
                        .read(cx)
                        .root_name_str(),
                )
            });

            if let Some(filename) = filename {
                title.push_str(" — ");
                title.push_str(filename.as_ref());
            }
        }

        if project.is_via_collab() {
            title.push_str(" ↙");
        } else if project.is_shared() {
            title.push_str(" ↗");
        }

        if let Some(rbf_version) = RbfVersion::try_global(cx) {
            title.push_str(" (rbf v");
            title.push_str(&rbf_version);
            title.push(')');
        }

        let document_path = active_project_path
            .as_ref()
            .and_then(|path| project.absolute_path(path, cx));
        window.set_document_path(document_path.as_deref());

        if let Some(last_title) = self.last_window_title.as_ref()
            && &title == last_title
        {
            return;
        }
        window.set_window_title(&title);
        SystemWindowTabController::update_tab_title(
            cx,
            window.window_handle().window_id(),
            SharedString::from(&title),
        );
        self.last_window_title = Some(title);
    }

    fn is_window_edited(&self, cx: &App) -> bool {
        !self.project.read(cx).is_disconnected(cx) && !self.dirty_items.is_empty()
    }

    fn update_window_edited(&mut self, window: &mut Window, cx: &mut App) {
        if !self.owns_window_chrome() {
            return;
        }
        let is_edited = self.is_window_edited(cx);
        if is_edited != self.window_edited {
            self.window_edited = is_edited;
            window.set_window_edited(self.window_edited)
        }
    }

    /// Re-applies this workspace's title and edited indicator to the platform
    /// window, bypassing the change-detection caches.
    ///
    /// Several workspaces can share a single platform window (a multi-workspace
    /// window). The `last_window_title`/`window_edited` caches assume the
    /// window's title and edited state reflect *this* workspace, but after the
    /// active workspace changes those values may have been set by a different
    /// workspace. Clearing the caches forces the values to be re-applied so the
    /// window reflects the newly-active workspace.
    ///
    /// This is only ever invoked for the workspace that just became active, so
    /// it writes directly rather than going through `update_window_title` /
    /// `update_window_edited`, whose change-detection caches would otherwise
    /// suppress the write when this workspace's computed title/edited state
    /// matches what *it* last set (even though the shared window currently
    /// shows a different workspace's state).
    pub fn refresh_window_state(&mut self, window: &mut Window, cx: &mut App) {
        self.last_window_title = None;
        self.apply_window_title(window, cx);

        self.window_edited = self.is_window_edited(cx);
        window.set_window_edited(self.window_edited);
    }

    fn update_item_dirty_state(
        &mut self,
        item: &dyn ItemHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let is_dirty = item.is_dirty(cx);
        let item_id = item.item_id();
        let was_dirty = self.dirty_items.contains_key(&item_id);
        if is_dirty == was_dirty {
            return false;
        }
        if was_dirty {
            self.dirty_items.remove(&item_id);
            self.update_window_edited(window, cx);
            return true;
        }

        let workspace = self.weak_handle();
        let Some(window_handle) = window.window_handle().downcast::<MultiWorkspace>() else {
            return false;
        };
        let on_release_callback = Box::new(move |cx: &mut App| {
            window_handle
                .update(cx, |_, window, cx| {
                    workspace
                        .update(cx, |workspace, cx| {
                            workspace.dirty_items.remove(&item_id);
                            workspace.update_window_edited(window, cx)
                        })
                        .ok();
                })
                .ok();
        });

        let s = item.on_release(cx, on_release_callback);
        self.dirty_items.insert(item_id, s);
        self.update_window_edited(window, cx);
        true
    }

    fn render_notifications(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Div> {
        if self.notifications.is_empty() {
            None
        } else {
            Some(
                div()
                    .absolute()
                    .right_3()
                    .bottom_3()
                    .w_112()
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .gap_2()
                    .children(
                        self.notifications
                            .iter()
                            .map(|(_, notification)| notification.clone().into_any_element()),
                    ),
            )
        }
    }

    // RPC handlers

    fn active_view_for_follower(
        &self,
        follower_project_id: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<proto::View> {
        let (item, panel_id) = self.active_item_for_followers(window, cx);
        let item = item?;
        let leader_id = self
            .pane_for(&*item)
            .and_then(|pane| self.leader_for_pane(&pane));
        let leader_peer_id = match leader_id {
            Some(CollaboratorId::PeerId(peer_id)) => Some(peer_id),
            Some(CollaboratorId::Agent) | None => None,
        };

        let item_handle = item.to_followable_item_handle(cx)?;
        let id = item_handle.remote_id(&self.app_state.client, window, cx)?;
        let variant = item_handle.to_state_proto(window, cx)?;

        if item_handle.is_project_item(window, cx)
            && (follower_project_id.is_none()
                || follower_project_id != self.project.read(cx).remote_id())
        {
            return None;
        }

        Some(proto::View {
            id: id.to_proto(),
            leader_id: leader_peer_id,
            variant: Some(variant),
            panel_id: panel_id.map(|id| id as i32),
        })
    }

    fn handle_follow(
        &mut self,
        follower_project_id: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> proto::FollowResponse {
        let active_view = self.active_view_for_follower(follower_project_id, window, cx);

        cx.notify();
        proto::FollowResponse {
            views: active_view.iter().cloned().collect(),
            active_view,
        }
    }

    fn handle_update_followers(
        &mut self,
        leader_id: PeerId,
        message: proto::UpdateFollowers,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.leader_updates_tx
            .unbounded_send((leader_id, message))
            .ok();
    }

    async fn process_leader_update(
        this: &WeakEntity<Self>,
        leader_id: PeerId,
        update: proto::UpdateFollowers,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        match update.variant.context("invalid update")? {
            proto::update_followers::Variant::CreateView(view) => {
                let view_id = ViewId::from_proto(view.id.clone().context("invalid view id")?)?;
                let should_add_view = this.update(cx, |this, _| {
                    if let Some(state) = this.follower_states.get_mut(&leader_id.into()) {
                        anyhow::Ok(!state.items_by_leader_view_id.contains_key(&view_id))
                    } else {
                        anyhow::Ok(false)
                    }
                })??;

                if should_add_view {
                    Self::add_view_from_leader(this.clone(), leader_id, &view, cx).await?
                }
            }
            proto::update_followers::Variant::UpdateActiveView(update_active_view) => {
                let should_add_view = this.update(cx, |this, _| {
                    if let Some(state) = this.follower_states.get_mut(&leader_id.into()) {
                        state.active_view_id = update_active_view
                            .view
                            .as_ref()
                            .and_then(|view| ViewId::from_proto(view.id.clone()?).ok());

                        if state.active_view_id.is_some_and(|view_id| {
                            !state.items_by_leader_view_id.contains_key(&view_id)
                        }) {
                            anyhow::Ok(true)
                        } else {
                            anyhow::Ok(false)
                        }
                    } else {
                        anyhow::Ok(false)
                    }
                })??;

                if should_add_view && let Some(view) = update_active_view.view {
                    Self::add_view_from_leader(this.clone(), leader_id, &view, cx).await?
                }
            }
            proto::update_followers::Variant::UpdateView(update_view) => {
                let variant = update_view.variant.context("missing update view variant")?;
                let id = update_view.id.context("missing update view id")?;
                let mut tasks = Vec::new();
                this.update_in(cx, |this, window, cx| {
                    let project = this.project.clone();
                    if let Some(state) = this.follower_states.get(&leader_id.into()) {
                        let view_id = ViewId::from_proto(id.clone())?;
                        if let Some(item) = state.items_by_leader_view_id.get(&view_id) {
                            tasks.push(item.view.apply_update_proto(
                                &project,
                                variant.clone(),
                                window,
                                cx,
                            ));
                        }
                    }
                    anyhow::Ok(())
                })??;
                try_join_all(tasks).await.log_err();
            }
        }
        this.update_in(cx, |this, window, cx| {
            this.leader_updated(leader_id, window, cx)
        })?;
        Ok(())
    }

    async fn add_view_from_leader(
        this: WeakEntity<Self>,
        leader_id: PeerId,
        view: &proto::View,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        let this = this.upgrade().context("workspace dropped")?;

        let Some(id) = view.id.clone() else {
            anyhow::bail!("no id for view");
        };
        let id = ViewId::from_proto(id)?;
        let panel_id = view.panel_id.and_then(proto::PanelId::from_i32);

        let pane = this.update(cx, |this, _cx| {
            let state = this
                .follower_states
                .get(&leader_id.into())
                .context("stopped following")?;
            anyhow::Ok(state.pane().clone())
        })?;
        let existing_item = pane.update_in(cx, |pane, window, cx| {
            let client = this.read(cx).client().clone();
            pane.items().find_map(|item| {
                let item = item.to_followable_item_handle(cx)?;
                if item.remote_id(&client, window, cx) == Some(id) {
                    Some(item)
                } else {
                    None
                }
            })
        })?;
        let item = if let Some(existing_item) = existing_item {
            existing_item
        } else {
            let variant = view.variant.clone();
            anyhow::ensure!(variant.is_some(), "missing view variant");

            let task = cx.update(|window, cx| {
                FollowableViewRegistry::from_state_proto(this.clone(), id, variant, window, cx)
            })?;

            let Some(task) = task else {
                anyhow::bail!(
                    "failed to construct view from leader (maybe from a different version of zed?)"
                );
            };

            let mut new_item = task.await?;
            pane.update_in(cx, |pane, window, cx| {
                let mut item_to_remove = None;
                for (ix, item) in pane.items().enumerate() {
                    if let Some(item) = item.to_followable_item_handle(cx) {
                        match new_item.dedup(item.as_ref(), window, cx) {
                            Some(item::Dedup::KeepExisting) => {
                                new_item =
                                    item.boxed_clone().to_followable_item_handle(cx).unwrap();
                                break;
                            }
                            Some(item::Dedup::ReplaceExisting) => {
                                item_to_remove = Some((ix, item.item_id()));
                                break;
                            }
                            None => {}
                        }
                    }
                }

                if let Some((ix, id)) = item_to_remove {
                    pane.remove_item(id, false, false, window, cx);
                    pane.add_item(new_item.boxed_clone(), false, false, Some(ix), window, cx);
                }
            })?;

            new_item
        };

        this.update_in(cx, |this, window, cx| {
            let state = this.follower_states.get_mut(&leader_id.into())?;
            item.set_leader_id(Some(leader_id.into()), window, cx);
            state.items_by_leader_view_id.insert(
                id,
                FollowerView {
                    view: item,
                    location: panel_id,
                },
            );

            Some(())
        })
        .context("no follower state")?;

        Ok(())
    }

    fn handle_agent_location_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(follower_state) = self.follower_states.get_mut(&CollaboratorId::Agent) else {
            return;
        };

        if let Some(agent_location) = self.project.read(cx).agent_location() {
            let buffer_entity_id = agent_location.buffer.entity_id();
            let view_id = ViewId {
                creator: CollaboratorId::Agent,
                id: buffer_entity_id.as_u64(),
            };
            follower_state.active_view_id = Some(view_id);

            let item = match follower_state.items_by_leader_view_id.entry(view_id) {
                hash_map::Entry::Occupied(entry) => Some(entry.into_mut()),
                hash_map::Entry::Vacant(entry) => {
                    let existing_view =
                        follower_state
                            .center_pane
                            .read(cx)
                            .items()
                            .find_map(|item| {
                                let item = item.to_followable_item_handle(cx)?;
                                if item.buffer_kind(cx) == ItemBufferKind::Singleton
                                    && item.project_item_model_ids(cx).as_slice()
                                        == [buffer_entity_id]
                                {
                                    Some(item)
                                } else {
                                    None
                                }
                            });
                    let view = existing_view.or_else(|| {
                        agent_location.buffer.upgrade().and_then(|buffer| {
                            cx.update_default_global(|registry: &mut ProjectItemRegistry, cx| {
                                registry.build_item(buffer, self.project.clone(), None, window, cx)
                            })?
                            .to_followable_item_handle(cx)
                        })
                    });

                    view.map(|view| {
                        entry.insert(FollowerView {
                            view,
                            location: None,
                        })
                    })
                }
            };

            if let Some(item) = item {
                item.view
                    .set_leader_id(Some(CollaboratorId::Agent), window, cx);
                item.view
                    .update_agent_location(agent_location.position, window, cx);
            }
        } else {
            follower_state.active_view_id = None;
        }

        self.leader_updated(CollaboratorId::Agent, window, cx);
    }

    pub fn update_active_view_for_followers(&mut self, window: &mut Window, cx: &mut App) {
        let mut is_project_item = true;
        let mut update = proto::UpdateActiveView::default();
        if window.is_window_active() {
            let (active_item, panel_id) = self.active_item_for_followers(window, cx);

            if let Some(item) = active_item
                && item.item_focus_handle(cx).contains_focused(window, cx)
            {
                let leader_id = self
                    .pane_for(&*item)
                    .and_then(|pane| self.leader_for_pane(&pane));
                let leader_peer_id = match leader_id {
                    Some(CollaboratorId::PeerId(peer_id)) => Some(peer_id),
                    Some(CollaboratorId::Agent) | None => None,
                };

                if let Some(item) = item.to_followable_item_handle(cx) {
                    let id = item
                        .remote_id(&self.app_state.client, window, cx)
                        .map(|id| id.to_proto());

                    if let Some(id) = id
                        && let Some(variant) = item.to_state_proto(window, cx)
                    {
                        let view = Some(proto::View {
                            id,
                            leader_id: leader_peer_id,
                            variant: Some(variant),
                            panel_id: panel_id.map(|id| id as i32),
                        });

                        is_project_item = item.is_project_item(window, cx);
                        update = proto::UpdateActiveView { view };
                    };
                }
            }
        }

        let active_view_id = update.view.as_ref().and_then(|view| view.id.as_ref());
        if active_view_id != self.last_active_view_id.as_ref() {
            self.last_active_view_id = active_view_id.cloned();
            self.update_followers(
                is_project_item,
                proto::update_followers::Variant::UpdateActiveView(update),
                window,
                cx,
            );
        }
    }

    fn active_item_for_followers(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> (Option<Box<dyn ItemHandle>>, Option<proto::PanelId>) {
        let mut active_item = None;
        let mut panel_id = None;
        for dock in self.all_docks() {
            if dock.focus_handle(cx).contains_focused(window, cx)
                && let Some(panel) = dock.read(cx).active_panel()
                && let Some(pane) = panel.pane(cx)
                && let Some(item) = pane.read(cx).active_item()
            {
                active_item = Some(item);
                panel_id = panel.remote_id();
                break;
            }
        }

        if active_item.is_none() {
            active_item = self.active_pane().read(cx).active_item();
        }
        (active_item, panel_id)
    }

    fn update_followers(
        &self,
        project_only: bool,
        update: proto::update_followers::Variant,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<()> {
        // If this update only applies to for followers in the current project,
        // then skip it unless this project is shared. If it applies to all
        // followers, regardless of project, then set `project_id` to none,
        // indicating that it goes to all followers.
        let project_id = if project_only {
            Some(self.project.read(cx).remote_id()?)
        } else {
            None
        };
        self.app_state().workspace_store.update(cx, |store, cx| {
            store.update_followers(project_id, update, cx)
        })
    }

    pub fn leader_for_pane(&self, pane: &Entity<Pane>) -> Option<CollaboratorId> {
        self.follower_states.iter().find_map(|(leader_id, state)| {
            if state.center_pane == *pane || state.dock_pane.as_ref() == Some(pane) {
                Some(*leader_id)
            } else {
                None
            }
        })
    }

    fn leader_updated(
        &mut self,
        leader_id: impl Into<CollaboratorId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Box<dyn ItemHandle>> {
        cx.notify();

        let leader_id = leader_id.into();
        let (panel_id, item) = match leader_id {
            CollaboratorId::PeerId(peer_id) => self.active_item_for_peer(peer_id, window, cx)?,
            CollaboratorId::Agent => (None, self.active_item_for_agent()?),
        };

        let state = self.follower_states.get(&leader_id)?;
        let mut transfer_focus = state.center_pane.read(cx).has_focus(window, cx);
        let pane;
        if let Some(panel_id) = panel_id {
            pane = self
                .activate_panel_for_proto_id(panel_id, window, cx)?
                .pane(cx)?;
            let state = self.follower_states.get_mut(&leader_id)?;
            state.dock_pane = Some(pane.clone());
        } else {
            pane = state.center_pane.clone();
            let state = self.follower_states.get_mut(&leader_id)?;
            if let Some(dock_pane) = state.dock_pane.take() {
                transfer_focus |= dock_pane.focus_handle(cx).contains_focused(window, cx);
            }
        }

        pane.update(cx, |pane, cx| {
            let focus_active_item = pane.has_focus(window, cx) || transfer_focus;
            if let Some(index) = pane.index_for_item(item.as_ref()) {
                pane.activate_item(index, false, false, window, cx);
            } else {
                pane.add_item(item.boxed_clone(), false, false, None, window, cx)
            }

            if focus_active_item {
                pane.focus_active_item(window, cx)
            }
        });

        Some(item)
    }

    fn active_item_for_agent(&self) -> Option<Box<dyn ItemHandle>> {
        let state = self.follower_states.get(&CollaboratorId::Agent)?;
        let active_view_id = state.active_view_id?;
        Some(
            state
                .items_by_leader_view_id
                .get(&active_view_id)?
                .view
                .boxed_clone(),
        )
    }

    fn active_item_for_peer(
        &self,
        peer_id: PeerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(Option<PanelId>, Box<dyn ItemHandle>)> {
        let call = self.active_call()?;
        let participant = call.remote_participant_for_peer_id(peer_id, cx)?;
        let leader_in_this_app;
        let leader_in_this_project;
        match participant.location {
            ParticipantLocation::SharedProject { project_id } => {
                leader_in_this_app = true;
                leader_in_this_project = Some(project_id) == self.project.read(cx).remote_id();
            }
            ParticipantLocation::UnsharedProject => {
                leader_in_this_app = true;
                leader_in_this_project = false;
            }
            ParticipantLocation::External => {
                leader_in_this_app = false;
                leader_in_this_project = false;
            }
        };
        let state = self.follower_states.get(&peer_id.into())?;
        let mut item_to_activate = None;
        if let (Some(active_view_id), true) = (state.active_view_id, leader_in_this_app) {
            if let Some(item) = state.items_by_leader_view_id.get(&active_view_id)
                && (leader_in_this_project || !item.view.is_project_item(window, cx))
            {
                item_to_activate = Some((item.location, item.view.boxed_clone()));
            }
        } else if let Some(shared_screen) =
            self.shared_screen_for_peer(peer_id, &state.center_pane, window, cx)
        {
            item_to_activate = Some((None, Box::new(shared_screen)));
        }
        item_to_activate
    }

    fn shared_screen_for_peer(
        &self,
        peer_id: PeerId,
        pane: &Entity<Pane>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<SharedScreen>> {
        self.active_call()?
            .create_shared_screen(peer_id, pane, window, cx)
    }

    /// Reacts to this window having moved to a different display.
    ///
    /// Called from the window-bounds observer, which fires on every frame of a drag, so the
    /// identity comparison must come before any other work — re-resolving font sizes per
    /// frame would re-wrap the DisplayMap at frame rate.
    ///
    /// Font sizes themselves are not applied here: every surface re-resolves from its own
    /// window when it renders, and the bounds change already refreshed the window. What does
    /// belong here is the state that has no other owner — a manual `cmd +` adjustment is
    /// discarded on arrival at a new display, so the arriving display's profile wins
    /// outright and what is visible in settings.json stays the whole truth.
    fn handle_possible_display_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let display_id = window.display_id();
        if self.last_display_id == display_id {
            return;
        }
        self.last_display_id = display_id;

        // The set of connected displays may have changed along with this window's, as it
        // does on a dock, undock, or monitor hot-plug.
        theme_settings::refresh_display_profile_assignments(cx);

        theme_settings::reset_ui_font_size(cx);
        theme_settings::reset_buffer_font_size(cx);
        theme_settings::reset_agent_ui_font_size(cx);
        theme_settings::reset_agent_buffer_font_size(cx);
    }

    pub fn on_window_activation_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_window_active() {
            if let Some(database_id) = self.database_id {
                let db = WorkspaceDb::global(cx);
                cx.background_spawn(async move { db.update_timestamp(database_id).await })
                    .detach();
            }
        } else {
            // When window is deactivated, flush any deferred saves since focus has left the window
            self.flush_deferred_saves(window, cx);
            for pane in &self.panes {
                pane.update(cx, |pane, cx| {
                    if let Some(item) = pane.active_item() {
                        item.workspace_deactivated(window, cx);
                    }
                    for item in pane.items() {
                        if matches!(
                            item.workspace_settings(cx).autosave,
                            AutosaveSetting::OnWindowChange | AutosaveSetting::OnFocusChange
                        ) {
                            Pane::autosave_item(item.as_ref(), self.project.clone(), window, cx)
                                .detach_and_log_err(cx);
                        }
                    }
                });
            }
        }
    }

    pub fn active_call(&self) -> Option<&dyn AnyActiveCall> {
        self.active_call.as_ref().map(|(call, _)| &*call.0)
    }

    pub fn active_global_call(&self) -> Option<GlobalAnyActiveCall> {
        self.active_call.as_ref().map(|(call, _)| call.clone())
    }

    fn on_active_call_event(
        &mut self,
        event: &ActiveCallEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ActiveCallEvent::ParticipantLocationChanged { participant_id } => {
                self.leader_updated(participant_id, window, cx);
            }
            ActiveCallEvent::RemoteVideoTracksChanged { participant_id } => {
                self.leader_updated(participant_id, window, cx);
                self.handle_auto_watch_video_tracks_changed(*participant_id, window, cx);
            }
            ActiveCallEvent::LocalScreenShareStarted => {
                if let AutoWatch::Active { .. } = self.auto_watch {
                    self.auto_watch = AutoWatch::Paused;
                    cx.notify();
                }
            }
            ActiveCallEvent::LocalScreenShareStopped => {
                self.handle_auto_watch_local_share_stopped(window, cx);
            }
            ActiveCallEvent::RoomLeft => {
                if self.auto_watch.enabled() {
                    self.auto_watch = AutoWatch::Off;
                    cx.notify();
                }
            }
        }
    }

    pub fn database_id(&self) -> Option<WorkspaceId> {
        self.database_id
    }

    /// `pub` rather than `pub(crate)` so tests in dependent crates can exercise the
    /// `database_id` branch of anything keyed off a workspace. `Workspace::test_new` leaves this
    /// `None`, which silently routes such tests through the `session_id` fallback instead — and
    /// that fallback is app-wide in production, so a test relying on it proves the opposite of
    /// per-workspace scoping.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_database_id(&mut self, id: WorkspaceId) {
        self.database_id = Some(id);
    }

    pub fn session_id(&self) -> Option<String> {
        self.session_id.clone()
    }

    fn save_window_bounds_now(&self, window: &mut Window, cx: &mut App) -> Task<()> {
        let Some(display) = window.display(cx) else {
            return Task::ready(());
        };
        let Ok(display_uuid) = display.uuid() else {
            return Task::ready(());
        };

        let window_bounds = window.inner_window_bounds();
        let database_id = self.database_id;
        let has_paths = !self.root_paths(cx).is_empty();
        let db = WorkspaceDb::global(cx);
        let kvp = db::kvp::KeyValueStore::global(cx);

        cx.background_executor().spawn(async move {
            if !has_paths {
                persistence::write_default_window_bounds(&kvp, window_bounds, display_uuid)
                    .await
                    .log_err();
            }
            if let Some(database_id) = database_id {
                db.set_window_open_status(
                    database_id,
                    SerializedWindowBounds(window_bounds),
                    display_uuid,
                )
                .await
                .log_err();
            } else {
                persistence::write_default_window_bounds(&kvp, window_bounds, display_uuid)
                    .await
                    .log_err();
            }
        })
    }

    fn save_window_bounds(&self, cx: &mut App) -> Task<()> {
        let (send_result, receive_result) = oneshot::channel();
        if let Err(error) = self
            .serializable_items_tx
            .unbounded_send(ItemSerializationCommand::SaveWindowBounds(send_result))
        {
            log::error!("failed to enqueue workspace bounds serialization: {error}");
            return Task::ready(());
        }

        cx.background_spawn(async move {
            if receive_result.await.is_err() {
                log::debug!("workspace serializer dropped bounds serialization");
            }
        })
    }

    /// Bypass the 200ms serialization throttle and write workspace state to
    /// the DB immediately. Returns a task the caller can await to ensure the
    /// write completes. Used by the quit handler so the most recent state
    /// isn't lost to a pending throttle timer when the process exits.
    pub fn flush_serialization(&mut self, window: &mut Window, cx: &mut App) -> Task<()> {
        self._schedule_serialize_workspace.take();
        self._serialize_workspace_task.take();
        self.bounds_save_task_queued.take();

        let bounds_task = self.save_window_bounds(cx);
        let serialize_task = self.serialize_workspace_internal(window, cx);
        cx.spawn(async move |_| {
            bounds_task.await;
            serialize_task.await;
        })
    }

    pub(crate) fn flush_session_serialization_for_shutdown(
        &mut self,
        window_id: WindowId,
        snapshot: Option<ShutdownSerializationSnapshot>,
        cx: &mut App,
    ) -> Task<()> {
        self._schedule_serialize_workspace.take();
        self._serialize_workspace_task.take();
        self.bounds_save_task_queued.take();

        let Some(database_id) = self.database_id() else {
            return Task::ready(());
        };

        fn serialize_pane_handle(
            pane_handle: &Entity<Pane>,
            active_pane: &Entity<Pane>,
            cx: &mut App,
        ) -> SerializedPane {
            let pane = pane_handle.read(cx);
            let active_item_id = pane.active_item().map(|item| item.item_id());
            SerializedPane::new(
                pane.items()
                    .filter_map(|handle| {
                        let handle = handle.to_serializable_item_handle(cx)?;

                        Some(SerializedItem {
                            kind: Arc::from(handle.serialized_item_kind()),
                            item_id: handle.item_id().as_u64(),
                            active: Some(handle.item_id()) == active_item_id,
                            preview: pane.is_active_preview_item(handle.item_id()),
                        })
                    })
                    .collect(),
                pane_handle == active_pane,
                pane.pinned_count(),
            )
        }

        fn build_serialized_pane_group(
            pane_group: &Member,
            active_pane: &Entity<Pane>,
            cx: &mut App,
        ) -> SerializedPaneGroup {
            match pane_group {
                Member::Axis(PaneAxis {
                    axis,
                    members,
                    flexes,
                    bounding_boxes: _,
                }) => SerializedPaneGroup::Group {
                    axis: SerializedAxis(*axis),
                    children: members
                        .iter()
                        .map(|member| build_serialized_pane_group(member, active_pane, cx))
                        .collect(),
                    flexes: Some(flexes.lock().clone()),
                },
                Member::Pane(pane_handle) => {
                    SerializedPaneGroup::Pane(serialize_pane_handle(pane_handle, active_pane, cx))
                }
            }
        }

        match self.workspace_location(cx) {
            WorkspaceLocation::Location(location, paths) => {
                let bookmarks = self.project.update(cx, |project, cx| {
                    project
                        .bookmark_store()
                        .read(cx)
                        .all_serialized_bookmarks(cx)
                });

                let breakpoints = self.project.update(cx, |project, cx| {
                    project
                        .breakpoint_store()
                        .read(cx)
                        .all_source_breakpoints(cx)
                });
                let user_toolchains = self
                    .project
                    .read(cx)
                    .user_toolchains(cx)
                    .unwrap_or_default();
                let center_group =
                    build_serialized_pane_group(&self.center.root, &self.active_pane(), cx);
                let identity_paths_hint = self.project_group_key(cx).path_list().clone();
                let (window_bounds, display, docks, preserve_existing_docks) =
                    if let Some(snapshot) = snapshot {
                        (
                            Some(snapshot.window_bounds),
                            snapshot.display,
                            snapshot.docks,
                            false,
                        )
                    } else {
                        (None, Default::default(), Default::default(), true)
                    };

                let mut serialized_workspace = SerializedWorkspace {
                    id: database_id,
                    location,
                    paths,
                    identity_paths: Some(identity_paths_hint),
                    center_group,
                    window_bounds,
                    display,
                    docks,
                    centered_layout: self.centered_layout,
                    session_id: self.session_id.clone(),
                    bookmarks,
                    breakpoints,
                    window_id: Some(window_id.as_u64()),
                    user_toolchains,
                };

                let db = WorkspaceDb::global(cx);
                cx.background_spawn(async move {
                    if preserve_existing_docks
                        && let Some(existing) = db.workspace_for_id(database_id)
                    {
                        serialized_workspace.docks = existing.docks;
                    }
                    db.save_workspace(serialized_workspace).await;
                })
            }
            WorkspaceLocation::DetachFromSession | WorkspaceLocation::None => Task::ready(()),
        }
    }

    pub(crate) fn shutdown_serialization_snapshot(
        &self,
        window: &Window,
        cx: &App,
    ) -> ShutdownSerializationSnapshot {
        ShutdownSerializationSnapshot {
            window_bounds: SerializedWindowBounds(window.window_bounds()),
            display: window.display(cx).and_then(|display| display.uuid().ok()),
            docks: self.capture_dock_state(window, cx),
        }
    }

    pub fn root_paths(&self, cx: &App) -> Vec<Arc<Path>> {
        let project = self.project().read(cx);
        project
            .visible_worktrees(cx)
            .map(|worktree| worktree.read(cx).abs_path())
            .collect::<Vec<_>>()
    }

    fn remove_panes(&mut self, member: Member, window: &mut Window, cx: &mut Context<Workspace>) {
        match member {
            Member::Axis(PaneAxis { members, .. }) => {
                for child in members.iter() {
                    self.remove_panes(child.clone(), window, cx)
                }
            }
            Member::Pane(pane) => {
                self.force_remove_pane(&pane, &None, window, cx);
            }
        }
    }

    fn remove_from_session(&mut self, window: &mut Window, cx: &mut App) -> Task<()> {
        self.session_id.take();
        self.serialize_workspace_internal(window, cx)
    }

    fn force_remove_pane(
        &mut self,
        pane: &Entity<Pane>,
        focus_on: &Option<Entity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let removing_active_pane = self.active_pane() == pane;
        self.panes.retain(|p| p != pane);
        if let Some(focus_on) = focus_on {
            if removing_active_pane {
                self.set_active_pane(focus_on, window, cx);
            }
            focus_on.update(cx, |pane, cx| window.focus(&pane.focus_handle(cx), cx));
        } else if removing_active_pane {
            let fallback_pane = self.panes.last().unwrap().clone();
            self.set_active_pane(&fallback_pane, window, cx);
            if !self.has_active_modal(window, cx) {
                fallback_pane.update(cx, |pane, cx| window.focus(&pane.focus_handle(cx), cx));
            }
        }
        if self.last_active_center_pane == Some(pane.downgrade()) {
            self.last_active_center_pane = None;
        }
        cx.notify();
    }

    fn serialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.detached_restore {
            return;
        }
        if self._schedule_serialize_workspace.is_none() {
            self._schedule_serialize_workspace =
                Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor()
                        .timer(SERIALIZATION_THROTTLE_TIME)
                        .await;
                    this.update_in(cx, |this, window, cx| {
                        this._serialize_workspace_task =
                            Some(this.serialize_workspace_internal(window, cx));
                        this._schedule_serialize_workspace.take();
                    })
                    .log_err();
                }));
        }
    }

    fn build_serialized_workspace(
        &self,
        database_id: WorkspaceId,
        location: SerializedWorkspaceLocation,
        paths: PathList,
        window: &mut Window,
        cx: &mut App,
    ) -> SerializedWorkspace {
        fn serialize_pane_handle(
            pane_handle: &Entity<Pane>,
            window: &mut Window,
            cx: &mut App,
        ) -> SerializedPane {
            let (items, active, pinned_count) = {
                let pane = pane_handle.read(cx);
                let active_item_id = pane.active_item().map(|item| item.item_id());
                (
                    pane.items()
                        .filter_map(|handle| {
                            let handle = handle.to_serializable_item_handle(cx)?;

                            Some(SerializedItem {
                                kind: Arc::from(handle.serialized_item_kind()),
                                item_id: handle.item_id().as_u64(),
                                active: Some(handle.item_id()) == active_item_id,
                                preview: pane.is_active_preview_item(handle.item_id()),
                            })
                        })
                        .collect::<Vec<_>>(),
                    pane.has_focus(window, cx),
                    pane.pinned_count(),
                )
            };

            SerializedPane::new(items, active, pinned_count)
        }

        fn build_serialized_pane_group(
            pane_group: &Member,
            window: &mut Window,
            cx: &mut App,
        ) -> SerializedPaneGroup {
            match pane_group {
                Member::Axis(PaneAxis {
                    axis,
                    members,
                    flexes,
                    bounding_boxes: _,
                }) => SerializedPaneGroup::Group {
                    axis: SerializedAxis(*axis),
                    children: members
                        .iter()
                        .map(|member| build_serialized_pane_group(member, window, cx))
                        .collect::<Vec<_>>(),
                    flexes: Some(flexes.lock().clone()),
                },
                Member::Pane(pane_handle) => {
                    SerializedPaneGroup::Pane(serialize_pane_handle(pane_handle, window, cx))
                }
            }
        }

        let bookmarks = self.project.update(cx, |project, cx| {
            project
                .bookmark_store()
                .read(cx)
                .all_serialized_bookmarks(cx)
        });
        let breakpoints = self.project.update(cx, |project, cx| {
            project
                .breakpoint_store()
                .read(cx)
                .all_source_breakpoints(cx)
        });
        let user_toolchains = self
            .project
            .read(cx)
            .user_toolchains(cx)
            .unwrap_or_default();

        SerializedWorkspace {
            id: database_id,
            location,
            paths,
            identity_paths: Some(self.project_group_key(cx).path_list().clone()),
            center_group: build_serialized_pane_group(&self.center.root, window, cx),
            window_bounds: Some(SerializedWindowBounds(window.window_bounds())),
            display: window.display(cx).and_then(|display| display.uuid().ok()),
            docks: self.capture_dock_state(window, cx),
            centered_layout: self.centered_layout,
            session_id: self.session_id.clone(),
            bookmarks,
            breakpoints,
            window_id: Some(window.window_handle().window_id().as_u64()),
            user_toolchains,
        }
    }

    fn serialized_workspace_for_exact(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<SerializedWorkspace> {
        let database_id = self
            .database_id()
            .context("workspace has no durable persistence id")?;
        anyhow::ensure!(
            self.project.read(cx).is_local(),
            "exact workspace checkpoints only support local workspaces"
        );
        let paths = PathList::new(&self.root_paths(cx));
        Ok(self.build_serialized_workspace(
            database_id,
            SerializedWorkspaceLocation::Local,
            paths,
            window,
            cx,
        ))
    }

    fn serialize_workspace_internal(&self, _window: &mut Window, cx: &mut App) -> Task<()> {
        if self.detached_restore {
            return Task::ready(());
        }
        let (send_result, receive_result) = oneshot::channel();
        if let Err(error) = self
            .serializable_items_tx
            .unbounded_send(ItemSerializationCommand::SerializeWorkspace(send_result))
        {
            log::error!("failed to enqueue workspace serialization: {error}");
            return Task::ready(());
        }

        cx.background_spawn(async move {
            if receive_result.await.is_err() {
                log::debug!("workspace serializer dropped workspace serialization");
            }
        })
    }

    fn serialize_workspace_now(&self, window: &mut Window, cx: &mut App) -> Task<()> {
        let Some(database_id) = self.database_id() else {
            return Task::ready(());
        };

        match self.workspace_location(cx) {
            WorkspaceLocation::Location(location, paths) => {
                let serialized_workspace =
                    self.build_serialized_workspace(database_id, location, paths, window, cx);

                let open_components = self.capture_open_components(cx);
                let db = WorkspaceDb::global(cx);
                window.spawn(cx, async move |_| {
                    db.save_workspace(serialized_workspace).await;
                    db.save_open_components(database_id, open_components)
                        .await
                        .log_err();
                })
            }
            WorkspaceLocation::DetachFromSession => {
                let window_bounds = SerializedWindowBounds(window.window_bounds());
                let display = window.display(cx).and_then(|d| d.uuid().ok());
                // Save dock state for empty local workspaces
                let docks = self.capture_dock_state(window, cx);
                let db = WorkspaceDb::global(cx);
                let kvp = db::kvp::KeyValueStore::global(cx);
                window.spawn(cx, async move |_| {
                    db.set_window_open_status(
                        database_id,
                        window_bounds,
                        display.unwrap_or_default(),
                    )
                    .await
                    .log_err();
                    db.set_session_id(database_id, None).await.log_err();
                    persistence::write_default_dock_state(&kvp, docks)
                        .await
                        .log_err();
                })
            }
            WorkspaceLocation::None => {
                // Save dock state for empty non-local workspaces
                let docks = self.capture_dock_state(window, cx);
                let kvp = db::kvp::KeyValueStore::global(cx);
                window.spawn(cx, async move |_| {
                    persistence::write_default_dock_state(&kvp, docks)
                        .await
                        .log_err();
                })
            }
        }
    }

    fn has_any_items_open(&self, cx: &App) -> bool {
        self.panes.iter().any(|pane| pane.read(cx).items_len() > 0)
    }

    fn workspace_location(&self, cx: &App) -> WorkspaceLocation {
        let paths = PathList::new(&self.root_paths(cx));
        if let Some(connection) = self.project.read(cx).remote_connection_options(cx) {
            WorkspaceLocation::Location(SerializedWorkspaceLocation::Remote(connection), paths)
        } else if self.project.read(cx).is_local() {
            if !paths.is_empty() || self.has_any_items_open(cx) {
                WorkspaceLocation::Location(SerializedWorkspaceLocation::Local, paths)
            } else {
                WorkspaceLocation::DetachFromSession
            }
        } else {
            WorkspaceLocation::None
        }
    }

    fn update_history(&self, cx: &mut App) {
        if self.detached_restore {
            return;
        }
        let Some(id) = self.database_id() else {
            return;
        };
        if !self.project.read(cx).is_local() {
            return;
        }
        if let Some(manager) = HistoryManager::global(cx) {
            let paths = PathList::new(&self.root_paths(cx));
            manager.update(cx, |this, cx| {
                this.update_history(id, HistoryManagerEntry::new(id, &paths), cx);
            });
        }
    }

    async fn serialize_items(
        this: &WeakEntity<Self>,
        items_rx: UnboundedReceiver<ItemSerializationCommand>,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        const CHUNK_SIZE: usize = 200;

        let mut commands = items_rx.ready_chunks(CHUNK_SIZE);

        while let Some(commands_received) = commands.next().await {
            let mut pending_items = HashMap::default();

            for command in commands_received {
                match command {
                    ItemSerializationCommand::Serialize(item) => {
                        pending_items.entry(item.item_id()).or_insert(item);
                    }
                    ItemSerializationCommand::SerializeWorkspace(completion) => {
                        Self::serialize_item_batch(this, std::mem::take(&mut pending_items), cx)
                            .await?;
                        let task = this.update_in(cx, |workspace, window, cx| {
                            workspace.serialize_workspace_now(window, cx)
                        })?;
                        task.await;
                        if completion.send(()).is_err() {
                            log::debug!("workspace serialization caller was dropped");
                        }
                    }
                    ItemSerializationCommand::SaveWindowBounds(completion) => {
                        Self::serialize_item_batch(this, std::mem::take(&mut pending_items), cx)
                            .await?;
                        let task = this.update_in(cx, |workspace, window, cx| {
                            workspace.save_window_bounds_now(window, cx)
                        })?;
                        task.await;
                        if completion.send(()).is_err() {
                            log::debug!("workspace bounds serialization caller was dropped");
                        }
                    }
                    ItemSerializationCommand::CheckpointExact(completion) => {
                        Self::serialize_item_batch(this, std::mem::take(&mut pending_items), cx)
                            .await?;

                        let result = Self::serialize_exact_checkpoint(this, cx).await;
                        if completion.send(result).is_err() {
                            log::debug!("exact workspace checkpoint caller was dropped");
                        }
                    }
                }
            }

            Self::serialize_item_batch(this, pending_items, cx).await?;

            cx.background_executor()
                .timer(SERIALIZATION_THROTTLE_TIME)
                .await;
        }

        Ok(())
    }

    async fn serialize_item_batch(
        this: &WeakEntity<Self>,
        items: HashMap<EntityId, Box<dyn SerializableItemHandle>>,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        let tasks = this.update_in(cx, |workspace, window, cx| {
            items
                .into_values()
                .filter_map(|item| item.serialize(workspace, false, window, cx))
                .collect::<Vec<_>>()
        })?;

        let results = futures::future::join_all(tasks).await;
        for result in results {
            result.log_err();
        }
        Ok(())
    }

    async fn serialize_exact_checkpoint(
        this: &WeakEntity<Self>,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        let db = cx.update(|_window, cx| WorkspaceDb::global(cx))?;
        let transaction = db.begin_exact_checkpoint_transaction().await?;
        let checkpoint = async {
            let (tasks, serialized_workspace) =
                this.update_in(cx, |workspace, window, cx| {
                    let mut items = HashMap::default();
                    for pane in &workspace.panes {
                        for item in pane.read(cx).items() {
                            if let Some(item) = item.to_serializable_item_handle(cx) {
                                items.entry(item.item_id()).or_insert(item);
                            }
                        }
                    }

                    let tasks = items
                        .into_values()
                        .map(|item| {
                            item.checkpoint_in_transaction(
                                workspace,
                                transaction.clone(),
                                window,
                                cx,
                            )
                        })
                        .collect::<Vec<_>>();
                    let serialized_workspace =
                        workspace.serialized_workspace_for_exact(window, cx)?;
                    Ok::<_, anyhow::Error>((tasks, serialized_workspace))
                })??;

            let results = futures::future::join_all(tasks).await;
            let mut first_error = None;
            for result in results {
                if let Err(error) = result
                    && first_error.is_none()
                {
                    first_error = Some(error);
                }
            }
            if let Some(error) = first_error {
                return Err(error);
            }
            WorkspaceDb::save_workspace_in_transaction(&transaction, serialized_workspace).await
        }
        .await;

        if let Err(error) = checkpoint {
            if let Err(rollback_error) = transaction.rollback().await {
                return Err(error.context(format!(
                    "rolling back the exact workspace checkpoint also failed: {rollback_error:#}"
                )));
            }
            return Err(error);
        }
        transaction.commit().await
    }

    pub(crate) fn checkpoint_exact(&mut self, cx: &mut App) -> Task<Result<()>> {
        self._schedule_serialize_workspace.take();
        self._serialize_workspace_task.take();
        self.bounds_save_task_queued.take();

        let (send_result, receive_result) = oneshot::channel();
        if let Err(error) = self
            .serializable_items_tx
            .unbounded_send(ItemSerializationCommand::CheckpointExact(send_result))
        {
            return Task::ready(Err(anyhow!(
                "failed to enqueue exact workspace checkpoint: {error}"
            )));
        }

        cx.background_spawn(async move {
            receive_result
                .await
                .context("workspace serializer dropped the exact checkpoint")?
        })
    }

    pub(crate) fn checkpoint_exact_for_shutdown(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        self._schedule_serialize_workspace.take();
        self._serialize_workspace_task.take();
        self.bounds_save_task_queued.take();
        self._items_serializer = Task::ready(Ok(()));

        let db = WorkspaceDb::global(cx);
        let (transaction, transaction_ready) = db.queue_exact_checkpoint_transaction();
        let mut items = HashMap::default();
        for pane in &self.panes {
            for item in pane.read(cx).items() {
                if let Some(item) = item.to_serializable_item_handle(cx) {
                    items.entry(item.item_id()).or_insert(item);
                }
            }
        }
        let tasks = items
            .into_values()
            .map(|item| item.checkpoint_in_transaction(self, transaction.clone(), window, cx))
            .collect::<Vec<_>>();
        let serialized_workspace = match self.serialized_workspace_for_exact(window, cx) {
            Ok(serialized_workspace) => serialized_workspace,
            Err(error) => return Task::ready(Err(error)),
        };

        cx.background_spawn(async move {
            transaction.activate()?;
            transaction_ready.await?;
            let mut first_error = None;
            for result in futures::future::join_all(tasks).await {
                if let Err(error) = result
                    && first_error.is_none()
                {
                    first_error = Some(error);
                }
            }
            if let Some(error) = first_error {
                if let Err(rollback_error) = transaction.rollback().await {
                    return Err(error.context(format!(
                        "rolling back the exact workspace checkpoint also failed: {rollback_error:#}"
                    )));
                }
                return Err(error);
            }
            if let Err(error) =
                WorkspaceDb::save_workspace_in_transaction(&transaction, serialized_workspace).await
            {
                if let Err(rollback_error) = transaction.rollback().await {
                    return Err(error.context(format!(
                        "rolling back the exact workspace checkpoint also failed: {rollback_error:#}"
                    )));
                }
                return Err(error);
            }
            transaction.commit().await
        })
    }

    pub(crate) fn enqueue_item_serialization(
        &mut self,
        item: Box<dyn SerializableItemHandle>,
    ) -> Result<()> {
        self.serializable_items_tx
            .unbounded_send(ItemSerializationCommand::Serialize(item))
            .map_err(|err| anyhow!("failed to send serializable item over channel: {err}"))
    }

    pub(crate) fn load_workspace(
        serialized_workspace: SerializedWorkspace,
        paths_to_open: Vec<Option<ProjectPath>>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<Vec<Option<Box<dyn ItemHandle>>>>> {
        cx.spawn_in(window, async move |workspace, cx| {
            let project = workspace.read_with(cx, |workspace, _| workspace.project().clone())?;

            let mut center_group = None;
            let mut center_items = None;

            // Traverse the splits tree and add to things
            if let Some((group, active_pane, items)) = serialized_workspace
                .center_group
                .deserialize(&project, serialized_workspace.id, workspace.clone(), cx)
                .await
            {
                center_items = Some(items);
                center_group = Some((group, active_pane))
            }

            let mut items_by_project_path = HashMap::default();
            let mut item_ids_by_kind = HashMap::default();
            let mut all_deserialized_items = Vec::default();
            cx.update(|_, cx| {
                for item in center_items.unwrap_or_default().into_iter().flatten() {
                    if let Some(serializable_item_handle) = item.to_serializable_item_handle(cx) {
                        item_ids_by_kind
                            .entry(serializable_item_handle.serialized_item_kind())
                            .or_insert(Vec::new())
                            .push(item.item_id().as_u64() as ItemId);
                    }

                    if let Some(project_path) = item.project_path(cx) {
                        items_by_project_path.insert(project_path, item.clone());
                    }
                    all_deserialized_items.push(item);
                }
            })?;

            let opened_items = paths_to_open
                .into_iter()
                .map(|path_to_open| {
                    path_to_open
                        .and_then(|path_to_open| items_by_project_path.remove(&path_to_open))
                })
                .collect::<Vec<_>>();

            // Remove old panes from workspace panes list
            workspace.update_in(cx, |workspace, window, cx| {
                if let Some((center_group, active_pane)) = center_group {
                    workspace.remove_panes(workspace.center.root.clone(), window, cx);

                    // Swap workspace center group
                    workspace.center = PaneGroup::with_root(center_group);
                    workspace.center.set_is_center(true);
                    workspace.center.mark_positions(cx);

                    if let Some(active_pane) = active_pane {
                        workspace.set_active_pane(&active_pane, window, cx);
                        cx.focus_self(window);
                    } else {
                        workspace.set_active_pane(&workspace.center.first_pane(), window, cx);
                    }
                }

                let docks = serialized_workspace.docks;
                // Every dock receives the whole set; a dock only opens the keys
                // it actually registers, so it filters itself.
                let open_components = WorkspaceDb::global(cx)
                    .open_components(serialized_workspace.id)
                    .log_err()
                    .unwrap_or_default();

                for (dock, serialized_dock) in [
                    (&mut workspace.right_dock, docks.right),
                    (&mut workspace.left_dock, docks.left),
                    (&mut workspace.bottom_dock, docks.bottom),
                ]
                .iter_mut()
                {
                    dock.update(cx, |dock, cx| {
                        dock.set_serialized_state(
                            serialized_dock.clone(),
                            Some(open_components.clone()),
                        );
                        dock.restore_state(window, cx);
                    });
                }

                cx.notify();
            })?;

            project
                .update(cx, |project, cx| {
                    project.bookmark_store().update(cx, |bookmark_store, cx| {
                        bookmark_store.load_serialized_bookmarks(serialized_workspace.bookmarks, cx)
                    })
                })
                .await
                .log_err();

            let _ = project
                .update(cx, |project, cx| {
                    project
                        .breakpoint_store()
                        .update(cx, |breakpoint_store, cx| {
                            breakpoint_store
                                .with_serialized_breakpoints(serialized_workspace.breakpoints, cx)
                        })
                })
                .await;

            // Clean up all the items that have _not_ been loaded. Our ItemIds aren't stable. That means
            // after loading the items, we might have different items and in order to avoid
            // the database filling up, we delete items that haven't been loaded now.
            //
            // The items that have been loaded, have been saved after they've been added to the workspace.
            let clean_up_tasks = workspace.update_in(cx, |_, window, cx| {
                item_ids_by_kind
                    .into_iter()
                    .map(|(item_kind, loaded_items)| {
                        SerializableItemRegistry::cleanup(
                            item_kind,
                            serialized_workspace.id,
                            loaded_items,
                            window,
                            cx,
                        )
                        .log_err()
                    })
                    .collect::<Vec<_>>()
            })?;

            futures::future::join_all(clean_up_tasks).await;

            workspace
                .update_in(cx, |workspace, window, cx| {
                    // Serialize ourself to make sure our timestamps and any pane / item changes are replicated
                    workspace.serialize_workspace_internal(window, cx).detach();

                    // Ensure that we mark the window as edited if we did load dirty items
                    workspace.update_window_edited(window, cx);
                })
                .ok();

            Ok(opened_items)
        })
    }

    fn load_workspace_strict(
        serialized_workspace: SerializedWorkspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        cx.spawn_in(window, async move |workspace, cx| {
            let project = workspace.read_with(cx, |workspace, _| workspace.project().clone())?;
            let workspace_id = serialized_workspace.id;
            let (center_group, active_pane, _items) = serialized_workspace
                .center_group
                .deserialize_strict(&project, workspace_id, workspace.clone(), cx)
                .await?;

            workspace.update_in(cx, |workspace, window, cx| {
                workspace.center = PaneGroup::with_root(center_group);
                workspace.center.set_is_center(true);
                workspace.center.mark_positions(cx);
                workspace.panes = workspace.center.panes().into_iter().cloned().collect();
                workspace.active_pane =
                    active_pane.unwrap_or_else(|| workspace.center.first_pane());
                workspace.last_active_center_pane = Some(workspace.active_pane.downgrade());

                let docks = serialized_workspace.docks;
                for (dock, serialized_dock) in [
                    (&workspace.right_dock, docks.right),
                    (&workspace.left_dock, docks.left),
                    (&workspace.bottom_dock, docks.bottom),
                ] {
                    dock.update(cx, |dock, cx| {
                        dock.set_serialized_state(serialized_dock, None);
                        dock.restore_state(window, cx);
                    });
                }
                cx.notify();
            })?;

            project
                .update(cx, |project, cx| {
                    project.bookmark_store().update(cx, |bookmark_store, cx| {
                        bookmark_store.load_serialized_bookmarks(serialized_workspace.bookmarks, cx)
                    })
                })
                .await?;
            project
                .update(cx, |project, cx| {
                    project
                        .breakpoint_store()
                        .update(cx, |breakpoint_store, cx| {
                            breakpoint_store
                                .with_serialized_breakpoints(serialized_workspace.breakpoints, cx)
                        })
                })
                .await?;

            Ok(())
        })
    }

    pub fn key_context(&self, cx: &App) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add("Workspace");
        context.set("keyboard_layout", cx.keyboard_layout().name().to_string());
        if let Some(status) = self
            .debugger_provider
            .as_ref()
            .and_then(|provider| provider.active_thread_state(cx))
        {
            match status {
                ThreadStatus::Running | ThreadStatus::Stepping => {
                    context.add("debugger_running");
                }
                ThreadStatus::Stopped => context.add("debugger_stopped"),
                ThreadStatus::Exited | ThreadStatus::Ended => {}
            }
            // A coarse "there is a live debug session" flag (running, stepping,
            // or stopped at a breakpoint) used to gate debugger controls like
            // step/pause/stop. Distinct from `debugger_running`, which is only
            // true while the program is actually executing.
            if matches!(
                status,
                ThreadStatus::Running | ThreadStatus::Stepping | ThreadStatus::Stopped
            ) {
                context.add("debugger_session");
            }
        }

        if self.left_dock.read(cx).is_open() {
            if let Some(active_panel) = self.left_dock.read(cx).active_panel() {
                context.set("left_dock", active_panel.panel_key());
            }
        }

        if self.right_dock.read(cx).is_open() {
            if let Some(active_panel) = self.right_dock.read(cx).active_panel() {
                context.set("right_dock", active_panel.panel_key());
            }
        }

        if self.bottom_dock.read(cx).is_open() {
            if let Some(active_panel) = self.bottom_dock.read(cx).active_panel() {
                context.set("bottom_dock", active_panel.panel_key());
            }
        }

        context
    }

    /// Multiworkspace uses this to add workspace action handling to itself
    pub fn actions(&self, div: Div, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.add_workspace_actions_listeners(div, window, cx)
            .on_action(cx.listener(
                |_workspace, action_sequence: &settings::ActionSequence, window, cx| {
                    for action in &action_sequence.0 {
                        window.dispatch_action(action.boxed_clone(), cx);
                    }
                },
            ))
            .on_action(cx.listener(Self::close_inactive_items_and_panes))
            .on_action(cx.listener(Self::close_all_items_and_panes))
            .on_action(cx.listener(Self::close_item_in_all_panes))
            .on_action(cx.listener(Self::save_all))
            .on_action(cx.listener(Self::send_keystrokes))
            .on_action(cx.listener(Self::add_folder_to_project))
            .on_action(cx.listener(Self::follow_next_collaborator))
            .on_action(cx.listener(Self::activate_pane_at_index))
            .on_action(cx.listener(Self::move_item_to_pane_at_index))
            .on_action(cx.listener(Self::move_focused_panel_to_next_position))
            .on_action(cx.listener(Self::reopen_last_picker))
            .on_action(cx.listener(Self::toggle_edit_predictions_all_files))
            .on_action(cx.listener(Self::toggle_theme_mode))
            .on_action(cx.listener(|workspace, _: &Unfollow, window, cx| {
                let pane = workspace.active_pane().clone();
                workspace.unfollow_in_pane(&pane, window, cx);
            }))
            .on_action(cx.listener(|workspace, action: &Save, window, cx| {
                workspace
                    .save_active_item(action.save_intent.unwrap_or(SaveIntent::Save), window, cx)
                    .detach_and_prompt_err("Failed to save", window, cx, |_, _, _| None);
            }))
            .on_action(cx.listener(|workspace, _: &FormatAndSave, window, cx| {
                workspace
                    .save_active_item(SaveIntent::FormatAndSave, window, cx)
                    .detach_and_prompt_err("Failed to save", window, cx, |_, _, _| None);
            }))
            .on_action(cx.listener(|workspace, _: &SaveWithoutFormat, window, cx| {
                workspace
                    .save_active_item(SaveIntent::SaveWithoutFormat, window, cx)
                    .detach_and_prompt_err("Failed to save", window, cx, |_, _, _| None);
            }))
            .on_action(cx.listener(|workspace, _: &SaveAs, window, cx| {
                workspace
                    .save_active_item(SaveIntent::SaveAs, window, cx)
                    .detach_and_prompt_err("Failed to save", window, cx, |_, _, _| None);
            }))
            .on_action(
                cx.listener(|workspace, _: &ActivatePreviousPane, window, cx| {
                    workspace.activate_previous_pane(window, cx)
                }),
            )
            .on_action(cx.listener(|workspace, _: &ActivateNextPane, window, cx| {
                workspace.activate_next_pane(window, cx)
            }))
            .on_action(cx.listener(|workspace, _: &ActivateLastPane, window, cx| {
                workspace.activate_last_pane(window, cx)
            }))
            .on_action(cx.listener(|workspace, _: &FocusNextPart, window, cx| {
                workspace.move_part_focus(true, window, cx);
            }))
            .on_action(cx.listener(|workspace, _: &FocusPreviousPart, window, cx| {
                workspace.move_part_focus(false, window, cx);
            }))
            .on_action(
                cx.listener(|workspace, _: &ActivateNextWindow, _window, cx| {
                    workspace.activate_next_window(cx)
                }),
            )
            .on_action(
                cx.listener(|workspace, _: &ActivatePreviousWindow, _window, cx| {
                    workspace.activate_previous_window(cx)
                }),
            )
            .on_action(cx.listener(|workspace, _: &ActivatePaneLeft, window, cx| {
                workspace.activate_pane_in_direction(SplitDirection::Left, window, cx)
            }))
            .on_action(cx.listener(|workspace, _: &ActivatePaneRight, window, cx| {
                workspace.activate_pane_in_direction(SplitDirection::Right, window, cx)
            }))
            .on_action(cx.listener(|workspace, _: &ActivatePaneUp, window, cx| {
                workspace.activate_pane_in_direction(SplitDirection::Up, window, cx)
            }))
            .on_action(cx.listener(|workspace, _: &ActivatePaneDown, window, cx| {
                workspace.activate_pane_in_direction(SplitDirection::Down, window, cx)
            }))
            .on_action(cx.listener(
                |workspace, action: &MoveItemToPaneInDirection, window, cx| {
                    workspace.move_item_to_pane_in_direction(action, window, cx)
                },
            ))
            .on_action(cx.listener(|workspace, _: &SwapPaneLeft, _, cx| {
                workspace.swap_pane_in_direction(SplitDirection::Left, cx)
            }))
            .on_action(cx.listener(|workspace, _: &SwapPaneRight, _, cx| {
                workspace.swap_pane_in_direction(SplitDirection::Right, cx)
            }))
            .on_action(cx.listener(|workspace, _: &SwapPaneUp, _, cx| {
                workspace.swap_pane_in_direction(SplitDirection::Up, cx)
            }))
            .on_action(cx.listener(|workspace, _: &SwapPaneDown, _, cx| {
                workspace.swap_pane_in_direction(SplitDirection::Down, cx)
            }))
            .on_action(cx.listener(|workspace, _: &SwapPaneAdjacent, window, cx| {
                const DIRECTION_PRIORITY: [SplitDirection; 4] = [
                    SplitDirection::Down,
                    SplitDirection::Up,
                    SplitDirection::Right,
                    SplitDirection::Left,
                ];
                for dir in DIRECTION_PRIORITY {
                    if workspace.find_pane_in_direction(dir, cx).is_some() {
                        workspace.swap_pane_in_direction(dir, cx);
                        workspace.activate_pane_in_direction(dir.opposite(), window, cx);
                        break;
                    }
                }
            }))
            .on_action(cx.listener(|workspace, _: &MovePaneLeft, _, cx| {
                workspace.move_pane_to_border(SplitDirection::Left, cx)
            }))
            .on_action(cx.listener(|workspace, _: &MovePaneRight, _, cx| {
                workspace.move_pane_to_border(SplitDirection::Right, cx)
            }))
            .on_action(cx.listener(|workspace, _: &MovePaneUp, _, cx| {
                workspace.move_pane_to_border(SplitDirection::Up, cx)
            }))
            .on_action(cx.listener(|workspace, _: &MovePaneDown, _, cx| {
                workspace.move_pane_to_border(SplitDirection::Down, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleLeftDock, window, cx| {
                this.toggle_dock(DockPosition::Left, window, cx);
            }))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ToggleRightDock, window, cx| {
                    workspace.toggle_dock(DockPosition::Right, window, cx);
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ToggleBottomDock, window, cx| {
                    workspace.toggle_dock(DockPosition::Bottom, window, cx);
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &CloseActiveDock, window, cx| {
                    if !workspace.close_active_dock(window, cx) {
                        cx.propagate();
                    }
                },
            ))
            .on_action(
                cx.listener(|workspace: &mut Workspace, _: &CloseAllDocks, window, cx| {
                    workspace.close_all_docks(window, cx);
                }),
            )
            .on_action(cx.listener(Self::toggle_all_docks))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ClearAllNotifications, _, cx| {
                    workspace.clear_all_notifications(cx);
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ClearNavigationHistory, window, cx| {
                    workspace.clear_navigation_history(window, cx);
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &SuppressNotification, _, cx| {
                    if let Some((notification_id, _)) = workspace.notifications.pop() {
                        workspace.suppress_notification(&notification_id, cx);
                    }
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ToggleWorktreeSecurity, window, cx| {
                    workspace.show_worktree_trust_security_modal(true, window, cx);
                },
            ))
            .on_action(
                cx.listener(|_: &mut Workspace, _: &ClearTrustedWorktrees, _, cx| {
                    if let Some(trusted_worktrees) = TrustedWorktrees::try_get_global(cx) {
                        trusted_worktrees.update(cx, |trusted_worktrees, _| {
                            trusted_worktrees.clear_trusted_paths()
                        });
                        let db = WorkspaceDb::global(cx);
                        cx.spawn(async move |_, cx| {
                            if db.clear_trusted_worktrees().await.log_err().is_some() {
                                cx.update(|cx| reload(cx));
                            }
                        })
                        .detach();
                    }
                }),
            )
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ReopenClosedItem, window, cx| {
                    workspace.reopen_closed_item(window, cx).detach();
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ResetActiveDockSize, window, cx| {
                    for dock in workspace.all_docks() {
                        if dock.focus_handle(cx).contains_focused(window, cx) {
                            let panel = dock.read(cx).active_panel().cloned();
                            if let Some(panel) = panel {
                                dock.update(cx, |dock, cx| {
                                    dock.set_panel_size_state(
                                        panel.as_ref(),
                                        dock::PanelSizeState::default(),
                                        cx,
                                    );
                                });
                            }
                            return;
                        }
                    }
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ResetOpenDocksSize, _window, cx| {
                    workspace.reset_open_docks_size(cx);
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, _: &ResetPaneSizes, _window, cx| {
                    workspace.reset_pane_sizes(cx);
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, act: &IncreaseActiveDockSize, window, cx| {
                    adjust_active_dock_size_by_px(
                        px_with_ui_font_fallback(act.px, cx),
                        workspace,
                        window,
                        cx,
                    );
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, act: &DecreaseActiveDockSize, window, cx| {
                    adjust_active_dock_size_by_px(
                        px_with_ui_font_fallback(act.px, cx) * -1.,
                        workspace,
                        window,
                        cx,
                    );
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, act: &IncreaseOpenDocksSize, window, cx| {
                    adjust_open_docks_size_by_px(
                        px_with_ui_font_fallback(act.px, cx),
                        workspace,
                        window,
                        cx,
                    );
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, act: &DecreaseOpenDocksSize, window, cx| {
                    adjust_open_docks_size_by_px(
                        px_with_ui_font_fallback(act.px, cx) * -1.,
                        workspace,
                        window,
                        cx,
                    );
                },
            ))
            .on_action(cx.listener(Workspace::toggle_centered_layout))
            .on_action(cx.listener(Workspace::toggle_editor_zoom))
            .on_action(cx.listener(
                |workspace: &mut Workspace, action: &pane::ActivateNextItem, window, cx| {
                    if let Some(active_dock) = workspace.active_dock(window, cx) {
                        let dock = active_dock.read(cx);
                        if let Some(active_panel) = dock.active_panel() {
                            if active_panel.pane(cx).is_none() {
                                let mut recent_pane: Option<Entity<Pane>> = None;
                                let mut recent_timestamp = 0;
                                for pane_handle in workspace.panes() {
                                    let pane = pane_handle.read(cx);
                                    for entry in pane.activation_history() {
                                        if entry.timestamp > recent_timestamp {
                                            recent_timestamp = entry.timestamp;
                                            recent_pane = Some(pane_handle.clone());
                                        }
                                    }
                                }

                                if let Some(pane) = recent_pane {
                                    let wrap_around = action.wrap_around;
                                    pane.update(cx, |pane, cx| {
                                        let current_index = pane.active_item_index();
                                        let items_len = pane.items_len();
                                        if items_len > 0 {
                                            let next_index = if current_index + 1 < items_len {
                                                current_index + 1
                                            } else if wrap_around {
                                                0
                                            } else {
                                                return;
                                            };
                                            pane.activate_item(
                                                next_index, false, false, window, cx,
                                            );
                                        }
                                    });
                                    return;
                                }
                            }
                        }
                    }
                    cx.propagate();
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, action: &pane::ActivatePreviousItem, window, cx| {
                    if let Some(active_dock) = workspace.active_dock(window, cx) {
                        let dock = active_dock.read(cx);
                        if let Some(active_panel) = dock.active_panel() {
                            if active_panel.pane(cx).is_none() {
                                let mut recent_pane: Option<Entity<Pane>> = None;
                                let mut recent_timestamp = 0;
                                for pane_handle in workspace.panes() {
                                    let pane = pane_handle.read(cx);
                                    for entry in pane.activation_history() {
                                        if entry.timestamp > recent_timestamp {
                                            recent_timestamp = entry.timestamp;
                                            recent_pane = Some(pane_handle.clone());
                                        }
                                    }
                                }

                                if let Some(pane) = recent_pane {
                                    let wrap_around = action.wrap_around;
                                    pane.update(cx, |pane, cx| {
                                        let current_index = pane.active_item_index();
                                        let items_len = pane.items_len();
                                        if items_len > 0 {
                                            let prev_index = if current_index > 0 {
                                                current_index - 1
                                            } else if wrap_around {
                                                items_len.saturating_sub(1)
                                            } else {
                                                return;
                                            };
                                            pane.activate_item(
                                                prev_index, false, false, window, cx,
                                            );
                                        }
                                    });
                                    return;
                                }
                            }
                        }
                    }
                    cx.propagate();
                },
            ))
            .on_action(cx.listener(
                |workspace: &mut Workspace, action: &pane::CloseActiveItem, window, cx| {
                    if let Some(active_dock) = workspace.active_dock(window, cx) {
                        let dock = active_dock.read(cx);
                        if let Some(active_panel) = dock.active_panel() {
                            if active_panel.pane(cx).is_none() {
                                let active_pane = workspace.active_pane().clone();
                                active_pane.update(cx, |pane, cx| {
                                    pane.close_active_item(action, window, cx)
                                        .detach_and_log_err(cx);
                                });
                                return;
                            }
                        }
                    }
                    cx.propagate();
                },
            ))
            .on_action(
                cx.listener(|workspace, _: &ToggleReadOnlyFile, window, cx| {
                    let pane = workspace.active_pane().clone();
                    if let Some(item) = pane.read(cx).active_item() {
                        item.toggle_read_only(window, cx);
                    }
                }),
            )
            .on_action(cx.listener(|workspace, _: &FocusCenterPane, window, cx| {
                workspace.focus_center_pane(window, cx);
            }))
            .on_action(cx.listener(Workspace::clear_bookmarks))
            .on_action(cx.listener(Workspace::cancel))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_random_database_id(&mut self) {
        self.database_id = Some(WorkspaceId(Uuid::new_v4().as_u64_pair().0 as i64));
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_new(project: Entity<Project>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        use node_runtime::NodeRuntime;
        use session::Session;

        let client = project.read(cx).client();
        let user_store = project.read(cx).user_store();
        let workspace_store = cx.new(|cx| WorkspaceStore::new(client.clone(), cx));
        let session = cx.new(|cx| AppSession::new(Session::test(), cx));
        window.activate_window();
        let app_state = Arc::new(AppState {
            languages: project.read(cx).languages().clone(),
            workspace_store,
            client,
            user_store,
            fs: project.read(cx).fs().clone(),
            build_window_options: |_, _| Default::default(),
            node_runtime: NodeRuntime::unavailable(),
            session,
        });
        let workspace = Self::new(Default::default(), project, app_state, window, cx);
        workspace
            .active_pane
            .update(cx, |pane, cx| window.focus(&pane.focus_handle(cx), cx));
        workspace
    }

    pub fn register_action<A: Action>(
        &mut self,
        callback: impl Fn(&mut Self, &A, &mut Window, &mut Context<Self>) + 'static,
    ) -> &mut Self {
        let callback = Arc::new(callback);

        self.workspace_actions.push(Box::new(move |div, _, _, cx| {
            let callback = callback.clone();
            div.on_action(cx.listener(move |workspace, event, window, cx| {
                (callback)(workspace, event, window, cx)
            }))
        }));
        self
    }
    pub fn register_action_renderer(
        &mut self,
        callback: impl Fn(Div, &Workspace, &mut Window, &mut Context<Self>) -> Div + 'static,
    ) -> &mut Self {
        self.workspace_actions.push(Box::new(callback));
        self
    }

    fn add_workspace_actions_listeners(
        &self,
        mut div: Div,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        for action in self.workspace_actions.iter() {
            div = (action)(div, self, window, cx)
        }
        div
    }

    pub fn has_active_modal(&self, _: &mut Window, cx: &mut App) -> bool {
        self.modal_layer.read(cx).has_active_modal()
    }

    pub fn active_modal<V: ManagedView + 'static>(&self, cx: &App) -> Option<Entity<V>> {
        self.modal_layer.read(cx).active_modal()
    }

    /// Toggles a modal of type `V`. If a modal of the same type is currently active,
    /// it will be hidden. If a different modal is active, it will be replaced with the new one.
    /// If no modal is active, the new modal will be shown.
    ///
    /// If closing the current modal fails (e.g., due to `on_before_dismiss` returning
    /// `DismissDecision::Dismiss(false)` or `DismissDecision::Pending`), the new modal
    /// will not be shown.
    pub fn toggle_modal<V: ModalView, B>(&mut self, window: &mut Window, cx: &mut App, build: B)
    where
        B: FnOnce(&mut Window, &mut Context<V>) -> V,
    {
        self.modal_layer.update(cx, |modal_layer, cx| {
            modal_layer.toggle_modal(window, cx, build)
        })
    }

    pub fn hide_modal(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.modal_layer
            .update(cx, |modal_layer, cx| modal_layer.hide_modal(window, cx))
    }

    fn reopen_last_picker(
        &mut self,
        _: &ReopenLastPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // When triggered from within another modal (e.g. the command palette), that
        // modal's dismissal is asynchronous, so defer the reveal until it has closed;
        // otherwise a modal would still be active and the reveal would be a no-op.
        cx.defer_in(window, |workspace, window, cx| {
            workspace.modal_layer.update(cx, |modal_layer, cx| {
                modal_layer.reveal_stashed_modal(window, cx);
            });
        });
    }

    pub fn toggle_status_toast<V: ToastView>(&mut self, entity: Entity<V>, cx: &mut App) {
        self.toast_layer
            .update(cx, |toast_layer, cx| toast_layer.toggle_toast(cx, entity))
    }

    pub fn toggle_centered_layout(
        &mut self,
        _: &ToggleCenteredLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.centered_layout = !self.centered_layout;
        self.serialize_workspace(window, cx);
        cx.notify();
    }

    pub fn clear_bookmarks(&mut self, _: &ClearBookmarks, _: &mut Window, cx: &mut Context<Self>) {
        self.project()
            .read(cx)
            .bookmark_store()
            .update(cx, |bookmark_store, cx| {
                bookmark_store.clear_bookmarks(cx);
            });
    }

    pub fn toggle_editor_zoom(
        &mut self,
        _: &ToggleEditorZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zoomed.is_some() {
            self.active_pane.update(cx, |pane, cx| {
                pane.set_zoomed(false, cx);
            });
            self.zoomed = None;
            self.zoomed_position = None;
            cx.emit(Event::ZoomChanged);
        }

        if let Some(maximized) = self.maximized_pane.take() {
            if maximized.upgrade().as_ref() == Some(&self.active_pane) {
                cx.notify();
                return;
            }
        }

        self.maximized_pane = Some(self.active_pane.downgrade());
        window.focus(&self.active_pane.focus_handle(cx), cx);
        cx.notify();
    }

    pub fn is_pane_maximized(&self) -> bool {
        self.maximized_pane.is_some()
    }

    fn adjust_padding(padding: Option<f32>) -> f32 {
        padding
            .unwrap_or(CenteredPaddingSettings::default().0)
            .clamp(
                CenteredPaddingSettings::MIN_PADDING,
                CenteredPaddingSettings::MAX_PADDING,
            )
    }

    fn render_dock(
        &self,
        position: DockPosition,
        dock: &Entity<Dock>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Stateful<Div>> {
        if self.zoomed_position == Some(position) {
            return None;
        }

        let leader_border = dock.read(cx).active_panel().and_then(|panel| {
            let pane = panel.pane(cx)?;
            let follower_states = &self.follower_states;
            leader_border_for_pane(follower_states, &pane, window, cx)
        });

        // Expose each open dock as a landmark region so assistive technology
        // can navigate to it, and so region navigation announces it. While a
        // screen reader is active the wrapper is also the focus target for
        // region navigation (it carries the landmark role and label), so
        // focusing it announces the region instead of falling back to the whole
        // window. We only make it focusable in that case so it never adds a
        // hitbox or intercepts mouse focus for other users.
        let (dock_element_id, dock_label) = match position {
            DockPosition::Left => ("left-dock", "Left dock"),
            DockPosition::Right => ("right-dock", "Right dock"),
            DockPosition::Bottom => ("bottom-dock", "Bottom dock"),
        };
        let dock_is_open = dock.read(cx).is_open();
        let a11y_active = window.is_a11y_active();

        let mut container = div()
            .id(dock_element_id)
            .when(dock_is_open, |this| {
                this.role(gpui::Role::Complementary)
                    .aria_label(dock_label)
                    .when(a11y_active, |this| {
                        this.track_focus(self.region_focus_handles.dock(position))
                    })
            })
            .flex()
            .overflow_hidden()
            .flex_none()
            .child(dock.clone())
            .children(leader_border);

        // Apply sizing only when the dock is open. When closed the dock is still
        // included in the element tree so its focus handle remains mounted — without
        // this, toggle_panel_focus cannot focus the panel when the dock is closed.
        let dock = dock.read(cx);
        if dock.is_open() {
            if position.axis() == Axis::Horizontal {
                let fixed = dock.open_fixed_size(window, cx);
                let flex_total = dock.open_flex(self.default_dock_flex(position), window, cx);
                if let Some(total) = flex_total {
                    let grow = (total / self.center_full_height_column_count()).max(0.001);
                    let style = container.style();
                    style.flex_grow = Some(grow);
                    style.flex_shrink = Some(1.0);
                    style.flex_basis = Some(if fixed > Pixels::ZERO {
                        fixed.into()
                    } else {
                        relative(0.).into()
                    });
                } else {
                    container = container.w(fixed);
                    // Allow the fixed-width dock to shrink when there isn't
                    // enough space (e.g. when the sidebar is open). The
                    // stored sizes are preserved so the dock expands back
                    // when space becomes available.
                    let style = container.style();
                    style.flex_shrink = Some(1.0);
                }
            } else if let Some(size) = dock.open_row_size(window, cx) {
                container = container.h(size);
            }
        }

        Some(container)
    }

    /// Returns the currently-visible major window regions ("parts"), in a stable
    /// cyclic order: title bar, left dock, editor, right dock, bottom dock,
    /// status bar. Closed docks are skipped. Used by
    /// [`FocusNextPart`]/[`FocusPreviousPart`] so keyboard and screen-reader
    /// users can move between regions without a mouse.
    fn focusable_parts(&self, cx: &App) -> Vec<FocusablePart> {
        // The interactive focus target inside an open dock: the active panel, or
        // the dock itself as a fallback. This is what region navigation focuses
        // when no screen reader is active (preserving the previous behavior).
        fn dock_content_handle(dock: &Entity<Dock>, cx: &App) -> FocusHandle {
            let dock = dock.read(cx);
            dock.active_panel()
                .map(|panel| panel.activation_focus_handle(cx))
                .unwrap_or_else(|| dock.focus_handle(cx))
        }

        let dock_part = |dock: &Entity<Dock>, wrapper: &FocusHandle| {
            dock.read(cx)
                .is_open()
                .then(|| FocusablePart::landmark(wrapper.clone(), dock_content_handle(dock, cx)))
        };

        let mut parts = Vec::new();
        if self.titlebar_item.is_some() {
            // The title bar is an ARIA toolbar, so region navigation lands on
            // its first control rather than the toolbar container.
            parts.push(FocusablePart::toolbar(self.titlebar_focus_handle.clone()));
        }
        parts.extend(dock_part(
            &self.left_dock,
            &self.region_focus_handles.left_dock,
        ));

        let center_pane = self
            .last_active_center_pane
            .as_ref()
            .and_then(|pane| pane.upgrade())
            .unwrap_or_else(|| self.center.first_pane());
        parts.push(FocusablePart::landmark(
            self.region_focus_handles.editor.clone(),
            center_pane.read(cx).focus_handle(cx),
        ));

        parts.extend(dock_part(
            &self.right_dock,
            &self.region_focus_handles.right_dock,
        ));
        parts.extend(dock_part(
            &self.bottom_dock,
            &self.region_focus_handles.bottom_dock,
        ));
        // The status bar is an ARIA toolbar, so region navigation lands on its
        // first control rather than the toolbar container.
        parts.push(FocusablePart::toolbar(
            self.status_bar.read(cx).focus_handle(cx),
        ));
        parts
    }

    /// Moves focus to the next (or previous) visible window region. See
    /// [`FocusNextPart`].
    fn move_part_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let parts = self.focusable_parts(cx);
        if parts.is_empty() {
            return;
        }
        let current = parts
            .iter()
            .position(|part| part.contains_focused(window, cx));
        let next_index = match current {
            Some(index) if forward => (index + 1) % parts.len(),
            Some(index) => (index + parts.len() - 1) % parts.len(),
            None => 0,
        };
        let part = &parts[next_index];
        match &part.behavior {
            PartBehavior::Toolbar => {
                // The ARIA toolbar pattern requires focus to rest on the first
                // control, not the container. Focusing the tab-group container
                // and advancing descends into its first control; if the toolbar
                // has no focusable control, restore focus to the container so
                // navigation doesn't escape into an unrelated region.
                let container = part.container.clone();
                window.focus(&container, cx);
                window.focus_next(cx);
                let landed_inside = window
                    .focused(cx)
                    .is_some_and(|handle| container.contains(&handle, window));
                if !landed_inside {
                    window.focus(&container, cx);
                }
            }
            PartBehavior::Landmark { content } => {
                // Only redirect to the (otherwise non-focusable) wrapper when a
                // screen reader is active, so it is announced as a landmark.
                // Without a screen reader, focus the interactive content so
                // sighted keyboard users land somewhere usable, unchanged from
                // before this feature existed.
                if window.is_a11y_active() {
                    window.focus(&part.container, cx);
                } else {
                    window.focus(content, cx);
                }
            }
        }
        cx.notify();
    }

    /// Moves focus between the interactive controls within the title bar
    /// toolbar in response to arrow keys. Navigation is clamped to the title
    /// bar so arrows move between items and stop at the ends (ARIA toolbar
    /// semantics); Tab is still used to leave the toolbar.
    fn move_titlebar_item_focus(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = window.focused(cx);
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
        let landed_in_titlebar = window
            .focused(cx)
            .is_some_and(|handle| self.titlebar_focus_handle.contains(&handle, window));
        // If Tab navigation wandered out of the toolbar, restore the previous
        // item so the ends of the toolbar act as stops rather than exits.
        if !landed_in_titlebar && let Some(previous) = previous {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    /// Renders the center pane group wrapped in a `Main` landmark so assistive
    /// technology recognizes the editor as the main region and can navigate to
    /// it. While a screen reader is active the wrapper is also the focus target
    /// for region navigation (it carries the landmark role and label), so
    /// focusing it announces "Editor" instead of falling back to the whole
    /// window. We only make it focusable in that case so it never adds a hitbox
    /// or intercepts mouse focus for other users.
    fn render_center(
        &self,
        render_cx: &PaneRenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        div()
            .id("editor-region")
            .debug_selector(|| "EDITOR-REGION".to_string())
            .role(gpui::Role::Main)
            .aria_label("Editor")
            .when(window.is_a11y_active(), |this| {
                this.track_focus(&self.region_focus_handles.editor)
            })
            .size_full()
            .child(self.center.render(
                self.zoomed.as_ref(),
                self.maximized_pane.as_ref(),
                render_cx,
                window,
                cx,
            ))
    }

    pub fn for_window(window: &Window, cx: &App) -> Option<Entity<Workspace>> {
        window
            .root::<MultiWorkspace>()
            .flatten()
            .map(|multi_workspace| multi_workspace.read(cx).workspace().clone())
    }

    pub fn zoomed_item(&self) -> Option<&AnyWeakView> {
        self.zoomed.as_ref()
    }

    pub fn activate_next_window(&mut self, cx: &mut Context<Self>) {
        let Some(current_window_id) = cx.active_window().map(|a| a.window_id()) else {
            return;
        };
        let windows = cx.windows();
        let next_window =
            SystemWindowTabController::get_next_tab_group_window(cx, current_window_id).or_else(
                || {
                    windows
                        .iter()
                        .cycle()
                        .skip_while(|window| window.window_id() != current_window_id)
                        .nth(1)
                },
            );

        if let Some(window) = next_window {
            window
                .update(cx, |_, window, _| window.activate_window())
                .ok();
        }
    }

    pub fn activate_previous_window(&mut self, cx: &mut Context<Self>) {
        let Some(current_window_id) = cx.active_window().map(|a| a.window_id()) else {
            return;
        };
        let windows = cx.windows();
        let prev_window =
            SystemWindowTabController::get_prev_tab_group_window(cx, current_window_id).or_else(
                || {
                    windows
                        .iter()
                        .rev()
                        .cycle()
                        .skip_while(|window| window.window_id() != current_window_id)
                        .nth(1)
                },
            );

        if let Some(window) = prev_window {
            window
                .update(cx, |_, window, _| window.activate_window())
                .ok();
        }
    }

    pub fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if cx.stop_active_drag(window) {
        } else if let Some((notification_id, _)) = self.notifications.pop() {
            dismiss_app_notification(&notification_id, cx);
        } else {
            cx.propagate();
        }
    }

    fn resize_dock(
        &mut self,
        dock_pos: DockPosition,
        new_size: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match dock_pos {
            DockPosition::Left => self.resize_left_dock(new_size, window, cx),
            DockPosition::Right => self.resize_right_dock(new_size, window, cx),
            DockPosition::Bottom => self.resize_bottom_dock(new_size, window, cx),
        }
    }

    fn resize_left_dock(&mut self, new_size: Pixels, window: &mut Window, cx: &mut App) {
        let workspace_width = self.bounds.size.width;
        // Floored so every open column still fits, and ceilinged so the editor
        // keeps `CENTER_MIN_WIDTH`. Both ends matter and for the same reason: a
        // drag must never store a width the viewport cannot hold. Three narrower
        // floors were dogfooded and each left a column off-window (Decision
        // 2026-09-05); the ceiling was then found by a cold read to have the same
        // hole in the other direction — the editor minimum is a layout `min_w`, so
        // without a term here a wide drag stored widths that shrank the container
        // while the columns held, and the side scrolled.
        //
        // `Dock::render` stays unfloored on purpose, so `clamp_panel_size` can
        // still overflow on window shrink — the one route to a scroll that
        // remains, and the one per-area scrolling was built for.
        let floor = self
            .left_dock
            .read_with(cx, |left_dock, cx| left_dock.open_min_size(window, cx))
            .unwrap_or(Pixels::ZERO);
        let opposite = self.right_dock.read_with(cx, |right_dock, cx| {
            right_dock
                .stored_active_panel_size(window, cx)
                .unwrap_or(Pixels::ZERO)
        });
        let ceiling = (workspace_width - crate::layout_registry::CENTER_MIN_WIDTH - opposite)
            .max(Pixels::ZERO);
        // Ceiling last: on a window too narrow to hold the floor it wins, which
        // collapses the side rather than pushing the editor off-screen.
        let size = new_size.max(floor).min(ceiling);

        let flex_grow = self.dock_flex_for_size(DockPosition::Left, size, window, cx);
        // `resize_all_panels_in_dock` is accepted and ignored: every open panel is
        // visible now, so the editor-facing divider always resizes the whole side.
        self.left_dock.update(cx, |left_dock, cx| {
            left_dock.resize_active_panel(Some(size), flex_grow, window, cx);
        });
    }

    fn resize_right_dock(&mut self, new_size: Pixels, window: &mut Window, cx: &mut App) {
        let workspace_width = self.bounds.size.width;
        let floor = self
            .right_dock
            .read_with(cx, |right_dock, cx| right_dock.open_min_size(window, cx))
            .unwrap_or(Pixels::ZERO);
        let opposite = self.left_dock.read_with(cx, |left_dock, cx| {
            left_dock
                .stored_active_panel_size(window, cx)
                .unwrap_or(Pixels::ZERO)
        });
        let ceiling = (workspace_width - crate::layout_registry::CENTER_MIN_WIDTH - opposite)
            .max(Pixels::ZERO);
        let size = new_size.max(floor).min(ceiling);
        let flex_grow = self.dock_flex_for_size(DockPosition::Right, size, window, cx);
        self.right_dock.update(cx, |right_dock, cx| {
            right_dock.resize_active_panel(Some(size), flex_grow, window, cx);
        });
    }

    /// Put the seam under the pointer, the same way the editor-facing divider does
    /// — `resize_left_dock` is handed `position.x - bounds.left()` and nothing is
    /// remembered between frames.
    ///
    /// An earlier version tracked the drag's origin instead, and it jumped: the
    /// `Point` `on_drag` hands its constructor is `event.position - hitbox.origin`,
    /// the cursor's offset **inside the 6px handle**, not a window coordinate. Held
    /// as an origin against absolute move positions, every frame asked for a width
    /// several hundred pixels off and pinned to the clamp. Deriving the seam from
    /// geometry each frame cannot drift and needs no state to get wrong.
    /// Where the bottom row starts and how wide it is — the two numbers a bottom
    /// seam needs and `dock_size` cannot give, because a vertical dock's size is
    /// its height.
    ///
    /// The row's horizontal extent is whatever its `bottom_dock_layout` leaves it,
    /// so this reproduces the four arms of `Workspace::render`: `Full` spans the
    /// workspace, `Contained` sits between both sides, and the aligned variants
    /// keep one side full-height and span the other.
    fn bottom_row_extent(&self, window: &Window, cx: &App) -> Option<(Pixels, Pixels)> {
        let left = self
            .left_dock
            .read(cx)
            .is_open()
            .then(|| self.dock_size(self.left_dock.read(cx), window, cx))
            .flatten()
            .unwrap_or(Pixels::ZERO);
        let right = self
            .right_dock
            .read(cx)
            .is_open()
            .then(|| self.dock_size(self.right_dock.read(cx), window, cx))
            .flatten()
            .unwrap_or(Pixels::ZERO);
        let (leading, trailing) = match WorkspaceSettings::get_global(cx).bottom_dock_layout {
            BottomDockLayout::Full => (Pixels::ZERO, Pixels::ZERO),
            BottomDockLayout::Contained => (left, right),
            BottomDockLayout::LeftAligned => (Pixels::ZERO, right),
            BottomDockLayout::RightAligned => (left, Pixels::ZERO),
        };
        let width = self.bounds.size.width - leading - trailing;
        (width > Pixels::ZERO).then(|| (self.bounds.left() + leading, width))
    }

    fn resize_dock_seam(
        &mut self,
        position: DockPosition,
        index: usize,
        x: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index == 0 {
            return;
        }
        let dock = self.dock_at_position(position).clone();
        if position == DockPosition::Bottom {
            let Some((leading_edge, row_total)) = self.bottom_row_extent(window, cx) else {
                return;
            };
            let widths = dock.read_with(cx, |dock, cx| dock.rendered_widths(row_total, window, cx));
            if index >= widths.len() {
                return;
            }
            let preceding = widths[..index - 1]
                .iter()
                .fold(Pixels::ZERO, |sum, width| sum + *width);
            let target = x - leading_edge - preceding;
            dock.update(cx, |dock, cx| {
                dock.resize_row_seam(index, target, row_total, window, cx);
            });
            self.serialize_workspace(window, cx);
            return;
        }
        let Some(dock_total) = dock.read_with(cx, |dock, cx| self.dock_size(dock, window, cx))
        else {
            return;
        };
        let widths = dock.read_with(cx, |dock, cx| dock.rendered_widths(dock_total, window, cx));
        if index >= widths.len() {
            return;
        }
        let preceding = widths[..index - 1]
            .iter()
            .fold(Pixels::ZERO, |sum, width| sum + *width);
        // The side is flush against the workspace edge it docks to, which is the
        // same assumption `resize_left_dock` and `resize_right_dock` already make.
        let dock_leading_edge = match position {
            DockPosition::Left => self.bounds.left(),
            DockPosition::Right => self.bounds.right() - dock_total,
            // Handled above: a bottom seam divides the row's width, which its
            // `bottom_dock_layout` geometry decides, not the dock's own size.
            DockPosition::Bottom => return,
        };
        let target = x - dock_leading_edge - preceding;

        let Some(geometry) = self.dock_geometry(position, window, cx) else {
            return;
        };
        dock.update(cx, |dock, cx| {
            dock.resize_seam(index, target, dock_total, &geometry, window, cx);
        });
        self.serialize_workspace(window, cx);
    }

    fn resize_bottom_dock(&mut self, new_size: Pixels, window: &mut Window, cx: &mut App) {
        // Floored so the dock stays usable, and ceilinged so the editor keeps
        // `CENTER_MIN_HEIGHT` — the same pair of ends the side drags carry, for the
        // same reason. Until now the ceiling was the whole workspace less the
        // handle, so a drag to the top stored a height that left the editor with
        // nothing: the width half of this was found and fixed on 2026-09-05 and the
        // vertical half went with it untouched.
        let ceiling = (self.bounds.size.height
            - RESIZE_HANDLE_SIZE
            - crate::layout_registry::CENTER_MIN_HEIGHT)
            .max(Pixels::ZERO);
        // Ceiling last, the same order the side drags use, so a window too short to
        // hold the floor yields a dock that fits inside it rather than one that
        // overflows to satisfy a minimum.
        let size = new_size
            .max(crate::layout_registry::BOTTOM_DOCK_MIN_HEIGHT)
            .min(ceiling);
        self.bottom_dock.update(cx, |bottom_dock, cx| {
            bottom_dock.resize_active_panel(Some(size), None, window, cx);
        });
    }

    fn toggle_edit_predictions_all_files(
        &mut self,
        _: &ToggleEditPrediction,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fs = self.project().read(cx).fs().clone();
        let show_edit_predictions = all_language_settings(None, cx).show_edit_predictions(None, cx);
        update_settings_file(fs, cx, move |file, _| {
            file.project.all_languages.defaults.show_edit_predictions = Some(!show_edit_predictions)
        });
    }

    fn toggle_theme_mode(&mut self, _: &ToggleMode, _window: &mut Window, cx: &mut Context<Self>) {
        let current_mode = ThemeSettings::get_global(cx).theme.mode();
        let next_mode = match current_mode {
            Some(theme_settings::ThemeAppearanceMode::Light) => {
                theme_settings::ThemeAppearanceMode::Dark
            }
            Some(theme_settings::ThemeAppearanceMode::Dark) => {
                theme_settings::ThemeAppearanceMode::Light
            }
            Some(theme_settings::ThemeAppearanceMode::System) | None => {
                match cx.theme().appearance() {
                    theme::Appearance::Light => theme_settings::ThemeAppearanceMode::Dark,
                    theme::Appearance::Dark => theme_settings::ThemeAppearanceMode::Light,
                }
            }
        };

        let fs = self.project().read(cx).fs().clone();
        settings::update_settings_file(fs, cx, move |settings, _cx| {
            theme_settings::set_mode(settings, next_mode);
        });
    }

    pub fn show_worktree_trust_security_modal(
        &mut self,
        toggle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(security_modal) = self.active_modal::<SecurityModal>(cx) {
            if toggle {
                security_modal.update(cx, |security_modal, cx| {
                    security_modal.dismiss(cx);
                })
            } else {
                security_modal.update(cx, |security_modal, cx| {
                    security_modal.refresh_restricted_paths(cx);
                });
            }
        } else {
            let has_restricted_worktrees = TrustedWorktrees::has_restricted_worktrees(
                &self.project().read(cx).worktree_store(),
                cx,
            );
            if has_restricted_worktrees {
                let project = self.project().read(cx);
                let remote_host = project
                    .remote_connection_options(cx)
                    .map(RemoteHostLocation::from);
                let worktree_store = project.worktree_store().downgrade();
                self.toggle_modal(window, cx, |window, cx| {
                    SecurityModal::new(worktree_store, remote_host, window, cx)
                });
            }
        }
    }
}

pub trait AnyActiveCall {
    fn entity(&self) -> AnyEntity;
    fn is_in_room(&self, _: &App) -> bool;
    fn room_id(&self, _: &App) -> Option<u64>;
    fn channel_id(&self, _: &App) -> Option<ChannelId>;
    fn hang_up(&self, _: &mut App) -> Task<Result<()>>;
    fn unshare_project(&self, _: Entity<Project>, _: &mut App) -> Result<()>;
    fn remote_participant_for_peer_id(&self, _: PeerId, _: &App) -> Option<RemoteCollaborator>;
    fn is_sharing_project(&self, _: &App) -> bool;
    fn is_sharing_screen(&self, _: &App) -> bool;
    fn has_remote_participants(&self, _: &App) -> bool;
    fn local_participant_is_guest(&self, _: &App) -> bool;
    fn client(&self, _: &App) -> Arc<Client>;
    fn share_on_join(&self, _: &App) -> bool;
    fn join_channel(&self, _: ChannelId, _: &mut App) -> Task<Result<bool>>;
    fn room_update_completed(&self, _: &mut App) -> Task<()>;
    fn most_active_project(&self, _: &App) -> Option<(u64, u64)>;
    fn share_project(&self, _: Entity<Project>, _: &mut App) -> Task<Result<u64>>;
    fn join_project(
        &self,
        _: u64,
        _: Arc<LanguageRegistry>,
        _: Arc<dyn Fs>,
        _: &mut App,
    ) -> Task<Result<Entity<Project>>>;
    fn peer_id_for_user_in_room(&self, _: u64, _: &App) -> Option<PeerId>;
    fn subscribe(
        &self,
        _: &mut Window,
        _: &mut Context<Workspace>,
        _: Box<dyn Fn(&mut Workspace, &ActiveCallEvent, &mut Window, &mut Context<Workspace>)>,
    ) -> Subscription;
    fn create_shared_screen(
        &self,
        _: PeerId,
        _: &Entity<Pane>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Entity<SharedScreen>>;
    fn peer_ids_with_video_tracks(&self, _: &App) -> Vec<PeerId>;
}

#[derive(Clone)]
pub struct GlobalAnyActiveCall(pub Arc<dyn AnyActiveCall>);
impl Global for GlobalAnyActiveCall {}

impl GlobalAnyActiveCall {
    pub(crate) fn try_global(cx: &App) -> Option<&Self> {
        cx.try_global()
    }

    pub(crate) fn global(cx: &App) -> &Self {
        cx.global()
    }
}

/// Workspace-local view of a remote participant's location.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticipantLocation {
    SharedProject { project_id: u64 },
    UnsharedProject,
    External,
}

impl ParticipantLocation {
    pub fn from_proto(location: Option<proto::ParticipantLocation>) -> Result<Self> {
        match location
            .and_then(|l| l.variant)
            .context("participant location was not provided")?
        {
            proto::participant_location::Variant::SharedProject(project) => {
                Ok(Self::SharedProject {
                    project_id: project.id,
                })
            }
            proto::participant_location::Variant::UnsharedProject(_) => Ok(Self::UnsharedProject),
            proto::participant_location::Variant::External(_) => Ok(Self::External),
        }
    }
}
/// Workspace-local view of a remote collaborator's state.
/// This is the subset of `call::RemoteParticipant` that workspace needs.
#[derive(Clone)]
pub struct RemoteCollaborator {
    pub user: Arc<User>,
    pub peer_id: PeerId,
    pub location: ParticipantLocation,
    pub participant_index: ParticipantIndex,
}

pub enum ActiveCallEvent {
    ParticipantLocationChanged { participant_id: PeerId },
    RemoteVideoTracksChanged { participant_id: PeerId },
    LocalScreenShareStarted,
    LocalScreenShareStopped,
    RoomLeft,
}

fn leader_border_for_pane(
    follower_states: &HashMap<CollaboratorId, FollowerState>,
    pane: &Entity<Pane>,
    _: &Window,
    cx: &App,
) -> Option<Div> {
    let (leader_id, _follower_state) = follower_states.iter().find_map(|(leader_id, state)| {
        if state.pane() == pane {
            Some((*leader_id, state))
        } else {
            None
        }
    })?;

    let mut leader_color = match leader_id {
        CollaboratorId::PeerId(leader_peer_id) => {
            let leader = GlobalAnyActiveCall::try_global(cx)?
                .0
                .remote_participant_for_peer_id(leader_peer_id, cx)?;

            cx.theme()
                .players()
                .color_for_participant(leader.participant_index.0)
                .cursor
        }
        CollaboratorId::Agent => cx.theme().players().agent().cursor,
    };
    leader_color.fade_out(0.3);
    Some(
        div()
            .absolute()
            .size_full()
            .left_0()
            .top_0()
            .border_2()
            .border_color(leader_color),
    )
}

fn window_bounds_env_override() -> Option<Bounds<Pixels>> {
    ZED_WINDOW_POSITION
        .zip(*ZED_WINDOW_SIZE)
        .map(|(position, size)| Bounds {
            origin: position,
            size,
        })
}

fn open_items(
    serialized_workspace: Option<SerializedWorkspace>,
    mut project_paths_to_open: Vec<(PathBuf, Option<ProjectPath>)>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl 'static + Future<Output = Result<Vec<Option<Result<Box<dyn ItemHandle>>>>>> + use<> {
    let restored_items = serialized_workspace.map(|serialized_workspace| {
        Workspace::load_workspace(
            serialized_workspace,
            project_paths_to_open
                .iter()
                .map(|(_, project_path)| project_path)
                .cloned()
                .collect(),
            window,
            cx,
        )
    });

    cx.spawn_in(window, async move |workspace, cx| {
        let mut opened_items = Vec::with_capacity(project_paths_to_open.len());

        if let Some(restored_items) = restored_items {
            let restored_items = restored_items.await?;

            let restored_project_paths = restored_items
                .iter()
                .filter_map(|item| {
                    cx.update(|_, cx| item.as_ref()?.project_path(cx))
                        .ok()
                        .flatten()
                })
                .collect::<HashSet<_>>();

            for restored_item in restored_items {
                opened_items.push(restored_item.map(Ok));
            }

            project_paths_to_open
                .iter_mut()
                .for_each(|(_, project_path)| {
                    if let Some(project_path_to_open) = project_path
                        && restored_project_paths.contains(project_path_to_open)
                    {
                        *project_path = None;
                    }
                });
        } else {
            for _ in 0..project_paths_to_open.len() {
                opened_items.push(None);
            }
        }
        assert!(opened_items.len() == project_paths_to_open.len());

        let tasks =
            project_paths_to_open
                .into_iter()
                .enumerate()
                .map(|(ix, (abs_path, project_path))| {
                    let workspace = workspace.clone();
                    cx.spawn(async move |cx| {
                        let file_project_path = project_path?;
                        let abs_path_task = workspace.update(cx, |workspace, cx| {
                            workspace.project().update(cx, |project, cx| {
                                project.resolve_abs_path(abs_path.to_string_lossy().as_ref(), cx)
                            })
                        });

                        // We only want to open file paths here. If one of the items
                        // here is a directory, it was already opened further above
                        // with a `find_or_create_worktree`.
                        if let Ok(task) = abs_path_task
                            && task.await.is_none_or(|p| p.is_file())
                        {
                            return Some((
                                ix,
                                workspace
                                    .update_in(cx, |workspace, window, cx| {
                                        workspace.open_path(
                                            file_project_path,
                                            None,
                                            true,
                                            window,
                                            cx,
                                        )
                                    })
                                    .log_err()?
                                    .await,
                            ));
                        }
                        None
                    })
                });

        let tasks = tasks.collect::<Vec<_>>();

        let tasks = futures::future::join_all(tasks);
        for (ix, path_open_result) in tasks.await.into_iter().flatten() {
            opened_items[ix] = Some(path_open_result);
        }

        Ok(opened_items)
    })
}

#[derive(Clone)]
enum ActivateInDirectionTarget {
    Pane(Entity<Pane>),
    Dock(Entity<Dock>),
    Sidebar(FocusHandle),
}

/// A focusable major window region used by region navigation
/// ([`FocusNextPart`]/[`FocusPreviousPart`]).
struct FocusablePart {
    /// The region's container/ancestor handle. For landmark regions this is the
    /// wrapper that carries the landmark role + label (and is only focusable
    /// while a screen reader is active); for toolbars it is the toolbar
    /// container.
    container: FocusHandle,
    behavior: PartBehavior,
}

enum PartBehavior {
    /// An ARIA toolbar (title bar, status bar). Region navigation focuses the
    /// container and descends to its first control, which is usable for
    /// everyone, so this is not gated on assistive technology.
    Toolbar,
    /// A landmark region (a dock or the editor). The wrapper carries the
    /// landmark role + label, so when a screen reader is active we focus it so
    /// it is announced. Otherwise we focus the interactive `content` so sighted
    /// keyboard users land on something usable, exactly as before - and the
    /// wrapper is left non-focusable so it never affects mouse focus.
    Landmark { content: FocusHandle },
}

impl FocusablePart {
    fn toolbar(container: FocusHandle) -> Self {
        Self {
            container,
            behavior: PartBehavior::Toolbar,
        }
    }

    fn landmark(wrapper: FocusHandle, content: FocusHandle) -> Self {
        Self {
            container: wrapper,
            behavior: PartBehavior::Landmark { content },
        }
    }

    /// Whether focus currently lies within this region. Checks both the wrapper
    /// (focused/ancestor when a screen reader is active) and, for landmarks, the
    /// interactive content (focused when a screen reader is not active, since
    /// the wrapper isn't in the focus tree then).
    fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        if self.container.contains_focused(window, cx) {
            return true;
        }
        match &self.behavior {
            PartBehavior::Landmark { content } => content.contains_focused(window, cx),
            PartBehavior::Toolbar => false,
        }
    }
}

/// Focus handles for the landmark region wrappers (the docks and the editor).
/// Region navigation ([`FocusNextPart`]/[`FocusPreviousPart`]) focuses these so
/// it lands on the wrapper that actually carries the landmark role and label
/// (e.g. `Main`/"Editor", `Complementary`/"Left dock"). Without them, region
/// navigation would focus an inner element that has no accessibility node, and
/// assistive technology would fall back to announcing the whole window.
struct RegionFocusHandles {
    left_dock: FocusHandle,
    right_dock: FocusHandle,
    bottom_dock: FocusHandle,
    editor: FocusHandle,
}

impl RegionFocusHandles {
    fn new(cx: &mut App) -> Self {
        Self {
            left_dock: cx.focus_handle(),
            right_dock: cx.focus_handle(),
            bottom_dock: cx.focus_handle(),
            editor: cx.focus_handle(),
        }
    }

    fn dock(&self, position: DockPosition) -> &FocusHandle {
        match position {
            DockPosition::Left => &self.left_dock,
            DockPosition::Right => &self.right_dock,
            DockPosition::Bottom => &self.bottom_dock,
        }
    }
}

fn notify_if_database_failed(window: WindowHandle<MultiWorkspace>, cx: &mut AsyncApp) {
    window
        .update(cx, |multi_workspace, _, cx| {
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, |workspace, cx| {
                if (*db::ALL_FILE_DB_FAILED).load(std::sync::atomic::Ordering::Acquire) {
                    struct DatabaseFailedNotification;

                    workspace.show_notification(
                        NotificationId::unique::<DatabaseFailedNotification>(),
                        cx,
                        |cx| {
                            cx.new(|cx| {
                                MessageNotification::new("Failed to load the database file.", cx)
                                    .primary_message("File an Issue")
                                    .primary_icon(IconName::Plus)
                                    .primary_on_click(|window, cx| {
                                        window.dispatch_action(Box::new(FileBugReport), cx)
                                    })
                            })
                        },
                    );
                }
            });
        })
        .log_err();
}

fn px_with_ui_font_fallback(val: u32, cx: &Context<Workspace>) -> Pixels {
    if val == 0 {
        ThemeSettings::get_global(cx).ui_font_size(cx)
    } else {
        px(val as f32)
    }
}

fn adjust_active_dock_size_by_px(
    px: Pixels,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(active_dock) = workspace
        .all_docks()
        .into_iter()
        .find(|dock| dock.focus_handle(cx).contains_focused(window, cx))
    else {
        return;
    };
    let dock = active_dock.read(cx);
    let Some(panel_size) = workspace.dock_size(&dock, window, cx) else {
        return;
    };
    workspace.resize_dock(dock.position(), panel_size + px, window, cx);
}

fn adjust_open_docks_size_by_px(
    px: Pixels,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let docks = workspace
        .all_docks()
        .into_iter()
        .filter_map(|dock_entity| {
            let dock = dock_entity.read(cx);
            if dock.is_open() {
                let dock_pos = dock.position();
                let panel_size = workspace.dock_size(&dock, window, cx)?;
                Some((dock_pos, panel_size + px))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    for (position, new_size) in docks {
        workspace.resize_dock(position, new_size, window, cx);
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.active_pane.focus_handle(cx)
    }
}

#[derive(Clone)]
struct DraggedDock(DockPosition);

impl Render for DraggedDock {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// A drag on the seam between two open columns. Deliberately a separate payload
/// from [`DraggedDock`]: that one sets the side's total width and this one only
/// redistributes inside it, so a handler cannot serve the wrong one by accident.
#[derive(Clone)]
struct DraggedDockSeam {
    position: DockPosition,
    /// The seam sits on this column's leading edge, in drawn order, so it is
    /// always between columns `index - 1` and `index` and never zero.
    index: usize,
}

impl Render for DraggedDockSeam {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        static FIRST_PAINT: AtomicBool = AtomicBool::new(true);
        if FIRST_PAINT.swap(false, std::sync::atomic::Ordering::Relaxed) {
            log::info!("Rendered first frame");
        }

        let centered_layout = self.centered_layout
            && self.center.panes().len() == 1
            && self.active_item(cx).is_some();
        let render_padding = |size| {
            (size > 0.0).then(|| {
                div()
                    .h_full()
                    .w(relative(size))
                    .bg(cx.theme().colors().editor_background)
                    .border_color(cx.theme().colors().pane_group_border)
            })
        };
        let paddings = if centered_layout {
            let settings = WorkspaceSettings::get_global(cx).centered_layout;
            (
                render_padding(Self::adjust_padding(
                    settings.left_padding.map(|padding| padding.0),
                )),
                render_padding(Self::adjust_padding(
                    settings.right_padding.map(|padding| padding.0),
                )),
            )
        } else {
            (None, None)
        };
        let ui_font = theme_settings::setup_ui_font(window, cx);

        let theme = cx.theme().clone();
        let colors = theme.colors();
        let notification_entities = self
            .notifications
            .iter()
            .map(|(_, notification)| notification.entity_id())
            .collect::<Vec<_>>();
        let bottom_dock_layout = WorkspaceSettings::get_global(cx).bottom_dock_layout;

        let pane_render_context = PaneRenderContext {
            follower_states: &self.follower_states,
            active_call: self.active_call(),
            active_pane: &self.active_pane,
            app_state: &self.app_state,
            project: &self.project,
            workspace: &self.weak_self,
        };

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font(ui_font)
            .gap_0()
            .justify_start()
            .items_start()
            .text_color(colors.text)
            .overflow_hidden()
            // Expose the title bar as an ARIA toolbar so region navigation
            // (FocusNextPart) can reach the top bar's controls and assistive
            // technology announces it as a toolbar. The contained controls form
            // a tab group: region navigation lands on the first control (per
            // the ARIA toolbar pattern), Tab steps through them, and arrow keys
            // move between them once focus is inside.
            .when_some(self.titlebar_item.clone(), |this, item| {
                this.child(
                    div()
                        .id("titlebar-region")
                        .track_focus(&self.titlebar_focus_handle)
                        .tab_group()
                        .role(gpui::Role::Toolbar)
                        .aria_label("Title bar")
                        .on_key_down(cx.listener(
                            |workspace, event: &gpui::KeyDownEvent, window, cx| {
                                if event.keystroke.modifiers.modified() {
                                    return;
                                }
                                match event.keystroke.key.as_str() {
                                    "right" => {
                                        workspace.move_titlebar_item_focus(true, window, cx);
                                        cx.stop_propagation();
                                    }
                                    "left" => {
                                        workspace.move_titlebar_item_focus(false, window, cx);
                                        cx.stop_propagation();
                                    }
                                    _ => {}
                                }
                            },
                        ))
                        .w_full()
                        .child(item),
                )
            })
            .on_modifiers_changed(move |_, _, cx| {
                for &id in &notification_entities {
                    cx.notify(id);
                }
            })
            .child(
                div()
                    .size_full()
                    .relative()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("workspace")
                            .bg(colors.background)
                            .relative()
                            .flex_1()
                            .w_full()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .border_t_1()
                            .border_b_1()
                            .border_color(colors.border)
                            .child({
                                let this = cx.entity();
                                canvas(
                                    move |bounds, window, cx| {
                                        this.update(cx, |this, cx| {
                                            let bounds_changed = this.bounds != bounds;
                                            this.bounds = bounds;

                                            if bounds_changed {
                                                this.left_dock.update(cx, |dock, cx| {
                                                    dock.clamp_panel_size(
                                                        bounds.size.width,
                                                        window,
                                                        cx,
                                                    )
                                                });

                                                this.right_dock.update(cx, |dock, cx| {
                                                    dock.clamp_panel_size(
                                                        bounds.size.width,
                                                        window,
                                                        cx,
                                                    )
                                                });

                                                this.bottom_dock.update(cx, |dock, cx| {
                                                    dock.clamp_panel_size(
                                                        bounds.size.height,
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }
                                        })
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full()
                            })
                            .when(self.zoomed.is_none(), |this| {
                                this.on_drag_move(cx.listener(
                                    move |workspace, e: &DragMoveEvent<DraggedDock>, window, cx| {
                                        if workspace.previous_dock_drag_coordinates
                                            != Some(e.event.position)
                                        {
                                            workspace.previous_dock_drag_coordinates =
                                                Some(e.event.position);

                                            match e.drag(cx).0 {
                                                DockPosition::Left => {
                                                    workspace.resize_left_dock(
                                                        e.event.position.x
                                                            - workspace.bounds.left(),
                                                        window,
                                                        cx,
                                                    );
                                                }
                                                DockPosition::Right => {
                                                    workspace.resize_right_dock(
                                                        workspace.bounds.right()
                                                            - e.event.position.x,
                                                        window,
                                                        cx,
                                                    );
                                                }
                                                DockPosition::Bottom => {
                                                    workspace.resize_bottom_dock(
                                                        workspace.bounds.bottom()
                                                            - e.event.position.y,
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            };
                                            workspace.serialize_workspace(window, cx);
                                        }
                                    },
                                ))
                                .on_drag_move(cx.listener(
                                    move |workspace,
                                          e: &DragMoveEvent<DraggedDockSeam>,
                                          window,
                                          cx| {
                                        let seam = e.drag(cx).clone();
                                        workspace.resize_dock_seam(
                                            seam.position,
                                            seam.index,
                                            e.event.position.x,
                                            window,
                                            cx,
                                        );
                                    },
                                ))
                            })
                            .child({
                                match bottom_dock_layout {
                                    BottomDockLayout::Full => div()
                                        .flex()
                                        .flex_col()
                                        .h_full()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .flex_1()
                                                .overflow_hidden()
                                                .children(self.render_dock(
                                                    DockPosition::Left,
                                                    &self.left_dock,
                                                    window,
                                                    cx,
                                                ))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .flex_1()
                                                        .min_w(crate::layout_registry::CENTER_MIN_WIDTH)
                                                        .overflow_hidden()
                                                        .child(
                                                            h_flex()
                                                                .flex_1()
                                                                .min_h(crate::layout_registry::CENTER_MIN_HEIGHT)
                                                                .when_some(paddings.0, |this, p| {
                                                                    this.child(p.border_r_1())
                                                                })
                                                                .child(self.render_center(
                                                                    &pane_render_context,
                                                                    window,
                                                                    cx,
                                                                ))
                                                                .when_some(
                                                                    paddings.1,
                                                                    |this, p| {
                                                                        this.child(p.border_l_1())
                                                                    },
                                                                ),
                                                        ),
                                                )
                                                .children(self.render_dock(
                                                    DockPosition::Right,
                                                    &self.right_dock,
                                                    window,
                                                    cx,
                                                )),
                                        )
                                        .child(div().w_full().children(self.render_dock(
                                            DockPosition::Bottom,
                                            &self.bottom_dock,
                                            window,
                                            cx,
                                        ))),

                                    BottomDockLayout::LeftAligned => div()
                                        .flex()
                                        .flex_row()
                                        .h_full()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .flex_1()
                                                .h_full()
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_row()
                                                        .flex_1()
                                                        .children(self.render_dock(
                                                            DockPosition::Left,
                                                            &self.left_dock,
                                                            window,
                                                            cx,
                                                        ))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_col()
                                                                .flex_1()
                                                                .min_w(crate::layout_registry::CENTER_MIN_WIDTH)
                                                                .overflow_hidden()
                                                                .child(
                                                                    h_flex()
                                                                        .flex_1()
                                                                        .min_h(crate::layout_registry::CENTER_MIN_HEIGHT)
                                                                        .when_some(
                                                                            paddings.0,
                                                                            |this, p| {
                                                                                this.child(
                                                                                    p.border_r_1(),
                                                                                )
                                                                            },
                                                                        )
                                                                        .child(self.render_center(
                                                                            &pane_render_context,
                                                                            window,
                                                                            cx,
                                                                        ))
                                                                        .when_some(
                                                                            paddings.1,
                                                                            |this, p| {
                                                                                this.child(
                                                                                    p.border_l_1(),
                                                                                )
                                                                            },
                                                                        ),
                                                                ),
                                                        ),
                                                )
                                                .child(div().w_full().children(self.render_dock(
                                                    DockPosition::Bottom,
                                                    &self.bottom_dock,
                                                    window,
                                                    cx,
                                                ))),
                                        )
                                        .children(self.render_dock(
                                            DockPosition::Right,
                                            &self.right_dock,
                                            window,
                                            cx,
                                        )),
                                    BottomDockLayout::RightAligned => div()
                                        .flex()
                                        .flex_row()
                                        .h_full()
                                        .children(self.render_dock(
                                            DockPosition::Left,
                                            &self.left_dock,
                                            window,
                                            cx,
                                        ))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .flex_1()
                                                .h_full()
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_row()
                                                        .flex_1()
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_col()
                                                                .flex_1()
                                                                .min_w(crate::layout_registry::CENTER_MIN_WIDTH)
                                                                .overflow_hidden()
                                                                .child(
                                                                    h_flex()
                                                                        .flex_1()
                                                                        .min_h(crate::layout_registry::CENTER_MIN_HEIGHT)
                                                                        .when_some(
                                                                            paddings.0,
                                                                            |this, p| {
                                                                                this.child(
                                                                                    p.border_r_1(),
                                                                                )
                                                                            },
                                                                        )
                                                                        .child(self.render_center(
                                                                            &pane_render_context,
                                                                            window,
                                                                            cx,
                                                                        ))
                                                                        .when_some(
                                                                            paddings.1,
                                                                            |this, p| {
                                                                                this.child(
                                                                                    p.border_l_1(),
                                                                                )
                                                                            },
                                                                        ),
                                                                ),
                                                        )
                                                        .children(self.render_dock(
                                                            DockPosition::Right,
                                                            &self.right_dock,
                                                            window,
                                                            cx,
                                                        )),
                                                )
                                                .child(div().w_full().children(self.render_dock(
                                                    DockPosition::Bottom,
                                                    &self.bottom_dock,
                                                    window,
                                                    cx,
                                                ))),
                                        ),
                                    BottomDockLayout::Contained => div()
                                        .flex()
                                        .flex_row()
                                        .h_full()
                                        .children(self.render_dock(
                                            DockPosition::Left,
                                            &self.left_dock,
                                            window,
                                            cx,
                                        ))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .flex_1()
                                                .min_w(crate::layout_registry::CENTER_MIN_WIDTH)
                                                .overflow_hidden()
                                                .child(
                                                    h_flex()
                                                        .flex_1()
                                                        .min_h(crate::layout_registry::CENTER_MIN_HEIGHT)
                                                        .when_some(paddings.0, |this, p| {
                                                            this.child(p.border_r_1())
                                                        })
                                                        .child(self.render_center(
                                                            &pane_render_context,
                                                            window,
                                                            cx,
                                                        ))
                                                        .when_some(paddings.1, |this, p| {
                                                            this.child(p.border_l_1())
                                                        }),
                                                )
                                                .children(self.render_dock(
                                                    DockPosition::Bottom,
                                                    &self.bottom_dock,
                                                    window,
                                                    cx,
                                                )),
                                        )
                                        .children(self.render_dock(
                                            DockPosition::Right,
                                            &self.right_dock,
                                            window,
                                            cx,
                                        )),
                                }
                            })
                            .children(self.zoomed.as_ref().and_then(|view| {
                                let zoomed_view = view.upgrade()?;
                                let div = div()
                                    .occlude()
                                    .absolute()
                                    .overflow_hidden()
                                    .border_color(colors.border)
                                    .bg(colors.background)
                                    .child(zoomed_view)
                                    .inset_0()
                                    .shadow_lg();

                                if !WorkspaceSettings::get_global(cx).zoomed_padding {
                                    return Some(div);
                                }

                                Some(match self.zoomed_position {
                                    Some(DockPosition::Left) => div.right_2().border_r_1(),
                                    Some(DockPosition::Right) => div.left_2().border_l_1(),
                                    Some(DockPosition::Bottom) => div.top_2().border_t_1(),
                                    None => div.top_2().bottom_2().left_2().right_2().border_1(),
                                })
                            }))
                            .children(self.render_notifications(window, cx)),
                    )
                    .when(self.status_bar_visible(cx), |parent| {
                        parent.child(self.status_bar.clone())
                    })
                    .child(self.toast_layer.clone()),
            )
    }
}

impl WorkspaceStore {
    pub fn new(client: Arc<Client>, cx: &mut Context<Self>) -> Self {
        Self {
            workspaces: Default::default(),
            _subscriptions: vec![
                client.add_request_handler(cx.weak_entity(), Self::handle_follow),
                client.add_message_handler(cx.weak_entity(), Self::handle_update_followers),
            ],
            client,
        }
    }

    pub fn update_followers(
        &self,
        project_id: Option<u64>,
        update: proto::update_followers::Variant,
        cx: &App,
    ) -> Option<()> {
        let active_call = GlobalAnyActiveCall::try_global(cx)?;
        let room_id = active_call.0.room_id(cx)?;
        self.client
            .send(proto::UpdateFollowers {
                room_id,
                project_id,
                variant: Some(update),
            })
            .log_err()
    }

    pub async fn handle_follow(
        this: Entity<Self>,
        envelope: TypedEnvelope<proto::Follow>,
        mut cx: AsyncApp,
    ) -> Result<proto::FollowResponse> {
        this.update(&mut cx, |this, cx| {
            let follower = Follower {
                project_id: envelope.payload.project_id,
                peer_id: envelope.original_sender_id()?,
            };

            let mut response = proto::FollowResponse::default();

            this.workspaces.retain(|(window_handle, weak_workspace)| {
                let Some(workspace) = weak_workspace.upgrade() else {
                    return false;
                };
                window_handle
                    .update(cx, |_, window, cx| {
                        workspace.update(cx, |workspace, cx| {
                            let handler_response =
                                workspace.handle_follow(follower.project_id, window, cx);
                            if let Some(active_view) = handler_response.active_view
                                && workspace.project.read(cx).remote_id() == follower.project_id
                            {
                                response.active_view = Some(active_view)
                            }
                        });
                    })
                    .is_ok()
            });

            Ok(response)
        })
    }

    async fn handle_update_followers(
        this: Entity<Self>,
        envelope: TypedEnvelope<proto::UpdateFollowers>,
        mut cx: AsyncApp,
    ) -> Result<()> {
        let leader_id = envelope.original_sender_id()?;
        let update = envelope.payload;

        this.update(&mut cx, |this, cx| {
            this.workspaces.retain(|(window_handle, weak_workspace)| {
                let Some(workspace) = weak_workspace.upgrade() else {
                    return false;
                };
                window_handle
                    .update(cx, |_, window, cx| {
                        workspace.update(cx, |workspace, cx| {
                            let project_id = workspace.project.read(cx).remote_id();
                            if update.project_id != project_id && update.project_id.is_some() {
                                return;
                            }
                            workspace.handle_update_followers(
                                leader_id,
                                update.clone(),
                                window,
                                cx,
                            );
                        });
                    })
                    .is_ok()
            });
            Ok(())
        })
    }

    pub fn workspaces(&self) -> impl Iterator<Item = &WeakEntity<Workspace>> {
        self.workspaces.iter().map(|(_, weak)| weak)
    }

    pub fn workspaces_with_windows(
        &self,
    ) -> impl Iterator<Item = (gpui::AnyWindowHandle, &WeakEntity<Workspace>)> {
        self.workspaces.iter().map(|(window, weak)| (*window, weak))
    }
}

impl ViewId {
    pub(crate) fn from_proto(message: proto::ViewId) -> Result<Self> {
        Ok(Self {
            creator: message
                .creator
                .map(CollaboratorId::PeerId)
                .context("creator is missing")?,
            id: message.id,
        })
    }

    pub(crate) fn to_proto(self) -> Option<proto::ViewId> {
        if let CollaboratorId::PeerId(peer_id) = self.creator {
            Some(proto::ViewId {
                creator: Some(peer_id),
                id: self.id,
            })
        } else {
            None
        }
    }
}

impl FollowerState {
    fn pane(&self) -> &Entity<Pane> {
        self.dock_pane.as_ref().unwrap_or(&self.center_pane)
    }
}

pub trait WorkspaceHandle {
    fn file_project_paths(&self, cx: &App) -> Vec<ProjectPath>;
}

impl WorkspaceHandle for Entity<Workspace> {
    fn file_project_paths(&self, cx: &App) -> Vec<ProjectPath> {
        self.read(cx)
            .worktrees(cx)
            .flat_map(|worktree| {
                let worktree_id = worktree.read(cx).id();
                worktree.read(cx).files(true, 0).map(move |f| ProjectPath {
                    worktree_id,
                    path: f.path.clone(),
                })
            })
            .collect::<Vec<_>>()
    }
}

pub async fn last_opened_workspace_location(
    db: &WorkspaceDb,
    fs: &dyn fs::Fs,
) -> Option<(WorkspaceId, SerializedWorkspaceLocation, PathList)> {
    db.last_workspace(fs)
        .await
        .log_err()
        .flatten()
        .map(|workspace| (workspace.workspace_id, workspace.location, workspace.paths))
}

pub async fn last_session_workspace_locations(
    db: &WorkspaceDb,
    last_session_id: &str,
    last_session_window_stack: Option<Vec<WindowId>>,
    fs: &dyn fs::Fs,
) -> Option<Vec<SessionWorkspace>> {
    db.last_session_workspace_locations(last_session_id, last_session_window_stack, fs)
        .await
        .log_err()
}

pub async fn restore_multiworkspace(
    multi_workspace: SerializedMultiWorkspace,
    app_state: Arc<AppState>,
    cx: &mut AsyncApp,
) -> anyhow::Result<WindowHandle<MultiWorkspace>> {
    let SerializedMultiWorkspace {
        active_workspace,
        workspaces,
        state,
    } = multi_workspace;

    let workspace_result = if active_workspace.paths.is_empty() {
        cx.update(|cx| {
            open_workspace_by_id(active_workspace.workspace_id, app_state.clone(), None, cx)
        })
        .await
    } else {
        cx.update(|cx| {
            Workspace::new_local(
                active_workspace.paths.paths().to_vec(),
                app_state.clone(),
                None,
                None,
                None,
                OpenMode::Activate,
                cx,
            )
        })
        .await
        .map(|result| result.window)
    };

    let window_handle = match workspace_result {
        Ok(handle) => handle,
        Err(err) => {
            log::error!("Failed to restore active workspace: {err:#}");

            let mut fallback_handle = None;
            for key in &state.project_groups {
                let key: ProjectGroupKey = key.clone().into();
                let paths = key.path_list().paths().to_vec();
                match cx
                    .update(|cx| {
                        Workspace::new_local(
                            paths,
                            app_state.clone(),
                            None,
                            None,
                            None,
                            OpenMode::Activate,
                            cx,
                        )
                    })
                    .await
                {
                    Ok(OpenResult { window, .. }) => {
                        fallback_handle = Some(window);
                        break;
                    }
                    Err(fallback_err) => {
                        log::error!("Fallback project group also failed: {fallback_err:#}");
                    }
                }
            }

            fallback_handle.ok_or(err)?
        }
    };

    restore_inactive_multiworkspace_members(
        window_handle,
        &active_workspace,
        workspaces,
        app_state.clone(),
        cx,
    )
    .await;

    apply_restored_multiworkspace_state(window_handle, &state, app_state.fs.clone(), cx).await;

    window_handle
        .update(cx, |_, window, _cx| {
            window.activate_window();
        })
        .ok();

    Ok(window_handle)
}

async fn restore_inactive_multiworkspace_members(
    window_handle: WindowHandle<MultiWorkspace>,
    active_workspace: &SessionWorkspace,
    workspaces: Vec<SessionWorkspace>,
    app_state: Arc<AppState>,
    cx: &mut AsyncApp,
) {
    let restore_workspace_membership =
        cx.update(|cx| MultiWorkspace::retention_enabled_from_settings(cx));
    if !restore_workspace_membership {
        return;
    }

    for workspace in workspaces {
        if workspace.workspace_id == active_workspace.workspace_id || workspace.paths.is_empty() {
            continue;
        }

        match workspace.location {
            SerializedWorkspaceLocation::Local => {
                if let Err(err) = cx
                    .update(|cx| {
                        Workspace::new_local(
                            workspace.paths.paths().to_vec(),
                            app_state.clone(),
                            Some(window_handle),
                            None,
                            None,
                            OpenMode::Add,
                            cx,
                        )
                    })
                    .await
                {
                    log::error!(
                        "Failed to restore inactive workspace {:?}: {err:#}",
                        workspace.paths
                    );
                }
            }
            SerializedWorkspaceLocation::Remote(_) => {
                log::warn!(
                    "Skipping inactive remote workspace restore for {:?}",
                    workspace.paths
                );
            }
        }
    }
}

pub async fn apply_restored_multiworkspace_state(
    window_handle: WindowHandle<MultiWorkspace>,
    state: &MultiWorkspaceState,
    fs: Arc<dyn fs::Fs>,
    cx: &mut AsyncApp,
) {
    let MultiWorkspaceState {
        sidebar_open,
        project_groups,
        sidebar_state,
        active_configuration_id,
        ..
    } = state;
    let restore_workspace_membership =
        cx.update(|cx| MultiWorkspace::retention_enabled_from_settings(cx));
    // Slot-structural: this restores whether the slot was open, not whether the
    // agent is enabled. It reads the agent setting only while the slot has one
    // occupant; `sidebar_slot_enabled` is the instance-side twin.
    let restore_sidebar_ui = cx.update(|cx| MultiWorkspace::sidebar_ui_enabled_from_settings(cx));

    if restore_workspace_membership {
        // Resolve linked worktree paths to their main repo paths so
        // stale keys from previous sessions get normalized and deduped.
        let mut resolved_groups: Vec<SerializedProjectGroupState> = Vec::new();
        for serialized in project_groups.iter().cloned() {
            let SerializedProjectGroupState { key, expanded } = serialized.into_restored_state();
            if key.path_list().paths().is_empty() {
                continue;
            }
            let mut resolved_paths = Vec::new();
            for path in key.path_list().paths() {
                if key.host().is_none()
                    && let Some(common_dir) =
                        project::discover_root_repo_common_dir(path, fs.as_ref()).await
                    && !project::is_submodule_git_dir(&common_dir)
                {
                    let main_path = project::repo_identity_path(&common_dir);
                    resolved_paths.push(main_path.to_path_buf());
                } else {
                    resolved_paths.push(path.to_path_buf());
                }
            }
            let resolved = ProjectGroupKey::new(key.host(), PathList::new(&resolved_paths));
            if !resolved_groups.iter().any(|g| g.key == resolved) {
                resolved_groups.push(SerializedProjectGroupState {
                    key: resolved,
                    expanded,
                });
            }
        }

        window_handle
            .update(cx, |multi_workspace, _window, cx| {
                multi_workspace.restore_project_groups(resolved_groups, cx);
            })
            .ok();
    }

    if let Some(configuration_id) = active_configuration_id {
        window_handle
            .update(cx, |multi_workspace, _window, cx| {
                multi_workspace.restore_configuration_association(*configuration_id, cx);
            })
            .ok();
    }

    if restore_sidebar_ui && *sidebar_open {
        window_handle
            .update(cx, |multi_workspace, _, cx| {
                multi_workspace.restore_open_sidebar(cx);
            })
            .ok();
    }

    if restore_sidebar_ui {
        if let Some(sidebar_state) = sidebar_state {
            window_handle
                .update(cx, |multi_workspace, window, cx| {
                    if let Some(sidebar) = multi_workspace.sidebar() {
                        sidebar.restore_serialized_state(sidebar_state, window, cx);
                    }
                    multi_workspace.serialize(cx);
                })
                .ok();
        }
    }
}

actions!(
    collab,
    [
        /// Opens the channel notes for the current call.
        ///
        /// Use `collab_panel::OpenSelectedChannelNotes` to open the channel notes for the selected
        /// channel in the collab panel.
        ///
        /// If you want to open a specific channel, use `zed::OpenZedUrl` with a channel notes URL -
        /// can be copied via "Copy link to section" in the context menu of the channel notes
        /// buffer. These URLs look like `https://zed.dev/channel/channel-name-CHANNEL_ID/notes`.
        OpenChannelNotes,
        /// Mutes your microphone.
        Mute,
        /// Deafens yourself (mute both microphone and speakers).
        Deafen,
        /// Leaves the current call.
        LeaveCall,
        /// Shares the current project with collaborators.
        ShareProject,
        /// Shares your screen with collaborators.
        ScreenShare,
        /// Copies the current room name and session id for debugging purposes.
        CopyRoomId,
    ]
);

/// Opens the channel notes for a specific channel by its ID.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = collab)]
#[serde(deny_unknown_fields)]
pub struct OpenChannelNotesById {
    pub channel_id: u64,
}

actions!(
    zed,
    [
        /// Opens the Zed log file.
        OpenLog,
        /// Reveals the Zed log file in the system file manager.
        RevealLogInFileManager
    ]
);

async fn join_channel_internal(
    channel_id: ChannelId,
    app_state: &Arc<AppState>,
    requesting_window: Option<WindowHandle<MultiWorkspace>>,
    requesting_workspace: Option<WeakEntity<Workspace>>,
    active_call: &dyn AnyActiveCall,
    cx: &mut AsyncApp,
) -> Result<bool> {
    let (should_prompt, already_in_channel) = cx.update(|cx| {
        if !active_call.is_in_room(cx) {
            return (false, false);
        }

        let already_in_channel = active_call.channel_id(cx) == Some(channel_id);
        let should_prompt = active_call.is_sharing_project(cx)
            && active_call.has_remote_participants(cx)
            && !already_in_channel;
        (should_prompt, already_in_channel)
    });

    if already_in_channel {
        let task = cx.update(|cx| {
            if let Some((project, host)) = active_call.most_active_project(cx) {
                Some(join_in_room_project(project, host, app_state.clone(), cx))
            } else {
                None
            }
        });
        if let Some(task) = task {
            task.await?;
        }
        return anyhow::Ok(true);
    }

    if should_prompt {
        if let Some(multi_workspace) = requesting_window {
            let answer = multi_workspace
                .update(cx, |_, window, cx| {
                    window.prompt(
                        PromptLevel::Warning,
                        "Do you want to switch channels?",
                        Some("Leaving this call will unshare your current project."),
                        &["Yes, Join Channel", "Cancel"],
                        cx,
                    )
                })?
                .await;

            if answer == Ok(1) {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }

    let client = cx.update(|cx| active_call.client(cx));

    let mut client_status = client.status();

    // this loop will terminate within client::CONNECTION_TIMEOUT seconds.
    'outer: loop {
        let Some(status) = client_status.recv().await else {
            anyhow::bail!("error connecting");
        };

        match status {
            Status::Connecting
            | Status::Authenticating
            | Status::Authenticated
            | Status::Reconnecting
            | Status::Reauthenticating
            | Status::Reauthenticated => continue,
            Status::Connected { .. } => break 'outer,
            Status::SignedOut | Status::AuthenticationError => {
                return Err(ErrorCode::SignedOut.into());
            }
            Status::UpgradeRequired => return Err(ErrorCode::UpgradeRequired.into()),
            Status::ConnectionError | Status::ConnectionLost | Status::ReconnectionError { .. } => {
                return Err(ErrorCode::Disconnected.into());
            }
        }
    }

    let joined = cx
        .update(|cx| active_call.join_channel(channel_id, cx))
        .await?;

    if !joined {
        return anyhow::Ok(true);
    }

    cx.update(|cx| active_call.room_update_completed(cx)).await;

    let task = cx.update(|cx| {
        if let Some((project, host)) = active_call.most_active_project(cx) {
            return Some(join_in_room_project(project, host, app_state.clone(), cx));
        }

        // If you are the first to join a channel, see if you should share your project.
        if !active_call.has_remote_participants(cx)
            && !active_call.local_participant_is_guest(cx)
            && let Some(workspace) = requesting_workspace.as_ref().and_then(|w| w.upgrade())
        {
            let project = workspace.update(cx, |workspace, cx| {
                let project = workspace.project.read(cx);

                if !active_call.share_on_join(cx) {
                    return None;
                }

                if (project.is_local() || project.is_via_remote_server())
                    && project.visible_worktrees(cx).any(|tree| {
                        tree.read(cx)
                            .root_entry()
                            .is_some_and(|entry| entry.is_dir())
                    })
                {
                    Some(workspace.project.clone())
                } else {
                    None
                }
            });
            if let Some(project) = project {
                let share_task = active_call.share_project(project, cx);
                return Some(cx.spawn(async move |_cx| -> Result<()> {
                    share_task.await?;
                    Ok(())
                }));
            }
        }

        None
    });
    if let Some(task) = task {
        task.await?;
        return anyhow::Ok(true);
    }
    anyhow::Ok(false)
}

pub fn join_channel(
    channel_id: ChannelId,
    app_state: Arc<AppState>,
    requesting_window: Option<WindowHandle<MultiWorkspace>>,
    requesting_workspace: Option<WeakEntity<Workspace>>,
    cx: &mut App,
) -> Task<Result<()>> {
    let active_call = GlobalAnyActiveCall::global(cx).clone();
    cx.spawn(async move |cx| {
        let result = join_channel_internal(
            channel_id,
            &app_state,
            requesting_window,
            requesting_workspace,
            &*active_call.0,
            cx,
        )
        .await;

        // join channel succeeded, and opened a window
        if matches!(result, Ok(true)) {
            return anyhow::Ok(());
        }

        // find an existing workspace to focus and show call controls
        let mut active_window = requesting_window.or_else(|| activate_any_workspace_window(cx));
        if active_window.is_none() {
            // no open workspaces, make one to show the error in (blergh)
            let OpenResult {
                window: window_handle,
                ..
            } = cx
                .update(|cx| {
                    Workspace::new_local(
                        vec![],
                        app_state.clone(),
                        requesting_window,
                        None,
                        None,
                        OpenMode::Activate,
                        cx,
                    )
                })
                .await?;

            window_handle
                .update(cx, |_, window, _cx| {
                    window.activate_window();
                })
                .ok();

            if result.is_ok() {
                cx.update(|cx| {
                    cx.dispatch_action(&OpenChannelNotes);
                });
            }

            active_window = Some(window_handle);
        }

        if let Err(err) = result {
            log::error!("failed to join channel: {}", err);
            if let Some(active_window) = active_window {
                active_window
                    .update(cx, |_, window, cx| {
                        let detail: SharedString = match err.error_code() {
                            ErrorCode::SignedOut => "Please sign in to continue.".into(),
                            ErrorCode::UpgradeRequired => concat!(
                                "Your are running an unsupported version of Zed. ",
                                "Please update to continue."
                            )
                            .into(),
                            ErrorCode::NoSuchChannel => concat!(
                                "No matching channel was found. ",
                                "Please check the link and try again."
                            )
                            .into(),
                            ErrorCode::Forbidden => concat!(
                                "This channel is private, and you do not have access. ",
                                "Please ask someone to add you and try again."
                            )
                            .into(),
                            ErrorCode::Disconnected => {
                                "Please check your internet connection and try again.".into()
                            }
                            _ => format!("{}\n\nPlease try again.", err).into(),
                        };
                        window.prompt(
                            PromptLevel::Critical,
                            "Failed to join channel",
                            Some(&detail),
                            &["OK"],
                            cx,
                        )
                    })?
                    .await
                    .ok();
            }
        }

        // return ok, we showed the error to the user.
        anyhow::Ok(())
    })
}

pub async fn get_any_active_multi_workspace(
    app_state: Arc<AppState>,
    mut cx: AsyncApp,
) -> anyhow::Result<WindowHandle<MultiWorkspace>> {
    // find an existing workspace to focus and show call controls
    let active_window = activate_any_workspace_window(&mut cx);
    if active_window.is_none() {
        cx.update(|cx| {
            Workspace::new_local(
                vec![],
                app_state.clone(),
                None,
                None,
                None,
                OpenMode::Activate,
                cx,
            )
        })
        .await?;
    }
    activate_any_workspace_window(&mut cx).context("could not open zed")
}

pub fn activate_any_workspace_window(cx: &mut AsyncApp) -> Option<WindowHandle<MultiWorkspace>> {
    cx.update(|cx| {
        if let Some(workspace_window) = cx
            .active_window()
            .and_then(|window| window.downcast::<MultiWorkspace>())
        {
            return Some(workspace_window);
        }

        for window in cx.windows() {
            if let Some(workspace_window) = window.downcast::<MultiWorkspace>() {
                workspace_window
                    .update(cx, |_, window, _| window.activate_window())
                    .ok();
                return Some(workspace_window);
            }
        }
        None
    })
}

pub fn workspace_windows_for_location(
    serialized_location: &SerializedWorkspaceLocation,
    cx: &App,
) -> Vec<WindowHandle<MultiWorkspace>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<MultiWorkspace>())
        .filter(|multi_workspace| {
            let same_host = |left: &RemoteConnectionOptions, right: &RemoteConnectionOptions| match (left, right) {
                (RemoteConnectionOptions::Ssh(a), RemoteConnectionOptions::Ssh(b)) => {
                    (&a.host, &a.username, &a.port) == (&b.host, &b.username, &b.port)
                }
                (RemoteConnectionOptions::Wsl(a), RemoteConnectionOptions::Wsl(b)) => {
                    // The WSL username is not consistently populated in the workspace location, so ignore it for now.
                    a.distro_name == b.distro_name
                }
                (RemoteConnectionOptions::Docker(a), RemoteConnectionOptions::Docker(b)) => {
                    a.container_id == b.container_id
                }
                #[cfg(any(test, feature = "test-support"))]
                (RemoteConnectionOptions::Mock(a), RemoteConnectionOptions::Mock(b)) => {
                    a.id == b.id
                }
                _ => false,
            };

            multi_workspace.read(cx).is_ok_and(|multi_workspace| {
                multi_workspace.workspaces().any(|workspace| {
                    match workspace.read(cx).workspace_location(cx) {
                        WorkspaceLocation::Location(location, _) => {
                            match (&location, serialized_location) {
                                (
                                    SerializedWorkspaceLocation::Local,
                                    SerializedWorkspaceLocation::Local,
                                ) => true,
                                (
                                    SerializedWorkspaceLocation::Remote(a),
                                    SerializedWorkspaceLocation::Remote(b),
                                ) => same_host(a, b),
                                _ => false,
                            }
                        }
                        _ => false,
                    }
                })
            })
        })
        .collect()
}

pub async fn find_existing_workspace(
    abs_paths: &[PathBuf],
    open_options: &OpenOptions,
    location: &SerializedWorkspaceLocation,
    cx: &mut AsyncApp,
) -> (
    Option<(WindowHandle<MultiWorkspace>, Entity<Workspace>)>,
    OpenVisible,
) {
    let mut existing: Option<(WindowHandle<MultiWorkspace>, Entity<Workspace>)> = None;
    let mut open_visible = OpenVisible::All;
    let mut best_match = None;

    if open_options.workspace_matching != WorkspaceMatching::None {
        cx.update(|cx| {
            for window in workspace_windows_for_location(location, cx) {
                if let Ok(multi_workspace) = window.read(cx) {
                    for workspace in multi_workspace.workspaces() {
                        let project = workspace.read(cx).project.read(cx);
                        let m = match open_options.workspace_matching {
                            WorkspaceMatching::None => None,
                            WorkspaceMatching::MatchExact => {
                                project.visibility_for_paths(abs_paths, true, cx)
                            }
                            WorkspaceMatching::MatchSubpaths => {
                                project.visibility_for_subpaths(abs_paths, cx)
                            }
                            WorkspaceMatching::MatchSubdirectory => {
                                project.visibility_for_paths(abs_paths, false, cx)
                            }
                        };
                        if m > best_match {
                            existing = Some((window, workspace.clone()));
                            best_match = m;
                        } else if best_match.is_none()
                            && open_options.workspace_matching
                                == WorkspaceMatching::MatchSubdirectory
                        {
                            existing = Some((window, workspace.clone()))
                        }
                    }
                }
            }
        });

        let all_paths_are_files = existing
            .as_ref()
            .and_then(|(_, target_workspace)| {
                cx.update(|cx| {
                    let workspace = target_workspace.read(cx);
                    let project = workspace.project.read(cx);
                    let path_style = workspace.path_style(cx);
                    Some(!abs_paths.iter().any(|path| {
                        let path = util::paths::SanitizedPath::new(path);
                        project.worktrees(cx).any(|worktree| {
                            let worktree = worktree.read(cx);
                            let abs_path = worktree.abs_path();
                            path_style
                                .strip_prefix(path.as_ref(), abs_path.as_ref())
                                .and_then(|rel| worktree.entry_for_path(&rel))
                                .is_some_and(|e| e.is_dir())
                        })
                    }))
                })
            })
            .unwrap_or(false);

        if open_options.wait && existing.is_some() && all_paths_are_files {
            cx.update(|cx| {
                let windows = workspace_windows_for_location(location, cx);
                let window = cx
                    .active_window()
                    .and_then(|window| window.downcast::<MultiWorkspace>())
                    .filter(|window| windows.contains(window))
                    .or_else(|| windows.into_iter().next());
                if let Some(window) = window {
                    if let Ok(multi_workspace) = window.read(cx) {
                        let active_workspace = multi_workspace.workspace().clone();
                        existing = Some((window, active_workspace));
                        open_visible = OpenVisible::None;
                    }
                }
            });
        }
    }
    (existing, open_visible)
}

/// Controls whether to reuse an existing workspace whose worktrees contain the
/// given paths, and how broadly to match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WorkspaceMatching {
    /// Always open a new workspace. No matching against existing worktrees.
    None,
    /// Match paths against existing worktree roots and files within them.
    #[default]
    MatchExact,
    /// Match files and directories inside existing worktrees, excluding the
    /// worktree roots themselves.
    MatchSubpaths,
    /// Match paths against existing worktrees including subdirectories, and
    /// fall back to any existing window if no worktree matched.
    ///
    /// For example, `zed -a foo/bar` will activate the `bar` workspace if it
    /// exists, otherwise it will open a new window with `foo/bar` as the root.
    MatchSubdirectory,
}

#[derive(Clone)]
pub struct OpenOptions {
    pub visible: Option<OpenVisible>,
    pub focus: Option<bool>,
    pub workspace_matching: WorkspaceMatching,
    /// Whether to add unmatched directories to the existing window's sidebar
    /// rather than opening a new window. Defaults to true, matching the default
    /// `cli_default_open_behavior` setting.
    pub add_dirs_to_sidebar: bool,
    pub wait: bool,
    pub requesting_window: Option<WindowHandle<MultiWorkspace>>,
    pub open_mode: OpenMode,
    pub env: Option<HashMap<String, String>>,
    pub open_in_dev_container: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            visible: None,
            focus: None,
            workspace_matching: WorkspaceMatching::default(),
            add_dirs_to_sidebar: true,
            wait: false,
            requesting_window: None,
            open_mode: OpenMode::default(),
            env: None,
            open_in_dev_container: false,
        }
    }
}

impl OpenOptions {
    fn should_reuse_existing_window(&self) -> bool {
        !matches!(
            self.workspace_matching,
            WorkspaceMatching::None | WorkspaceMatching::MatchSubpaths
        ) && self.open_mode != OpenMode::NewWindow
    }
}

/// The result of opening a workspace via [`open_paths`], [`Workspace::new_local`],
/// or [`Workspace::open_workspace_for_paths`].
pub struct OpenResult {
    pub window: WindowHandle<MultiWorkspace>,
    pub workspace: Entity<Workspace>,
    pub opened_items: Vec<Option<anyhow::Result<Box<dyn ItemHandle>>>>,
}

/// Opens a workspace by its database ID, used for restoring empty workspaces with unsaved content.
pub fn open_workspace_by_id(
    workspace_id: WorkspaceId,
    app_state: Arc<AppState>,
    requesting_window: Option<WindowHandle<MultiWorkspace>>,
    cx: &mut App,
) -> Task<anyhow::Result<WindowHandle<MultiWorkspace>>> {
    let project_handle = Project::local(
        app_state.client.clone(),
        app_state.node_runtime.clone(),
        app_state.user_store.clone(),
        app_state.languages.clone(),
        app_state.fs.clone(),
        None,
        project::LocalProjectFlags {
            init_worktree_trust: true,
            ..project::LocalProjectFlags::default()
        },
        cx,
    );

    let db = WorkspaceDb::global(cx);
    let kvp = db::kvp::KeyValueStore::global(cx);
    cx.spawn(async move |cx| {
        let serialized_workspace = db
            .workspace_for_id(workspace_id)
            .with_context(|| format!("Workspace {workspace_id:?} not found"))?;

        let centered_layout = serialized_workspace.centered_layout;

        let (window, workspace) = if let Some(window) = requesting_window {
            let workspace = window.update(cx, |multi_workspace, window, cx| {
                let workspace = cx.new(|cx| {
                    let mut workspace = Workspace::new(
                        Some(workspace_id),
                        project_handle.clone(),
                        app_state.clone(),
                        window,
                        cx,
                    );
                    workspace.centered_layout = centered_layout;
                    workspace
                });
                multi_workspace.add(workspace.clone(), window, cx);
                workspace
            })?;
            (window, workspace)
        } else {
            let window_bounds_override = window_bounds_env_override();

            let (window_bounds, display) = if let Some(bounds) = window_bounds_override {
                (Some(WindowBounds::Windowed(bounds)), None)
            } else if let Some(display) = serialized_workspace.display
                && let Some(bounds) = serialized_workspace.window_bounds.as_ref()
            {
                (Some(bounds.0), Some(display))
            } else if let Some((display, bounds)) = persistence::read_default_window_bounds(&kvp) {
                (Some(bounds), Some(display))
            } else {
                (None, None)
            };

            let options = cx.update(|cx| {
                let mut options = (app_state.build_window_options)(display, cx);
                options.window_bounds = window_bounds;
                options
            });

            let window = cx.open_window(options, {
                let app_state = app_state.clone();
                let project_handle = project_handle.clone();
                move |window, cx| {
                    let workspace = cx.new(|cx| {
                        let mut workspace = Workspace::new(
                            Some(workspace_id),
                            project_handle,
                            app_state,
                            window,
                            cx,
                        );
                        workspace.centered_layout = centered_layout;
                        workspace
                    });
                    cx.new(|cx| MultiWorkspace::new(workspace, window, cx))
                }
            })?;

            let workspace = window.update(cx, |multi_workspace: &mut MultiWorkspace, _, _cx| {
                multi_workspace.workspace().clone()
            })?;

            (window, workspace)
        };

        notify_if_database_failed(window, cx);

        // Restore items from the serialized workspace
        window
            .update(cx, |_, window, cx| {
                workspace.update(cx, |_workspace, cx| {
                    open_items(Some(serialized_workspace), vec![], window, cx)
                })
            })?
            .await?;

        window.update(cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.serialize_workspace(window, cx);
            });
        })?;

        Ok(window)
    })
}

#[allow(clippy::type_complexity)]
pub fn open_paths(
    abs_paths: &[PathBuf],
    app_state: Arc<AppState>,
    mut open_options: OpenOptions,
    cx: &mut App,
) -> Task<anyhow::Result<OpenResult>> {
    let abs_paths = abs_paths.to_vec();
    #[cfg(target_os = "windows")]
    let wsl_path = abs_paths
        .iter()
        .find_map(|p| util::paths::WslPath::from_path(p));

    cx.spawn(async move |cx| {
        let (mut existing, mut open_visible) = find_existing_workspace(
            &abs_paths,
            &open_options,
            &SerializedWorkspaceLocation::Local,
            cx,
        )
        .await;

        // Fallback: if no workspace contains the paths and all paths are files,
        // prefer an existing local workspace window (active window first).
        if open_options.should_reuse_existing_window() && existing.is_none() {
            let all_paths = abs_paths.iter().map(|path| app_state.fs.metadata(path));
            let all_metadatas = futures::future::join_all(all_paths)
                .await
                .into_iter()
                .filter_map(|result| result.ok().flatten());

            if all_metadatas.into_iter().all(|file| !file.is_dir) {
                cx.update(|cx| {
                    let windows = workspace_windows_for_location(
                        &SerializedWorkspaceLocation::Local,
                        cx,
                    );
                    let window = cx
                        .active_window()
                        .and_then(|window| window.downcast::<MultiWorkspace>())
                        .filter(|window| windows.contains(window))
                        .or_else(|| windows.into_iter().next());
                    if let Some(window) = window {
                        if let Ok(multi_workspace) = window.read(cx) {
                            let active_workspace = multi_workspace.workspace().clone();
                            existing = Some((window, active_workspace));
                            open_visible = OpenVisible::None;
                        }
                    }
                });
            }
        }

        // Fallback for directories: when no flag is specified and no existing
        // workspace matched, check the user's setting to decide whether to add
        // the directory as a new workspace in the active window's MultiWorkspace
        // or open a new window.
        // Skip when requesting_window is already set: the caller (e.g.
        // open_workspace_for_paths reusing an empty window) already chose the
        // target window, so we must not open the sidebar as a side-effect.
        if open_options.should_reuse_existing_window()
            && existing.is_none()
            && open_options.requesting_window.is_none()
        {
            let use_existing_window = open_options.add_dirs_to_sidebar;

            if use_existing_window {
                let target_window = cx.update(|cx| {
                    let windows = workspace_windows_for_location(
                        &SerializedWorkspaceLocation::Local,
                        cx,
                    );
                    let window = cx
                        .active_window()
                        .and_then(|window| window.downcast::<MultiWorkspace>())
                        .filter(|window| windows.contains(window))
                        .or_else(|| windows.into_iter().next());
                    window.filter(|window| {
                        window.read(cx).is_ok_and(|mw| mw.retention_enabled(cx))
                    })
                });

                if let Some(window) = target_window {
                    open_options.requesting_window = Some(window);
                    window
                        .update(cx, |multi_workspace, _, cx| {
                            if multi_workspace.sidebar_slot_enabled(cx) {
                                multi_workspace.open_sidebar(cx);
                            }
                        })
                        .log_err();
                }
            }
        }

        let open_in_dev_container = open_options.open_in_dev_container;

        let result = if let Some((existing, target_workspace)) = existing {
            let open_task = existing
                .update(cx, |multi_workspace, window, cx| {
                    window.activate_window();
                    // `#zed-66.4`: this branch never reaches `Workspace::new_local`, so
                    // the seam inside it cannot see this open. Reopening a project the
                    // window already holds displaces the current workspace just the same,
                    // and without this every such open from an empty one strands a row.
                    multi_workspace.activate_as_open(
                        target_workspace.clone(),
                        None,
                        window,
                        cx,
                    );
                    target_workspace.update(cx, |workspace, cx| {
                        if open_in_dev_container {
                            workspace.set_open_in_dev_container(true);
                        }
                        workspace.open_paths(
                            abs_paths,
                            OpenOptions {
                                visible: Some(open_visible),
                                ..Default::default()
                            },
                            None,
                            window,
                            cx,
                        )
                    })
                })?
                .await;

            _ = existing.update(cx, |multi_workspace, _, cx| {
                let workspace = multi_workspace.workspace().clone();
                workspace.update(cx, |workspace, cx| {
                    for item in open_task.iter().flatten() {
                        if let Err(e) = item {
                            workspace.show_error(format!("Error: {e}"), cx);
                        }
                    }
                });
            });

            Ok(OpenResult { window: existing, workspace: target_workspace, opened_items: open_task })
        } else {
            let init = if open_in_dev_container {
                Some(Box::new(|workspace: &mut Workspace, _window: &mut Window, _cx: &mut Context<Workspace>| {
                    workspace.set_open_in_dev_container(true);
                }) as Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send>)
            } else {
                None
            };
            let result = cx
                .update(move |cx| {
                    Workspace::new_local(
                        abs_paths,
                        app_state.clone(),
                        open_options.requesting_window,
                        open_options.env,
                        init,
                        open_options.open_mode,
                        cx,
                    )
                })
                .await;

            if let Ok(ref result) = result {
                result.window
                    .update(cx, |_, window, _cx| {
                        window.activate_window();
                    })
                    .log_err();
            }

            result
        };

        #[cfg(target_os = "windows")]
        if let Some(util::paths::WslPath{distro, path}) = wsl_path
            && let Ok(ref result) = result
        {
            result.window
                .update(cx, move |multi_workspace, _window, cx| {
                    struct OpenInWsl;
                    let workspace = multi_workspace.workspace().clone();
                    workspace.update(cx, |workspace, cx| {
                        workspace.show_notification(NotificationId::unique::<OpenInWsl>(), cx, move |cx| {
                            let display_path = util::markdown::MarkdownInlineCode(&path.to_string_lossy());
                            let msg = format!("{display_path} is inside a WSL filesystem, some features may not work unless you open it with WSL remote");
                            cx.new(move |cx| {
                                MessageNotification::new(msg, cx)
                                    .primary_message("Open in WSL")
                                    .primary_icon(IconName::FolderOpen)
                                    .primary_on_click(move |window, cx| {
                                        window.dispatch_action(Box::new(remote::OpenWslPath {
                                                distro: remote::WslConnectionOptions {
                                                        distro_name: distro.clone(),
                                                    user: None,
                                                },
                                                paths: vec![path.clone().into()],
                                            }), cx)
                                    })
                            })
                        });
                    });
                })
                .unwrap();
        };
        result
    })
}

pub fn open_new(
    open_options: OpenOptions,
    app_state: Arc<AppState>,
    cx: &mut App,
    init: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static + Send,
) -> Task<anyhow::Result<()>> {
    let addition = open_options.open_mode;
    let task = Workspace::new_local(
        Vec::new(),
        app_state,
        open_options.requesting_window,
        open_options.env,
        Some(Box::new(init)),
        addition,
        cx,
    );
    cx.spawn(async move |cx| {
        let OpenResult { window, .. } = task.await?;
        window
            .update(cx, |_, window, _cx| {
                window.activate_window();
            })
            .ok();
        Ok(())
    })
}

pub fn create_and_open_local_file(
    path: &'static Path,
    window: &mut Window,
    cx: &mut Context<Workspace>,
    default_content: impl 'static + Send + FnOnce() -> Rope,
) -> Task<Result<Box<dyn ItemHandle>>> {
    cx.spawn_in(window, async move |workspace, cx| {
        let fs = workspace.read_with(cx, |workspace, _| workspace.app_state().fs.clone())?;
        if !fs.is_file(path).await {
            fs.create_file(path, Default::default()).await?;
            fs.save(path, &default_content(), Default::default())
                .await?;
        }

        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.with_local_or_wsl_workspace(window, cx, |workspace, window, cx| {
                    let path = workspace
                        .project
                        .read_with(cx, |project, cx| project.try_windows_path_to_wsl(path, cx));
                    cx.spawn_in(window, async move |workspace, cx| {
                        let path = path.await?;

                        let path = fs.canonicalize(&path).await.unwrap_or(path);

                        let mut items = workspace
                            .update_in(cx, |workspace, window, cx| {
                                workspace.open_paths(
                                    vec![path.to_path_buf()],
                                    OpenOptions {
                                        visible: Some(OpenVisible::None),
                                        ..Default::default()
                                    },
                                    None,
                                    window,
                                    cx,
                                )
                            })?
                            .await;
                        let item = items.pop().flatten();
                        item.with_context(|| format!("path {path:?} is not a file"))?
                    })
                })
            })?
            .await?
            .await
    })
}

pub fn open_remote_project_with_new_connection(
    window: WindowHandle<MultiWorkspace>,
    remote_connection: Arc<dyn RemoteConnection>,
    cancel_rx: oneshot::Receiver<()>,
    delegate: Arc<dyn RemoteClientDelegate>,
    app_state: Arc<AppState>,
    paths: Vec<PathBuf>,
    cx: &mut App,
) -> Task<Result<(Option<Entity<Workspace>>, Vec<Option<Box<dyn ItemHandle>>>)>> {
    cx.spawn(async move |cx| {
        let (workspace_id, serialized_workspace) =
            deserialize_remote_project(remote_connection.connection_options(), paths.clone(), cx)
                .await?;

        let session = match cx
            .update(|cx| {
                remote::RemoteClient::new(
                    ConnectionIdentifier::Workspace(workspace_id.0),
                    remote_connection,
                    cancel_rx,
                    delegate,
                    cx,
                )
            })
            .await?
        {
            Some(result) => result,
            None => return Ok((None, Vec::new())),
        };

        let project = cx.update(|cx| {
            project::Project::remote(
                session,
                app_state.client.clone(),
                app_state.node_runtime.clone(),
                app_state.user_store.clone(),
                app_state.languages.clone(),
                app_state.fs.clone(),
                true,
                cx,
            )
        });

        let (workspace, items) = open_remote_project_inner(
            project,
            paths,
            workspace_id,
            serialized_workspace,
            app_state,
            window,
            None,
            None,
            cx,
        )
        .await?;
        Ok((Some(workspace), items))
    })
}

pub fn open_remote_project_with_existing_connection(
    connection_options: RemoteConnectionOptions,
    project: Entity<Project>,
    paths: Vec<PathBuf>,
    app_state: Arc<AppState>,
    window: WindowHandle<MultiWorkspace>,
    provisional_project_group_key: Option<ProjectGroupKey>,
    source_workspace: Option<WeakEntity<Workspace>>,
    cx: &mut AsyncApp,
) -> Task<Result<(Entity<Workspace>, Vec<Option<Box<dyn ItemHandle>>>)>> {
    cx.spawn(async move |cx| {
        let (workspace_id, serialized_workspace) =
            deserialize_remote_project(connection_options.clone(), paths.clone(), cx).await?;

        open_remote_project_inner(
            project,
            paths,
            workspace_id,
            serialized_workspace,
            app_state,
            window,
            provisional_project_group_key,
            source_workspace,
            cx,
        )
        .await
    })
}

async fn open_remote_project_inner(
    project: Entity<Project>,
    paths: Vec<PathBuf>,
    workspace_id: WorkspaceId,
    serialized_workspace: Option<SerializedWorkspace>,
    app_state: Arc<AppState>,
    window: WindowHandle<MultiWorkspace>,
    provisional_project_group_key: Option<ProjectGroupKey>,
    source_workspace: Option<WeakEntity<Workspace>>,
    cx: &mut AsyncApp,
) -> Result<(Entity<Workspace>, Vec<Option<Box<dyn ItemHandle>>>)> {
    let mut project_paths_to_open = vec![];
    let mut project_path_errors = vec![];

    for path in paths {
        let result = cx
            .update(|cx| {
                Workspace::project_path_for_path(project.clone(), path.as_path(), true, cx)
            })
            .await;
        match result {
            Ok((_, project_path)) => {
                project_paths_to_open.push((path, Some(project_path)));
            }
            Err(error) => {
                project_path_errors.push(error);
            }
        };
    }

    if project_paths_to_open.is_empty() {
        return Err(project_path_errors.pop().context("no paths given")?);
    }

    let workspace = window.update(cx, |multi_workspace, window, cx| {
        let new_workspace = cx.new(|cx| {
            let mut workspace = Workspace::new(
                Some(workspace_id),
                project.clone(),
                app_state.clone(),
                window,
                cx,
            );
            workspace.update_history(cx);

            if let Some(ref serialized) = serialized_workspace {
                workspace.centered_layout = serialized.centered_layout;
            }

            workspace
        });

        if let Some(project_group_key) = provisional_project_group_key.clone() {
            multi_workspace.activate_provisional_workspace(
                new_workspace.clone(),
                project_group_key,
                window,
                cx,
            );
        } else {
            multi_workspace.activate(new_workspace.clone(), source_workspace, window, cx);
        }
        new_workspace
    })?;

    let db = cx.update(|cx| WorkspaceDb::global(cx));
    let toolchains = db.toolchains(workspace_id).await?;
    for (toolchain, worktree_path, path) in toolchains {
        project
            .update(cx, |this, cx| {
                let Some(worktree_id) =
                    this.find_worktree(&worktree_path, cx)
                        .and_then(|(worktree, rel_path)| {
                            if rel_path.is_empty() {
                                Some(worktree.read(cx).id())
                            } else {
                                None
                            }
                        })
                else {
                    return Task::ready(None);
                };

                this.activate_toolchain(ProjectPath { worktree_id, path }, toolchain, cx)
            })
            .await;
    }

    let items = window
        .update(cx, |_, window, cx| {
            window.activate_window();
            workspace.update(cx, |_workspace, cx| {
                open_items(serialized_workspace, project_paths_to_open, window, cx)
            })
        })?
        .await?;

    workspace.update(cx, |workspace, cx| {
        for error in project_path_errors {
            if error.error_code() == proto::ErrorCode::DevServerProjectPathDoesNotExist {
                if let Some(path) = error.error_tag("path") {
                    workspace.show_error(format!("'{path}' does not exist"), cx)
                }
            } else {
                workspace.show_error(format!("{error}"), cx)
            }
        }
    });

    Ok((
        workspace,
        items.into_iter().map(|item| item?.ok()).collect(),
    ))
}

fn deserialize_remote_project(
    connection_options: RemoteConnectionOptions,
    paths: Vec<PathBuf>,
    cx: &AsyncApp,
) -> Task<Result<(WorkspaceId, Option<SerializedWorkspace>)>> {
    let db = cx.update(|cx| WorkspaceDb::global(cx));
    cx.background_spawn(async move {
        let remote_connection_id = db
            .get_or_create_remote_connection(connection_options)
            .await?;

        let serialized_workspace = db.remote_workspace_for_roots(&paths, remote_connection_id);

        let workspace_id = if let Some(workspace_id) =
            serialized_workspace.as_ref().map(|workspace| workspace.id)
        {
            workspace_id
        } else {
            db.next_id().await?
        };

        Ok((workspace_id, serialized_workspace))
    })
}

pub fn join_in_room_project(
    project_id: u64,
    follow_user_id: u64,
    app_state: Arc<AppState>,
    cx: &mut App,
) -> Task<Result<()>> {
    let windows = cx.windows();
    cx.spawn(async move |cx| {
        let existing_window_and_workspace: Option<(
            WindowHandle<MultiWorkspace>,
            Entity<Workspace>,
        )> = windows.into_iter().find_map(|window_handle| {
            window_handle
                .downcast::<MultiWorkspace>()
                .and_then(|window_handle| {
                    window_handle
                        .update(cx, |multi_workspace, _window, cx| {
                            multi_workspace
                                .workspaces()
                                .find(|workspace| {
                                    workspace.read(cx).project().read(cx).remote_id()
                                        == Some(project_id)
                                })
                                .map(|workspace| (window_handle, workspace.clone()))
                        })
                        .unwrap_or(None)
                })
        });

        let multi_workspace_window = if let Some((existing_window, target_workspace)) =
            existing_window_and_workspace
        {
            existing_window
                .update(cx, |multi_workspace, window, cx| {
                    multi_workspace.activate(target_workspace, None, window, cx);
                })
                .ok();
            existing_window
        } else {
            let active_call = cx.update(|cx| GlobalAnyActiveCall::global(cx).clone());
            let project = cx
                .update(|cx| {
                    active_call.0.join_project(
                        project_id,
                        app_state.languages.clone(),
                        app_state.fs.clone(),
                        cx,
                    )
                })
                .await?;

            let window_bounds_override = window_bounds_env_override();
            cx.update(|cx| {
                let mut options = (app_state.build_window_options)(None, cx);
                options.window_bounds = window_bounds_override.map(WindowBounds::Windowed);
                cx.open_window(options, |window, cx| {
                    let workspace = cx.new(|cx| {
                        Workspace::new(Default::default(), project, app_state.clone(), window, cx)
                    });
                    cx.new(|cx| MultiWorkspace::new(workspace, window, cx))
                })
            })?
        };

        multi_workspace_window.update(cx, |multi_workspace, window, cx| {
            cx.activate(true);
            window.activate_window();

            // We set the active workspace above, so this is the correct workspace.
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, |workspace, cx| {
                let follow_peer_id = GlobalAnyActiveCall::try_global(cx)
                    .and_then(|call| call.0.peer_id_for_user_in_room(follow_user_id, cx))
                    .or_else(|| {
                        // If we couldn't follow the given user, follow the host instead.
                        let collaborator = workspace
                            .project()
                            .read(cx)
                            .collaborators()
                            .values()
                            .find(|collaborator| collaborator.is_host)?;
                        Some(collaborator.peer_id)
                    });

                if let Some(follow_peer_id) = follow_peer_id {
                    workspace.follow(follow_peer_id, window, cx);
                }
            });
        })?;

        anyhow::Ok(())
    })
}

pub fn reload(cx: &mut App) {
    let should_confirm = WorkspaceSettings::get_global(cx).confirm_quit;
    let mut workspace_windows = cx
        .windows()
        .into_iter()
        .filter_map(|window| window.downcast::<MultiWorkspace>())
        .collect::<Vec<_>>();

    // If multiple windows have unsaved changes, and need a save prompt,
    // prompt in the active window before switching to a different window.
    workspace_windows.sort_by_key(|window| window.is_active(cx) == Some(false));

    let mut prompt = None;
    if let (true, Some(window)) = (should_confirm, workspace_windows.first()) {
        prompt = window
            .update(cx, |_, window, cx| {
                window.prompt(
                    PromptLevel::Info,
                    "Are you sure you want to restart?",
                    None,
                    &["Restart", "Cancel"],
                    cx,
                )
            })
            .ok();
    }

    cx.spawn(async move |cx| {
        if let Some(prompt) = prompt {
            let answer = prompt.await?;
            if answer != 0 {
                return anyhow::Ok(());
            }
        }

        // If the user cancels any save prompt, then keep the app open.
        for window in workspace_windows {
            if let Ok(should_close) = window.update(cx, |multi_workspace, window, cx| {
                let workspace = multi_workspace.workspace().clone();
                workspace.update(cx, |workspace, cx| {
                    workspace.prepare_to_close(CloseIntent::Quit, window, cx)
                })
            }) && !should_close.await?
            {
                return anyhow::Ok(());
            }
        }
        cx.update(|cx| cx.restart());
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

fn parse_pixel_position_env_var(value: &str) -> Option<Point<Pixels>> {
    let mut parts = value.split(',');
    let x: usize = parts.next()?.parse().ok()?;
    let y: usize = parts.next()?.parse().ok()?;
    Some(point(px(x as f32), px(y as f32)))
}

fn parse_pixel_size_env_var(value: &str) -> Option<Size<Pixels>> {
    let mut parts = value.split(',');
    let width: usize = parts.next()?.parse().ok()?;
    let height: usize = parts.next()?.parse().ok()?;
    Some(size(px(width as f32), px(height as f32)))
}

/// Add client-side decorations (rounded corners, shadows, resize handling) when
/// appropriate.
pub fn client_side_decorations(
    element: impl IntoElement,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    const BORDER_SIZE: Pixels = px(1.0);
    let decorations = window.window_decorations();
    let is_resizable = window.is_resizable();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };

    match decorations {
        Decorations::Client { .. } => window.set_client_inset(theme::CLIENT_SIDE_DECORATION_SHADOW),
        Decorations::Server => window.set_client_inset(px(0.0)),
    }

    struct GlobalResizeEdge(ResizeEdge);
    impl Global for GlobalResizeEdge {}

    div()
        .id("window-backdrop")
        .bg(transparent_black())
        .map(|div| match decorations {
            Decorations::Server => div,
            Decorations::Client { .. } => div
                .rounded_client_corners(tiling)
                .when(!tiling.top, |div| {
                    div.pt(theme::CLIENT_SIDE_DECORATION_SHADOW)
                })
                .when(!tiling.bottom, |div| {
                    div.pb(theme::CLIENT_SIDE_DECORATION_SHADOW)
                })
                .when(!tiling.left, |div| {
                    div.pl(theme::CLIENT_SIDE_DECORATION_SHADOW)
                })
                .when(!tiling.right, |div| {
                    div.pr(theme::CLIENT_SIDE_DECORATION_SHADOW)
                })
                .when(is_resizable, |div| {
                    div.on_mouse_move(move |e, window, cx| {
                        let size = window.window_bounds().get_bounds().size;
                        let pos = e.position;

                        let new_edge =
                            resize_edge(pos, theme::CLIENT_SIDE_DECORATION_SHADOW, size, tiling);

                        let edge = cx.try_global::<GlobalResizeEdge>();
                        if new_edge != edge.map(|edge| edge.0) {
                            window
                                .window_handle()
                                .update(cx, |workspace, _, cx| {
                                    cx.notify(workspace.entity_id());
                                })
                                .ok();
                        }
                    })
                    .on_mouse_down(MouseButton::Left, move |e, window, _| {
                        let size = window.window_bounds().get_bounds().size;
                        let pos = e.position;

                        let edge = match resize_edge(
                            pos,
                            theme::CLIENT_SIDE_DECORATION_SHADOW,
                            size,
                            tiling,
                        ) {
                            Some(value) => value,
                            None => return,
                        };

                        window.start_window_resize(edge);
                    })
                }),
        })
        .size_full()
        .child(
            div()
                .cursor(CursorStyle::Arrow)
                .map(|div| match decorations {
                    Decorations::Server => div,
                    Decorations::Client { .. } => div
                        .border_color(cx.theme().colors().border)
                        .rounded_client_corners(tiling)
                        .when(!tiling.top, |div| div.border_t(BORDER_SIZE))
                        .when(!tiling.bottom, |div| div.border_b(BORDER_SIZE))
                        .when(!tiling.left, |div| div.border_l(BORDER_SIZE))
                        .when(!tiling.right, |div| div.border_r(BORDER_SIZE))
                        .when(!tiling.is_tiled(), |div| {
                            div.shadow(vec![
                                gpui::BoxShadow::new(
                                    px(0.),
                                    px(0.),
                                    Hsla {
                                        h: 0.,
                                        s: 0.,
                                        l: 0.,
                                        a: 0.4,
                                    },
                                )
                                .blur_radius(theme::CLIENT_SIDE_DECORATION_SHADOW / 2.),
                            ])
                        }),
                })
                .on_mouse_move(|_e, _, cx| {
                    cx.stop_propagation();
                })
                .size_full()
                .child(element),
        )
        .map(|div| match decorations {
            Decorations::Server => div,
            Decorations::Client { tiling, .. } if is_resizable => div.child(
                canvas(
                    |_bounds, window, _| {
                        window.insert_hitbox(
                            Bounds::new(
                                point(px(0.0), px(0.0)),
                                window.window_bounds().get_bounds().size,
                            ),
                            HitboxBehavior::Normal,
                        )
                    },
                    move |_bounds, hitbox, window, cx| {
                        let mouse = window.mouse_position();
                        let size = window.window_bounds().get_bounds().size;
                        let Some(edge) =
                            resize_edge(mouse, theme::CLIENT_SIDE_DECORATION_SHADOW, size, tiling)
                        else {
                            return;
                        };
                        cx.set_global(GlobalResizeEdge(edge));
                        window.set_cursor_style(
                            match edge {
                                ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                                ResizeEdge::Left | ResizeEdge::Right => {
                                    CursorStyle::ResizeLeftRight
                                }
                                ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                    CursorStyle::ResizeUpLeftDownRight
                                }
                                ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                    CursorStyle::ResizeUpRightDownLeft
                                }
                            },
                            &hitbox,
                        );
                    },
                )
                .size_full()
                .absolute(),
            ),
            Decorations::Client { .. } => div,
        })
}

fn resize_edge(
    pos: Point<Pixels>,
    shadow_size: Pixels,
    window_size: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let bounds = Bounds::new(Point::default(), window_size).inset(shadow_size * 1.5);
    if bounds.contains(&pos) {
        return None;
    }

    let corner_size = size(shadow_size * 1.5, shadow_size * 1.5);
    let top_left_bounds = Bounds::new(Point::new(px(0.), px(0.)), corner_size);
    if !tiling.top && top_left_bounds.contains(&pos) {
        return Some(ResizeEdge::TopLeft);
    }

    let top_right_bounds = Bounds::new(
        Point::new(window_size.width - corner_size.width, px(0.)),
        corner_size,
    );
    if !tiling.top && top_right_bounds.contains(&pos) {
        return Some(ResizeEdge::TopRight);
    }

    let bottom_left_bounds = Bounds::new(
        Point::new(px(0.), window_size.height - corner_size.height),
        corner_size,
    );
    if !tiling.bottom && bottom_left_bounds.contains(&pos) {
        return Some(ResizeEdge::BottomLeft);
    }

    let bottom_right_bounds = Bounds::new(
        Point::new(
            window_size.width - corner_size.width,
            window_size.height - corner_size.height,
        ),
        corner_size,
    );
    if !tiling.bottom && bottom_right_bounds.contains(&pos) {
        return Some(ResizeEdge::BottomRight);
    }

    if !tiling.top && pos.y < shadow_size {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && pos.y > window_size.height - shadow_size {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && pos.x < shadow_size {
        Some(ResizeEdge::Left)
    } else if !tiling.right && pos.x > window_size.width - shadow_size {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

fn join_pane_into_active(
    active_pane: &Entity<Pane>,
    pane: &Entity<Pane>,
    window: &mut Window,
    cx: &mut App,
) {
    if pane == active_pane {
    } else if pane.read(cx).items_len() == 0 {
        pane.update(cx, |_, cx| {
            cx.emit(pane::Event::Remove {
                focus_on_pane: None,
            });
        })
    } else {
        move_all_items(pane, active_pane, window, cx);
    }
}

fn move_all_items(
    from_pane: &Entity<Pane>,
    to_pane: &Entity<Pane>,
    window: &mut Window,
    cx: &mut App,
) {
    let destination_is_different = from_pane != to_pane;
    let mut moved_items = 0;
    for (item_ix, item_handle) in from_pane
        .read(cx)
        .items()
        .enumerate()
        .map(|(ix, item)| (ix, item.clone()))
        .collect::<Vec<_>>()
    {
        let ix = item_ix - moved_items;
        if destination_is_different {
            // Close item from previous pane
            from_pane.update(cx, |source, cx| {
                source.remove_item_and_focus_on_pane(ix, false, to_pane.clone(), window, cx);
            });
            moved_items += 1;
        }

        // This automatically removes duplicate items in the pane
        to_pane.update(cx, |destination, cx| {
            destination.add_item(item_handle, true, true, None, window, cx);
            window.focus(&destination.focus_handle(cx), cx)
        });
    }
}

pub fn move_item(
    source: &Entity<Pane>,
    destination: &Entity<Pane>,
    item_id_to_move: EntityId,
    destination_index: usize,
    activate: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some((item_ix, item_handle)) = source
        .read(cx)
        .items()
        .enumerate()
        .find(|(_, item_handle)| item_handle.item_id() == item_id_to_move)
        .map(|(ix, item)| (ix, item.clone()))
    else {
        // Tab was closed during drag
        return;
    };

    if source != destination {
        // Close item from previous pane
        source.update(cx, |source, cx| {
            source.remove_item_and_focus_on_pane(item_ix, false, destination.clone(), window, cx);
        });
    }

    // This automatically removes duplicate items in the pane
    destination.update(cx, |destination, cx| {
        destination.add_item_inner(
            item_handle,
            activate,
            activate,
            activate,
            Some(destination_index),
            window,
            cx,
        );
        if activate {
            window.focus(&destination.focus_handle(cx), cx)
        }
    });
}

pub fn move_active_item(
    source: &Entity<Pane>,
    destination: &Entity<Pane>,
    focus_destination: bool,
    close_if_empty: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if source == destination {
        return;
    }
    let Some(active_item) = source.read(cx).active_item() else {
        return;
    };
    source.update(cx, |source_pane, cx| {
        let item_id = active_item.item_id();
        source_pane.remove_item(item_id, false, close_if_empty, window, cx);
        destination.update(cx, |target_pane, cx| {
            target_pane.add_item(
                active_item,
                focus_destination,
                focus_destination,
                Some(target_pane.items_len()),
                window,
                cx,
            );
        });
    });
}

pub fn clone_active_item(
    workspace_id: Option<WorkspaceId>,
    source: &Entity<Pane>,
    destination: &Entity<Pane>,
    focus_destination: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if source == destination {
        return;
    }
    let Some(active_item) = source.read(cx).active_item() else {
        return;
    };
    if !active_item.can_split(cx) {
        return;
    }
    let destination = destination.downgrade();
    let task = active_item.clone_on_split(workspace_id, window, cx);
    window
        .spawn(cx, async move |cx| {
            let Some(clone) = task.await else {
                return;
            };
            destination
                .update_in(cx, |target_pane, window, cx| {
                    target_pane.add_item(
                        clone,
                        focus_destination,
                        focus_destination,
                        Some(target_pane.items_len()),
                        window,
                        cx,
                    );
                })
                .log_err();
        })
        .detach();
}

#[derive(Debug)]
pub struct WorkspacePosition {
    pub window_bounds: Option<WindowBounds>,
    pub display: Option<Uuid>,
    pub centered_layout: bool,
}

pub fn remote_workspace_position_from_db(
    connection_options: RemoteConnectionOptions,
    paths_to_open: &[PathBuf],
    cx: &App,
) -> Task<Result<WorkspacePosition>> {
    let paths = paths_to_open.to_vec();
    let db = WorkspaceDb::global(cx);
    let kvp = db::kvp::KeyValueStore::global(cx);

    cx.background_spawn(async move {
        let remote_connection_id = db
            .get_or_create_remote_connection(connection_options)
            .await
            .context("fetching serialized ssh project")?;
        let serialized_workspace = db.remote_workspace_for_roots(&paths, remote_connection_id);

        let (window_bounds, display) = if let Some(bounds) = window_bounds_env_override() {
            (Some(WindowBounds::Windowed(bounds)), None)
        } else {
            let restorable_bounds = serialized_workspace
                .as_ref()
                .and_then(|workspace| {
                    Some((workspace.display?, workspace.window_bounds.map(|b| b.0)?))
                })
                .or_else(|| persistence::read_default_window_bounds(&kvp));

            if let Some((serialized_display, serialized_bounds)) = restorable_bounds {
                (Some(serialized_bounds), Some(serialized_display))
            } else {
                (None, None)
            }
        };

        let centered_layout = serialized_workspace
            .as_ref()
            .map(|w| w.centered_layout)
            .unwrap_or(false);

        Ok(WorkspacePosition {
            window_bounds,
            display,
            centered_layout,
        })
    })
}

pub fn with_active_or_new_workspace(
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send + 'static,
) {
    match cx
        .active_window()
        .and_then(|w| w.downcast::<MultiWorkspace>())
    {
        Some(multi_workspace) => {
            cx.defer(move |cx| {
                multi_workspace
                    .update(cx, |multi_workspace, window, cx| {
                        let workspace = multi_workspace.workspace().clone();
                        workspace.update(cx, |workspace, cx| f(workspace, window, cx));
                    })
                    .log_err();
            });
        }
        None => {
            let app_state = AppState::global(cx);
            open_new(
                OpenOptions::default(),
                app_state,
                cx,
                move |workspace, window, cx| f(workspace, window, cx),
            )
            .detach_and_log_err(cx);
        }
    }
}

/// Reads a panel's pixel size from its legacy KVP format and deletes the legacy
/// key. This migration path only runs once per panel per workspace.
fn load_legacy_panel_size(
    panel_key: &str,
    dock_position: DockPosition,
    workspace: &Workspace,
    cx: &mut App,
) -> Option<Pixels> {
    #[derive(Deserialize)]
    struct LegacyPanelState {
        #[serde(default)]
        width: Option<Pixels>,
        #[serde(default)]
        height: Option<Pixels>,
    }

    let workspace_id = workspace
        .database_id()
        .map(|id| i64::from(id).to_string())
        .or_else(|| workspace.session_id())?;

    let legacy_key = match panel_key {
        "ProjectPanel" => {
            format!("{}-{:?}", "ProjectPanel", workspace_id)
        }
        "OutlinePanel" => {
            format!("{}-{:?}", "OutlinePanel", workspace_id)
        }
        "GitPanel" => {
            format!("{}-{:?}", "GitPanel", workspace_id)
        }
        "TerminalPanel" => {
            format!("{:?}-{:?}", "TerminalPanel", workspace_id)
        }
        _ => return None,
    };

    let kvp = db::kvp::KeyValueStore::global(cx);
    let json = kvp.read_kvp(&legacy_key).log_err().flatten()?;
    let state = serde_json::from_str::<LegacyPanelState>(&json).log_err()?;
    let size = match dock_position {
        DockPosition::Bottom => state.height,
        DockPosition::Left | DockPosition::Right => state.width,
    }?;

    cx.background_spawn(async move { kvp.delete_kvp(legacy_key).await })
        .detach_and_log_err(cx);

    Some(size)
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::{Arc, atomic::Ordering},
        time::Duration,
    };

    use super::*;
    use crate::{
        dock::{
            PanelEvent, set_component_hidden, set_component_pinned, test::TestPanel,
            test::TestPanelB, test::TestPanelC,
        },
        item::{
            ItemBufferKind, ItemEvent,
            test::{TestItem, TestProjectItem},
        },
    };
    use fs::FakeFs;
    use gpui::{
        DismissEvent, Empty, EventEmitter, FocusHandle, Focusable, Render, TestAppContext,
        UpdateGlobal, VisualTestContext, px,
    };
    use project::{Project, ProjectEntryId, WorktreeId};
    use serde_json::json;
    use settings::SettingsStore;
    use util::path;
    use util::rel_path::rel_path;

    #[gpui::test]
    async fn test_tab_disambiguation(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        // Adding an item with no ambiguity renders the tab without detail.
        let item1 = cx.new(|cx| {
            let mut item = TestItem::new(cx);
            item.tab_descriptions = Some(vec!["c", "b1/c", "a/b1/c"]);
            item
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item1.clone()), None, true, window, cx);
        });
        item1.read_with(cx, |item, _| assert_eq!(item.tab_detail.get(), Some(0)));

        // Adding an item that creates ambiguity increases the level of detail on
        // both tabs.
        let item2 = cx.new_window_entity(|_window, cx| {
            let mut item = TestItem::new(cx);
            item.tab_descriptions = Some(vec!["c", "b2/c", "a/b2/c"]);
            item
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item2.clone()), None, true, window, cx);
        });
        item1.read_with(cx, |item, _| assert_eq!(item.tab_detail.get(), Some(1)));
        item2.read_with(cx, |item, _| assert_eq!(item.tab_detail.get(), Some(1)));

        // Adding an item that creates ambiguity increases the level of detail only
        // on the ambiguous tabs. In this case, the ambiguity can't be resolved so
        // we stop at the highest detail available.
        let item3 = cx.new(|cx| {
            let mut item = TestItem::new(cx);
            item.tab_descriptions = Some(vec!["c", "b2/c", "a/b2/c"]);
            item
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item3.clone()), None, true, window, cx);
        });
        item1.read_with(cx, |item, _| assert_eq!(item.tab_detail.get(), Some(1)));
        item2.read_with(cx, |item, _| assert_eq!(item.tab_detail.get(), Some(3)));
        item3.read_with(cx, |item, _| assert_eq!(item.tab_detail.get(), Some(3)));
    }

    #[gpui::test]
    async fn test_tracking_active_path(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            "/root1",
            json!({
                "one.txt": "",
                "two.txt": "",
            }),
        )
        .await;
        fs.insert_tree(
            "/root2",
            json!({
                "three.txt": "",
            }),
        )
        .await;

        let project = Project::test(fs, ["root1".as_ref()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        let worktree_id = project.update(cx, |project, cx| {
            project.worktrees(cx).next().unwrap().read(cx).id()
        });

        let item1 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "one.txt", cx)])
        });
        let item2 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(2, "two.txt", cx)])
        });

        // Add an item to an empty pane
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item1), None, true, window, cx)
        });
        project.update(cx, |project, cx| {
            assert_eq!(
                project.active_entry(),
                project
                    .entry_for_path(&(worktree_id, rel_path("one.txt")).into(), cx)
                    .map(|e| e.id)
            );
        });
        assert_eq!(cx.window_title().as_deref(), Some("root1 — one.txt"));

        // Add a second item to a non-empty pane
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item2), None, true, window, cx)
        });
        assert_eq!(cx.window_title().as_deref(), Some("root1 — two.txt"));
        project.update(cx, |project, cx| {
            assert_eq!(
                project.active_entry(),
                project
                    .entry_for_path(&(worktree_id, rel_path("two.txt")).into(), cx)
                    .map(|e| e.id)
            );
        });

        // Close the active item
        pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(&Default::default(), window, cx)
        })
        .await
        .unwrap();
        assert_eq!(cx.window_title().as_deref(), Some("root1 — one.txt"));
        project.update(cx, |project, cx| {
            assert_eq!(
                project.active_entry(),
                project
                    .entry_for_path(&(worktree_id, rel_path("one.txt")).into(), cx)
                    .map(|e| e.id)
            );
        });

        // Add a project folder
        project
            .update(cx, |project, cx| {
                project.find_or_create_worktree("root2", true, cx)
            })
            .await
            .unwrap();
        assert_eq!(cx.window_title().as_deref(), Some("root1, root2 — one.txt"));

        // Remove a project folder
        project.update(cx, |project, cx| project.remove_worktree(worktree_id, cx));
        assert_eq!(cx.window_title().as_deref(), Some("root2 — one.txt"));
    }

    #[gpui::test]
    async fn test_document_path_updates_with_active_item(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            "/root",
            json!({
                "one.txt": "",
                "two.txt": "",
            }),
        )
        .await;

        let project = Project::test(fs, ["root".as_ref()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        let worktree_id = project.update(cx, |project, cx| {
            project.worktrees(cx).next().unwrap().read(cx).id()
        });

        let item1 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[new_test_project_item(
                1,
                "one.txt",
                worktree_id,
                cx,
            )])
        });
        let item2 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[new_test_project_item(
                2,
                "two.txt",
                worktree_id,
                cx,
            )])
        });

        // Initially no document path
        assert_eq!(cx.document_path(), None);

        // Add an item - document path should be set
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item1), None, true, window, cx)
        });
        assert_eq!(
            cx.document_path(),
            Some(std::path::PathBuf::from("root/one.txt"))
        );

        // Add a second item - document path should update
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item2), None, true, window, cx)
        });
        assert_eq!(
            cx.document_path(),
            Some(std::path::PathBuf::from("root/two.txt"))
        );

        // Close the active item - document path should revert to first item
        pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(&Default::default(), window, cx)
        })
        .await
        .unwrap();
        assert_eq!(
            cx.document_path(),
            Some(std::path::PathBuf::from("root/one.txt"))
        );

        // Close all items - document path should be cleared
        pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(&Default::default(), window, cx)
        })
        .await
        .unwrap();
        assert_eq!(cx.document_path(), None);
    }

    #[gpui::test]
    async fn test_close_window(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/root", json!({ "one": "" })).await;

        let project = Project::test(fs, ["root".as_ref()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        // When there are no dirty items, there's nothing to do.
        let item1 = cx.new(TestItem::new);
        workspace.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item1.clone()), None, true, window, cx)
        });
        let task = workspace.update_in(cx, |w, window, cx| {
            w.prepare_to_close(CloseIntent::CloseWindow, window, cx)
        });
        assert!(task.await.unwrap());

        // When there are dirty untitled items, prompt to save each one. If the user
        // cancels any prompt, then abort.
        let item2 = cx.new(|cx| TestItem::new(cx).with_dirty(true));
        let item3 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_project_items(&[TestProjectItem::new(1, "1.txt", cx)])
        });
        workspace.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item2.clone()), None, true, window, cx);
            w.add_item_to_active_pane(Box::new(item3.clone()), None, true, window, cx);
        });
        let task = workspace.update_in(cx, |w, window, cx| {
            w.prepare_to_close(CloseIntent::CloseWindow, window, cx)
        });
        cx.executor().run_until_parked();
        cx.simulate_prompt_answer("Cancel"); // cancel save all
        cx.executor().run_until_parked();
        assert!(!cx.has_pending_prompt());
        assert!(!task.await.unwrap());
    }

    #[gpui::test]
    async fn test_multi_workspace_close_window_multiple_workspaces_cancel(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/root", json!({ "one": "" })).await;

        let project_a = Project::test(fs.clone(), ["root".as_ref()], cx).await;
        let project_b = Project::test(fs, ["root".as_ref()], cx).await;
        let multi_workspace_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));
        cx.run_until_parked();

        multi_workspace_handle
            .update(cx, |mw, _window, cx| {
                mw.open_sidebar(cx);
            })
            .unwrap();

        let workspace_a = multi_workspace_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let workspace_b = multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.test_add_workspace(project_b, window, cx)
            })
            .unwrap();

        // Activate workspace A
        multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.activate(workspace_a.clone(), None, window, cx);
            })
            .unwrap();

        let cx = &mut VisualTestContext::from_window(multi_workspace_handle.into(), cx);

        // Workspace A has a clean item
        let item_a = cx.new(TestItem::new);
        workspace_a.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item_a.clone()), None, true, window, cx)
        });

        // Workspace B has a dirty item
        let item_b = cx.new(|cx| TestItem::new(cx).with_dirty(true));
        workspace_b.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item_b.clone()), None, true, window, cx)
        });

        // Verify workspace A is active
        multi_workspace_handle
            .read_with(cx, |mw, _| {
                assert_eq!(mw.workspace(), &workspace_a);
            })
            .unwrap();

        // Dispatch CloseWindow — workspace A will pass, workspace B will prompt
        multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.close_window(&CloseWindow, window, cx);
            })
            .unwrap();
        cx.run_until_parked();

        // Workspace B should now be active since it has dirty items that need attention
        multi_workspace_handle
            .read_with(cx, |mw, _| {
                assert_eq!(
                    mw.workspace(),
                    &workspace_b,
                    "workspace B should be activated when it prompts"
                );
            })
            .unwrap();

        // User cancels the save prompt from workspace B
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();

        // Window should still exist because workspace B's close was cancelled
        assert!(
            multi_workspace_handle.update(cx, |_, _, _| ()).is_ok(),
            "window should still exist after cancelling one workspace's close"
        );
    }

    #[gpui::test]
    async fn test_remove_workspace_prompts_for_unsaved_changes(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/root", json!({ "one": "" })).await;

        let project_a = Project::test(fs.clone(), ["root".as_ref()], cx).await;
        let project_b = Project::test(fs.clone(), ["root".as_ref()], cx).await;
        let multi_workspace_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));
        cx.run_until_parked();

        multi_workspace_handle
            .update(cx, |mw, _window, cx| mw.open_sidebar(cx))
            .unwrap();

        let workspace_a = multi_workspace_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let workspace_b = multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.test_add_workspace(project_b, window, cx)
            })
            .unwrap();

        // Activate workspace A.
        multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.activate(workspace_a.clone(), None, window, cx);
            })
            .unwrap();

        let cx = &mut VisualTestContext::from_window(multi_workspace_handle.into(), cx);

        // Workspace B has a dirty item.
        let item_b = cx.new(|cx| TestItem::new(cx).with_dirty(true));
        workspace_b.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item_b.clone()), None, true, window, cx)
        });

        // Try to remove workspace B. It should prompt because of the dirty item.
        let remove_task = multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.remove(
                    [workspace_b.clone()],
                    RemovalIntent::CloseProject,
                    window,
                    cx,
                )
            })
            .unwrap();
        cx.run_until_parked();

        // The prompt should have activated workspace B.
        multi_workspace_handle
            .read_with(cx, |mw, _| {
                assert_eq!(
                    mw.workspace(),
                    &workspace_b,
                    "workspace B should be active while prompting"
                );
            })
            .unwrap();

        // Cancel the prompt — user stays on workspace B.
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        let removed = remove_task.await.unwrap();
        assert!(!removed, "removal should have been cancelled");

        multi_workspace_handle
            .read_with(cx, |mw, _cx| {
                assert_eq!(
                    mw.workspace(),
                    &workspace_b,
                    "user should stay on workspace B after cancelling"
                );
                assert_eq!(mw.workspaces().count(), 2, "both workspaces should remain");
            })
            .unwrap();

        // Try again. This time accept the prompt.
        let remove_task = multi_workspace_handle
            .update(cx, |mw, window, cx| {
                // First switch back to A.
                mw.activate(workspace_a.clone(), None, window, cx);
                mw.remove(
                    [workspace_b.clone()],
                    RemovalIntent::CloseProject,
                    window,
                    cx,
                )
            })
            .unwrap();
        cx.run_until_parked();

        // Accept the save prompt.
        cx.simulate_prompt_answer("Don't Save");
        cx.run_until_parked();
        let removed = remove_task.await.unwrap();
        assert!(removed, "removal should have succeeded");

        // Should be back on workspace A, and B should be gone.
        multi_workspace_handle
            .read_with(cx, |mw, _cx| {
                assert_eq!(
                    mw.workspace(),
                    &workspace_a,
                    "should be back on workspace A after removing B"
                );
                assert_eq!(mw.workspaces().count(), 1, "only workspace A should remain");
            })
            .unwrap();
    }

    #[gpui::test]
    async fn test_close_window_with_worktrees_hot_exits(cx: &mut TestAppContext) {
        init_test(cx);

        // Register TestItem as a serializable item
        cx.update(|cx| {
            register_serializable_item::<TestItem>(cx);
        });

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/root", json!({ "one": "" })).await;

        let project = Project::test(fs, ["root".as_ref()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        // When there are dirty untitled items, but they can serialize, then there is no prompt.
        let item1 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_serialize(|| Some(Task::ready(Ok(()))))
        });
        let item2 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_project_items(&[TestProjectItem::new(1, "1.txt", cx)])
                .with_serialize(|| Some(Task::ready(Ok(()))))
        });
        workspace.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item1.clone()), None, true, window, cx);
            w.add_item_to_active_pane(Box::new(item2.clone()), None, true, window, cx);
        });
        let task = workspace.update_in(cx, |w, window, cx| {
            w.prepare_to_close(CloseIntent::CloseWindow, window, cx)
        });
        assert!(task.await.unwrap());
    }

    // See https://github.com/zed-industries/zed/issues/55726.
    //
    // macOS only: on Linux/Windows, closing the last window sets
    // `save_last_workspace`, which preserves the session (same as `Quit`),
    // so hot-exit is safe there.
    #[cfg(target_os = "macos")]
    #[gpui::test]
    async fn test_close_window_without_worktrees_prompts(cx: &mut TestAppContext) {
        init_test(cx);

        cx.update(|cx| {
            register_serializable_item::<TestItem>(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let item = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_serialize(|| Some(Task::ready(Ok(()))))
        });
        workspace.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });

        let task = workspace.update_in(cx, |w, window, cx| {
            w.prepare_to_close(CloseIntent::CloseWindow, window, cx)
        });
        cx.executor().run_until_parked();

        assert!(
            cx.has_pending_prompt(),
            "closing a no-folder workspace with a dirty serializable item should prompt, \
             since the workspace will not be reachable after close"
        );
        cx.simulate_prompt_answer("Don't Save");
        cx.executor().run_until_parked();

        assert!(task.await.unwrap());
    }

    #[gpui::test]
    async fn test_quit_without_worktrees_hot_exits(cx: &mut TestAppContext) {
        init_test(cx);

        cx.update(|cx| {
            register_serializable_item::<TestItem>(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let item = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_serialize(|| Some(Task::ready(Ok(()))))
        });
        workspace.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });

        let task = workspace.update_in(cx, |w, window, cx| {
            w.prepare_to_close(CloseIntent::Quit, window, cx)
        });
        cx.executor().run_until_parked();

        assert!(
            !cx.has_pending_prompt(),
            "quitting should hot-exit silently; the session restore on next \
             launch will bring the dirty buffer back"
        );
        assert!(task.await.unwrap());
    }

    // See https://github.com/zed-industries/zed/issues/55726.
    #[gpui::test]
    async fn test_replace_window_without_worktrees_prompts(cx: &mut TestAppContext) {
        init_test(cx);

        cx.update(|cx| {
            register_serializable_item::<TestItem>(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let item = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_serialize(|| Some(Task::ready(Ok(()))))
        });
        workspace.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });

        let task = workspace.update_in(cx, |w, window, cx| {
            w.prepare_to_close(CloseIntent::ReplaceWindow, window, cx)
        });
        cx.executor().run_until_parked();

        assert!(
            cx.has_pending_prompt(),
            "replacing a workspace with a dirty serializable item should prompt, \
             since the workspace will be detached afterwards"
        );
        cx.simulate_prompt_answer("Don't Save");
        cx.executor().run_until_parked();

        assert!(task.await.unwrap());
    }

    #[gpui::test]
    async fn test_replace_window_with_worktrees_hot_exits(cx: &mut TestAppContext) {
        init_test(cx);

        cx.update(|cx| {
            register_serializable_item::<TestItem>(cx);
        });

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/root", json!({ "one": "" })).await;

        let project = Project::test(fs, ["root".as_ref()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let item = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_serialize(|| Some(Task::ready(Ok(()))))
        });
        workspace.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });

        let task = workspace.update_in(cx, |w, window, cx| {
            w.prepare_to_close(CloseIntent::ReplaceWindow, window, cx)
        });
        cx.executor().run_until_parked();

        assert!(
            !cx.has_pending_prompt(),
            "replacing a workspace with folder paths should hot-exit silently; \
             the buffer is recoverable by reopening the project"
        );
        assert!(task.await.unwrap());
    }

    #[gpui::test]
    async fn test_close_window_with_failing_serialize_prompts(cx: &mut TestAppContext) {
        init_test(cx);

        cx.update(|cx| {
            register_serializable_item::<TestItem>(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let item = cx.new(|cx| {
            TestItem::new(cx).with_dirty(true).with_serialize(|| {
                Some(Task::ready(Err(anyhow::anyhow!(
                    "FOREIGN KEY constraint failed"
                ))))
            })
        });
        workspace.update_in(cx, |w, window, cx| {
            w.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });

        let task = workspace.update_in(cx, |w, window, cx| {
            w.prepare_to_close(CloseIntent::CloseWindow, window, cx)
        });
        cx.executor().run_until_parked();

        // The failing serialization must not short-circuit the close; a
        // save/discard prompt must be shown for the dirty scratch item.
        assert!(
            cx.has_pending_prompt(),
            "a save/discard prompt should be shown for the dirty scratch item \
             when its serialization fails"
        );
        cx.simulate_prompt_answer("Don't Save");
        cx.executor().run_until_parked();

        // Preparing to close succeeds, even though serialization failed.
        assert!(task.await.unwrap());
    }

    #[gpui::test]
    async fn test_exact_checkpoint_forms_ordered_serializer_cut(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(register_serializable_item::<TestItem>);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        workspace.update(cx, |workspace, _| workspace.set_random_database_id());

        let trace = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let timeline_active = Rc::new(Cell::new(false));
        let timeline_invocation = Rc::new(Cell::new(0));
        let pre_receiver: Rc<RefCell<Option<oneshot::Receiver<()>>>> = Rc::new(RefCell::new(None));
        let post_receiver: Rc<RefCell<Option<oneshot::Receiver<()>>>> = Rc::new(RefCell::new(None));
        let executor = cx.background_executor.clone();
        let item = cx.new({
            let trace = trace.clone();
            let timeline_active = timeline_active.clone();
            let timeline_invocation = timeline_invocation.clone();
            let pre_receiver = pre_receiver.clone();
            let post_receiver = post_receiver.clone();
            move |cx| {
                TestItem::new(cx).with_serialize(move || {
                    if !timeline_active.get() {
                        return Some(Task::ready(Ok(())));
                    }

                    let invocation = timeline_invocation.get();
                    timeline_invocation.set(invocation + 1);
                    match invocation {
                        0 => {
                            let receiver = pre_receiver.borrow_mut().take();
                            let trace = trace.clone();
                            Some(executor.spawn(async move {
                                trace.lock().push("pre-started");
                                receiver
                                    .context("pre-barrier gate was not installed")?
                                    .await
                                    .context("pre-barrier gate was dropped")?;
                                trace.lock().push("pre-finished");
                                Ok(())
                            }))
                        }
                        1 => {
                            trace.lock().push("exact");
                            Some(Task::ready(Ok(())))
                        }
                        2 => {
                            let receiver = post_receiver.borrow_mut().take();
                            let trace = trace.clone();
                            Some(executor.spawn(async move {
                                trace.lock().push("post-started");
                                receiver
                                    .context("post-barrier gate was not installed")?
                                    .await
                                    .context("post-barrier gate was dropped")?;
                                trace.lock().push("post-finished");
                                Ok(())
                            }))
                        }
                        _ => Some(Task::ready(Err(anyhow!(
                            "unexpected serializer invocation after exact barrier"
                        )))),
                    }
                })
            }
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });

        let prime = workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx));
        if let Err(error) = prime.await {
            panic!("failed to prime exact serialization: {error:#}");
        }
        cx.executor().advance_clock(SERIALIZATION_THROTTLE_TIME);
        cx.run_until_parked();

        let (pre_release, pre_gate) = oneshot::channel();
        let (post_release, post_gate) = oneshot::channel();
        *pre_receiver.borrow_mut() = Some(pre_gate);
        *post_receiver.borrow_mut() = Some(post_gate);
        timeline_invocation.set(0);
        timeline_active.set(true);

        let checkpoint = workspace.update(cx, |workspace, cx| {
            if let Err(error) = workspace.enqueue_item_serialization(Box::new(item.clone())) {
                panic!("failed to enqueue pre-barrier serialization: {error:#}");
            }
            let checkpoint = workspace.checkpoint_exact(cx);
            if let Err(error) = workspace.enqueue_item_serialization(Box::new(item.clone())) {
                panic!("failed to enqueue post-barrier serialization: {error:#}");
            }
            checkpoint
        });
        cx.run_until_parked();

        assert_eq!(trace.lock().as_slice(), ["pre-started"]);
        assert!(!checkpoint.is_ready());
        assert!(pre_release.send(()).is_ok());
        cx.run_until_parked();

        assert_eq!(
            trace.lock().as_slice(),
            ["pre-started", "pre-finished", "exact", "post-started"]
        );
        assert!(checkpoint.is_ready());
        let database_id = match workspace.read_with(cx, |workspace, _| workspace.database_id()) {
            Some(database_id) => database_id,
            None => panic!("test workspace lost its database id"),
        };
        let db = cx.update(|_, cx| WorkspaceDb::global(cx));
        assert!(db.workspace_for_id(database_id).is_some());

        assert!(post_release.send(()).is_ok());
        cx.run_until_parked();
        if let Err(error) = checkpoint.await {
            panic!("exact checkpoint failed: {error:#}");
        }
        assert_eq!(
            trace.lock().as_slice(),
            [
                "pre-started",
                "pre-finished",
                "exact",
                "post-started",
                "post-finished"
            ]
        );
    }

    #[gpui::test]
    async fn test_exact_checkpoint_serializes_every_open_item(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(register_serializable_item::<TestItem>);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        workspace.update(cx, |workspace, _| workspace.set_random_database_id());

        let counters = (0..3).map(|_| Rc::new(Cell::new(0))).collect::<Vec<_>>();
        let items = counters
            .iter()
            .map(|counter| {
                cx.new({
                    let counter = counter.clone();
                    move |cx| {
                        TestItem::new(cx).with_serialize(move || {
                            counter.set(counter.get() + 1);
                            Some(Task::ready(Ok(())))
                        })
                    }
                })
            })
            .collect::<Vec<_>>();
        workspace.update_in(cx, |workspace, window, cx| {
            for item in &items {
                workspace.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
            }
        });

        let prime = workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx));
        if let Err(error) = prime.await {
            panic!("failed to prime exact serialization: {error:#}");
        }
        cx.executor().advance_clock(SERIALIZATION_THROTTLE_TIME);
        cx.run_until_parked();
        let baselines = counters
            .iter()
            .map(|counter| counter.get())
            .collect::<Vec<_>>();

        let checkpoint = workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx));
        if let Err(error) = checkpoint.await {
            panic!("exact checkpoint failed: {error:#}");
        }
        for (counter, baseline) in counters.iter().zip(baselines) {
            assert_eq!(counter.get(), baseline + 1);
        }
    }

    #[gpui::test]
    async fn test_exact_checkpoint_reports_async_item_failure(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(register_serializable_item::<TestItem>);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        workspace.update(cx, |workspace, _| {
            workspace.set_random_database_id();
            workspace.centered_layout = false;
        });

        let failure_active = Rc::new(Cell::new(false));
        let failure_receiver: Rc<RefCell<Option<oneshot::Receiver<()>>>> =
            Rc::new(RefCell::new(None));
        let failure_started = Arc::new(AtomicBool::new(false));
        let executor = cx.background_executor.clone();
        let item = cx.new({
            let failure_active = failure_active.clone();
            let failure_receiver = failure_receiver.clone();
            let failure_started = failure_started.clone();
            move |cx| {
                TestItem::new(cx).with_serialize(move || {
                    if !failure_active.get() {
                        return Some(Task::ready(Ok(())));
                    }
                    let receiver = failure_receiver.borrow_mut().take();
                    let failure_started = failure_started.clone();
                    Some(executor.spawn(async move {
                        failure_started.store(true, Ordering::SeqCst);
                        receiver
                            .context("item-failure gate was not installed")?
                            .await
                            .context("item-failure gate was dropped")?;
                        Err(anyhow!("exact item checkpoint witness failure"))
                    }))
                })
            }
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
        });

        let prime = workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx));
        if let Err(error) = prime.await {
            panic!("failed to prime exact serialization: {error:#}");
        }
        cx.executor().advance_clock(SERIALIZATION_THROTTLE_TIME);
        cx.run_until_parked();

        workspace.update(cx, |workspace, _| {
            workspace.centered_layout = true;
        });
        let (failure_release, failure_gate) = oneshot::channel();
        *failure_receiver.borrow_mut() = Some(failure_gate);
        failure_active.set(true);
        let checkpoint = workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx));
        cx.run_until_parked();
        assert!(failure_started.load(Ordering::SeqCst));
        assert!(!checkpoint.is_ready());
        assert!(failure_release.send(()).is_ok());

        let error = match checkpoint.await {
            Ok(()) => panic!("exact checkpoint accepted a failed item write"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("exact item checkpoint witness failure"));
        let database_id = match workspace.read_with(cx, |workspace, _| workspace.database_id()) {
            Some(database_id) => database_id,
            None => panic!("test workspace lost its database id"),
        };
        let db = cx.update(|_, cx| WorkspaceDb::global(cx));
        let stored = match db.workspace_for_id(database_id) {
            Some(stored) => stored,
            None => panic!("primed workspace row disappeared"),
        };
        assert!(!stored.centered_layout);

        failure_active.set(false);
        cx.executor().advance_clock(SERIALIZATION_THROTTLE_TIME);
        cx.run_until_parked();
        let retry = workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx));
        if let Err(error) = retry.await {
            panic!("exact checkpoint coordinator did not recover: {error:#}");
        }
        let stored = match db.workspace_for_id(database_id) {
            Some(stored) => stored,
            None => panic!("retried workspace row disappeared"),
        };
        assert!(stored.centered_layout);
    }

    #[gpui::test]
    async fn test_exact_checkpoint_reports_workspace_row_failure(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        workspace.update(cx, |workspace, _| {
            workspace.set_random_database_id();
            workspace.centered_layout = false;
        });

        let prime = workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx));
        if let Err(error) = prime.await {
            panic!("failed to prime exact serialization: {error:#}");
        }
        cx.executor().advance_clock(SERIALIZATION_THROTTLE_TIME);
        cx.run_until_parked();

        let database_id = match workspace.read_with(cx, |workspace, _| workspace.database_id()) {
            Some(database_id) => database_id,
            None => panic!("test workspace lost its database id"),
        };
        let db = cx.update(|_, cx| WorkspaceDb::global(cx));
        workspace.update(cx, |workspace, _| {
            workspace.centered_layout = true;
        });
        WorkspaceDb::fail_next_workspace_write_for_tests(database_id);
        let checkpoint = workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx));
        let result = checkpoint.await;

        assert!(result.is_err());
        let stored = match db.workspace_for_id(database_id) {
            Some(stored) => stored,
            None => panic!("primed workspace row disappeared"),
        };
        assert!(!stored.centered_layout);

        cx.executor().advance_clock(SERIALIZATION_THROTTLE_TIME);
        cx.run_until_parked();
        let retry = workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx));
        if let Err(error) = retry.await {
            panic!("exact checkpoint coordinator did not recover: {error:#}");
        }
        let stored = match db.workspace_for_id(database_id) {
            Some(stored) => stored,
            None => panic!("retried workspace row disappeared"),
        };
        assert!(stored.centered_layout);
    }

    #[gpui::test]
    async fn test_close_pane_items(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let item1 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_project_items(&[dirty_project_item(1, "1.txt", cx)])
        });
        let item2 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_conflict(true)
                .with_project_items(&[dirty_project_item(2, "2.txt", cx)])
        });
        let item3 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_conflict(true)
                .with_project_items(&[dirty_project_item(3, "3.txt", cx)])
        });
        let item4 = cx.new(|cx| {
            TestItem::new(cx).with_dirty(true).with_project_items(&[{
                let project_item = TestProjectItem::new_untitled(cx);
                project_item.update(cx, |project_item, _| project_item.is_dirty = true);
                project_item
            }])
        });
        let pane = workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item1.clone()), None, true, window, cx);
            workspace.add_item_to_active_pane(Box::new(item2.clone()), None, true, window, cx);
            workspace.add_item_to_active_pane(Box::new(item3.clone()), None, true, window, cx);
            workspace.add_item_to_active_pane(Box::new(item4.clone()), None, true, window, cx);
            workspace.active_pane().clone()
        });

        let close_items = pane.update_in(cx, |pane, window, cx| {
            pane.activate_item(1, true, true, window, cx);
            assert_eq!(pane.active_item().unwrap().item_id(), item2.item_id());
            let item1_id = item1.item_id();
            let item3_id = item3.item_id();
            let item4_id = item4.item_id();
            pane.close_items(window, cx, SaveIntent::Close, &move |id| {
                [item1_id, item3_id, item4_id].contains(&id)
            })
        });
        cx.executor().run_until_parked();

        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Save all");

        cx.executor().run_until_parked();

        // Item 1 is saved. There's a prompt to save item 3.
        pane.update(cx, |pane, cx| {
            assert_eq!(item1.read(cx).save_count, 1);
            assert_eq!(item1.read(cx).save_as_count, 0);
            assert_eq!(item1.read(cx).reload_count, 0);
            assert_eq!(pane.items_len(), 3);
            assert_eq!(pane.active_item().unwrap().item_id(), item3.item_id());
        });
        assert!(cx.has_pending_prompt());

        // Cancel saving item 3.
        cx.simulate_prompt_answer("Discard Edits");
        cx.executor().run_until_parked();

        // Item 3 is reloaded. There's a prompt to save item 4.
        pane.update(cx, |pane, cx| {
            assert_eq!(item3.read(cx).save_count, 0);
            assert_eq!(item3.read(cx).save_as_count, 0);
            assert_eq!(item3.read(cx).reload_count, 1);
            assert_eq!(pane.items_len(), 2);
            assert_eq!(pane.active_item().unwrap().item_id(), item4.item_id());
        });

        // There's a prompt for a path for item 4.
        cx.simulate_new_path_selection(|_| Some(Default::default()));
        close_items.await.unwrap();

        // The requested items are closed.
        pane.update(cx, |pane, cx| {
            assert_eq!(item4.read(cx).save_count, 1);
            assert_eq!(item4.read(cx).save_as_count, 1);
            assert_eq!(item4.read(cx).reload_count, 0);
            assert_eq!(pane.items_len(), 1);
            assert_eq!(pane.active_item().unwrap().item_id(), item2.item_id());
        });
    }

    #[gpui::test]
    async fn test_prompting_to_save_only_on_last_item_for_entry(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // Create several workspace items with single project entries, and two
        // workspace items with multiple project entries.
        let single_entry_items = (0..=4)
            .map(|project_entry_id| {
                cx.new(|cx| {
                    TestItem::new(cx)
                        .with_dirty(true)
                        .with_project_items(&[dirty_project_item(
                            project_entry_id,
                            &format!("{project_entry_id}.txt"),
                            cx,
                        )])
                })
            })
            .collect::<Vec<_>>();
        let item_2_3 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_buffer_kind(ItemBufferKind::Multibuffer)
                .with_project_items(&[
                    single_entry_items[2].read(cx).project_items[0].clone(),
                    single_entry_items[3].read(cx).project_items[0].clone(),
                ])
        });
        let item_3_4 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_buffer_kind(ItemBufferKind::Multibuffer)
                .with_project_items(&[
                    single_entry_items[3].read(cx).project_items[0].clone(),
                    single_entry_items[4].read(cx).project_items[0].clone(),
                ])
        });

        // Create two panes that contain the following project entries:
        //   left pane:
        //     multi-entry items:   (2, 3)
        //     single-entry items:  0, 2, 3, 4
        //   right pane:
        //     single-entry items:  4, 1
        //     multi-entry items:   (3, 4)
        let (left_pane, right_pane) = workspace.update_in(cx, |workspace, window, cx| {
            let left_pane = workspace.active_pane().clone();
            workspace.add_item_to_active_pane(Box::new(item_2_3.clone()), None, true, window, cx);
            workspace.add_item_to_active_pane(
                single_entry_items[0].boxed_clone(),
                None,
                true,
                window,
                cx,
            );
            workspace.add_item_to_active_pane(
                single_entry_items[2].boxed_clone(),
                None,
                true,
                window,
                cx,
            );
            workspace.add_item_to_active_pane(
                single_entry_items[3].boxed_clone(),
                None,
                true,
                window,
                cx,
            );
            workspace.add_item_to_active_pane(
                single_entry_items[4].boxed_clone(),
                None,
                true,
                window,
                cx,
            );

            let right_pane =
                workspace.split_and_clone(left_pane.clone(), SplitDirection::Right, window, cx);

            let boxed_clone = single_entry_items[1].boxed_clone();
            let right_pane = window.spawn(cx, async move |cx| {
                right_pane.await.inspect(|right_pane| {
                    right_pane
                        .update_in(cx, |pane, window, cx| {
                            pane.add_item(boxed_clone, true, true, None, window, cx);
                            pane.add_item(Box::new(item_3_4.clone()), true, true, None, window, cx);
                        })
                        .unwrap();
                })
            });

            (left_pane, right_pane)
        });
        let right_pane = right_pane.await.unwrap();
        cx.focus(&right_pane);

        let close = right_pane.update_in(cx, |pane, window, cx| {
            pane.close_all_items(&CloseAllItems::default(), window, cx)
                .unwrap()
        });
        cx.executor().run_until_parked();

        let msg = cx.pending_prompt().unwrap().0;
        assert!(msg.contains("1.txt"));
        assert!(!msg.contains("2.txt"));
        assert!(!msg.contains("3.txt"));
        assert!(!msg.contains("4.txt"));

        // With best-effort close, cancelling item 1 keeps it open but items 4
        // and (3,4) still close since their entries exist in left pane.
        cx.simulate_prompt_answer("Cancel");
        close.await;

        right_pane.read_with(cx, |pane, _| {
            assert_eq!(pane.items_len(), 1);
        });

        // Remove item 3 from left pane, making (2,3) the only item with entry 3.
        left_pane
            .update_in(cx, |left_pane, window, cx| {
                left_pane.close_item_by_id(
                    single_entry_items[3].entity_id(),
                    SaveIntent::Skip,
                    window,
                    cx,
                )
            })
            .await
            .unwrap();

        let close = left_pane.update_in(cx, |pane, window, cx| {
            pane.close_all_items(&CloseAllItems::default(), window, cx)
                .unwrap()
        });
        cx.executor().run_until_parked();

        let details = cx.pending_prompt().unwrap().1;
        assert!(details.contains("0.txt"));
        assert!(details.contains("3.txt"));
        assert!(details.contains("4.txt"));
        // Ideally 2.txt wouldn't appear since entry 2 still exists in item 2.
        // But we can only save whole items, so saving (2,3) for entry 3 includes 2.
        // assert!(!details.contains("2.txt"));

        cx.simulate_prompt_answer("Save all");
        cx.executor().run_until_parked();
        close.await;

        left_pane.read_with(cx, |pane, _| {
            assert_eq!(pane.items_len(), 0);
        });
    }

    #[gpui::test]
    async fn test_autosave(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        let item = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "1.txt", cx)])
        });
        let item_id = item.entity_id();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });

        // Autosave on window change.
        item.update(cx, |item, cx| {
            SettingsStore::update_global(cx, |settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.workspace.autosave = Some(AutosaveSetting::OnWindowChange);
                })
            });
            item.is_dirty = true;
        });

        // Deactivating the window saves the file.
        cx.deactivate_window();
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 1));

        // Re-activating the window doesn't save the file.
        cx.update(|window, _| window.activate_window());
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 1));

        // Autosave on focus change.
        item.update_in(cx, |item, window, cx| {
            cx.focus_self(window);
            SettingsStore::update_global(cx, |settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.workspace.autosave = Some(AutosaveSetting::OnFocusChange);
                })
            });
            item.is_dirty = true;
        });
        // Focus leaving the item (via window deactivation) saves the file.
        // Deferred autosaves are flushed when focus lands elsewhere (pane, panel)
        // or when the window is deactivated.
        cx.deactivate_window();
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 2));
        cx.update(|window, _| window.activate_window());

        // Deactivating the window still saves the file.
        item.update_in(cx, |item, window, cx| {
            cx.focus_self(window);
            item.is_dirty = true;
        });
        cx.deactivate_window();
        item.update(cx, |item, _| assert_eq!(item.save_count, 3));

        // Autosave after delay.
        item.update(cx, |item, cx| {
            SettingsStore::update_global(cx, |settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.workspace.autosave = Some(AutosaveSetting::AfterDelay {
                        milliseconds: 500.into(),
                    });
                })
            });
            item.is_dirty = true;
            cx.emit(ItemEvent::Edit);
        });

        // Delay hasn't fully expired, so the file is still dirty and unsaved.
        cx.executor().advance_clock(Duration::from_millis(250));
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 3));

        // After delay expires, the file is saved.
        cx.executor().advance_clock(Duration::from_millis(250));
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 4));

        // Autosave after delay, should save earlier than delay if tab is closed
        item.update(cx, |item, cx| {
            item.is_dirty = true;
            cx.emit(ItemEvent::Edit);
        });
        cx.executor().advance_clock(Duration::from_millis(250));
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 4));

        // // Ensure auto save with delay saves the item on close, even if the timer hasn't yet run out.
        pane.update_in(cx, |pane, window, cx| {
            pane.close_items(window, cx, SaveIntent::Close, &move |id| id == item_id)
        })
        .await
        .unwrap();
        assert!(!cx.has_pending_prompt());
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 5));

        // Add the item again, ensuring autosave is prevented if the underlying file has been deleted.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });
        item.update_in(cx, |item, _window, cx| {
            item.is_dirty = true;
            for project_item in &mut item.project_items {
                project_item.update(cx, |project_item, _| project_item.is_dirty = true);
            }
        });
        cx.run_until_parked();
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 5));

        // Autosave on focus change, ensuring closing the tab counts as such.
        item.update(cx, |item, cx| {
            SettingsStore::update_global(cx, |settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.workspace.autosave = Some(AutosaveSetting::OnFocusChange);
                })
            });
            item.is_dirty = true;
            for project_item in &mut item.project_items {
                project_item.update(cx, |project_item, _| project_item.is_dirty = true);
            }
        });

        pane.update_in(cx, |pane, window, cx| {
            pane.close_items(window, cx, SaveIntent::Close, &move |id| id == item_id)
        })
        .await
        .unwrap();
        assert!(!cx.has_pending_prompt());
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 6));

        // Add the item again, ensuring autosave is prevented if the underlying file has been deleted.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });
        item.update_in(cx, |item, window, cx| {
            item.project_items[0].update(cx, |item, _| {
                item.entry_id = None;
            });
            item.is_dirty = true;
            window.blur();
        });
        cx.run_until_parked();
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 6));

        // Ensure autosave is prevented for deleted files also when closing the buffer.
        let _close_items = pane.update_in(cx, |pane, window, cx| {
            pane.close_items(window, cx, SaveIntent::Close, &move |id| id == item_id)
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 6));
    }

    #[gpui::test]
    async fn test_autosave_on_focus_change_in_multibuffer(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // Create a multibuffer-like item with two child focus handles,
        // simulating individual buffer editors within a multibuffer.
        let item = cx.new(|cx| {
            TestItem::new(cx)
                .with_project_items(&[TestProjectItem::new(1, "1.txt", cx)])
                .with_child_focus_handles(2, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });

        // Set autosave to OnFocusChange and focus the first child handle,
        // simulating the user's cursor being inside one of the multibuffer's excerpts.
        item.update_in(cx, |item, window, cx| {
            SettingsStore::update_global(cx, |settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.workspace.autosave = Some(AutosaveSetting::OnFocusChange);
                })
            });
            item.is_dirty = true;
            window.focus(&item.child_focus_handles[0], cx);
        });
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 0));

        // Moving focus from one child to another within the same item should
        // NOT trigger autosave — focus is still within the item's focus hierarchy.
        item.update_in(cx, |item, window, cx| {
            window.focus(&item.child_focus_handles[1], cx);
        });
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| {
            assert_eq!(
                item.save_count, 0,
                "Switching focus between children within the same item should not autosave"
            );
        });

        // Focus leaving the item saves the file. This is the core regression scenario:
        // with `on_blur`, this would NOT trigger because `on_blur` only fires when
        // the item's own focus handle is the leaf that lost focus. In a multibuffer,
        // the leaf is always a child focus handle, so `on_blur` never detected
        // focus leaving the item.
        //
        // With deferred saves, the save happens when focus lands on a pane/panel or
        // the window deactivates.
        cx.deactivate_window();
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| {
            assert_eq!(
                item.save_count, 1,
                "Window deactivation should trigger autosave when focus was on a child of the item"
            );
        });
        cx.update(|window, _| window.activate_window());

        // Deactivating the window should also trigger autosave when a child of
        // the multibuffer item currently owns focus.
        item.update_in(cx, |item, window, cx| {
            item.is_dirty = true;
            window.focus(&item.child_focus_handles[0], cx);
        });
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| assert_eq!(item.save_count, 1));

        cx.deactivate_window();
        item.read_with(cx, |item, _| {
            assert_eq!(
                item.save_count, 2,
                "Deactivating window should trigger autosave when focus was on a child"
            );
        });
    }

    #[gpui::test]
    async fn test_autosave_deferred_for_modals(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let item = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "1.txt", cx)])
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
        });

        item.update_in(cx, |item, window, cx| {
            SettingsStore::update_global(cx, |settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.workspace.autosave = Some(AutosaveSetting::OnFocusChange);
                })
            });
            item.is_dirty = true;
            cx.focus_self(window);
        });
        cx.executor().run_until_parked();

        // Opening a modal moves focus away from the item, but autosave should be
        // deferred until focus lands on a pane or panel (not saved immediately).
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_modal(window, cx, TestModal::new);
        });
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| {
            assert_eq!(
                item.save_count, 0,
                "Opening a modal should NOT immediately trigger autosave"
            );
        });

        // If focus returns to the same item (modal dismissed), the deferred save
        // should be skipped.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.modal_layer.update(cx, |modal, cx| {
                modal.hide_modal(window, cx);
            });
        });
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| {
            assert_eq!(
                item.save_count, 0,
                "Returning focus to the same item should skip deferred save"
            );
        });

        // Open modal again with a dirty item.
        item.update_in(cx, |item, window, cx| {
            item.is_dirty = true;
            cx.focus_self(window);
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_modal(window, cx, TestModal::new);
        });
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| {
            assert_eq!(item.save_count, 0, "Modal open should not trigger save");
        });

        // Window deactivation should flush deferred saves.
        cx.deactivate_window();
        cx.executor().run_until_parked();
        item.read_with(cx, |item, _| {
            assert_eq!(
                item.save_count, 1,
                "Window deactivation should flush deferred saves"
            );
        });
    }

    #[gpui::test]
    async fn test_autosave_deferred_until_pane_focus(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let item1 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "1.txt", cx)])
        });
        let item2 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(2, "2.txt", cx)])
        });

        let pane = workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item1.clone()), None, false, window, cx);
            workspace.add_item_to_active_pane(Box::new(item2.clone()), None, false, window, cx);
            workspace.active_pane().clone()
        });
        // Ensure added_to_pane is called for both items (sets up focus handlers)
        cx.executor().run_until_parked();

        // Activate item1 (at index 0) and focus it.
        pane.update_in(cx, |pane, window, cx| {
            pane.activate_item(0, true, true, window, cx);
        });
        cx.executor().run_until_parked();

        // Set up OnFocusChange autosave and make item1 dirty.
        item1.update(cx, |item, cx| {
            SettingsStore::update_global(cx, |settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.workspace.autosave = Some(AutosaveSetting::OnFocusChange);
                })
            });
            item.is_dirty = true;
        });
        cx.executor().run_until_parked();

        // Activate item2 via the pane - this should trigger autosave of item1.
        pane.update_in(cx, |pane, window, cx| {
            pane.activate_item(1, true, true, window, cx);
        });
        cx.executor().run_until_parked();

        item1.read_with(cx, |item, _| {
            assert_eq!(
                item.save_count, 1,
                "Switching to another item should trigger deferred save of the previous item"
            );
        });
    }

    #[gpui::test]
    async fn test_pane_navigation(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let item = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "1.txt", cx)])
        });
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        let toolbar = pane.read_with(cx, |pane, _| pane.toolbar().clone());
        let toolbar_notify_count = Rc::new(RefCell::new(0));

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item.clone()), None, true, window, cx);
            let toolbar_notification_count = toolbar_notify_count.clone();
            cx.observe_in(&toolbar, window, move |_, _, _, _| {
                *toolbar_notification_count.borrow_mut() += 1
            })
            .detach();
        });

        pane.read_with(cx, |pane, _| {
            assert!(!pane.can_navigate_backward());
            assert!(!pane.can_navigate_forward());
        });

        item.update_in(cx, |item, _, cx| {
            item.set_state("one".to_string(), cx);
        });

        // Toolbar must be notified to re-render the navigation buttons
        assert_eq!(*toolbar_notify_count.borrow(), 1);

        pane.read_with(cx, |pane, _| {
            assert!(pane.can_navigate_backward());
            assert!(!pane.can_navigate_forward());
        });

        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.go_back(pane.downgrade(), window, cx)
            })
            .await
            .unwrap();

        assert_eq!(*toolbar_notify_count.borrow(), 2);
        pane.read_with(cx, |pane, _| {
            assert!(!pane.can_navigate_backward());
            assert!(pane.can_navigate_forward());
        });
    }

    /// Tests that the navigation history deduplicates entries for the same item.
    ///
    /// When navigating back and forth between items (e.g., A -> B -> A -> B -> A -> B -> C),
    /// the navigation history deduplicates by keeping only the most recent visit to each item,
    /// resulting in [A, B, C] instead of [A, B, A, B, A, B, C]. This ensures that Go Back (Ctrl-O)
    /// navigates through unique items efficiently: C -> B -> A, rather than bouncing between
    /// repeated entries: C -> B -> A -> B -> A -> B -> A.
    ///
    /// This behavior prevents the navigation history from growing unnecessarily large and provides
    /// a better user experience by eliminating redundant navigation steps when jumping between files.
    #[gpui::test]
    async fn test_navigation_history_deduplication(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let item_a = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "a.txt", cx)])
        });
        let item_b = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(2, "b.txt", cx)])
        });
        let item_c = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(3, "c.txt", cx)])
        });

        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item_a.clone()), None, true, window, cx);
            workspace.add_item_to_active_pane(Box::new(item_b.clone()), None, true, window, cx);
            workspace.add_item_to_active_pane(Box::new(item_c.clone()), None, true, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.activate_item(&item_a, false, false, window, cx);
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.activate_item(&item_b, false, false, window, cx);
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.activate_item(&item_a, false, false, window, cx);
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.activate_item(&item_b, false, false, window, cx);
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.activate_item(&item_a, false, false, window, cx);
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.activate_item(&item_b, false, false, window, cx);
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.activate_item(&item_c, false, false, window, cx);
        });
        cx.run_until_parked();

        let backward_count = pane.read_with(cx, |pane, cx| {
            let mut count = 0;
            pane.nav_history().for_each_entry(cx, &mut |_, _| {
                count += 1;
            });
            count
        });
        assert!(
            backward_count <= 4,
            "Should have at most 4 entries, got {}",
            backward_count
        );

        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.go_back(pane.downgrade(), window, cx)
            })
            .await
            .unwrap();

        let active_item = workspace.read_with(cx, |workspace, cx| {
            workspace.active_item(cx).unwrap().item_id()
        });
        assert_eq!(
            active_item,
            item_b.entity_id(),
            "After first go_back, should be at item B"
        );

        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.go_back(pane.downgrade(), window, cx)
            })
            .await
            .unwrap();

        let active_item = workspace.read_with(cx, |workspace, cx| {
            workspace.active_item(cx).unwrap().item_id()
        });
        assert_eq!(
            active_item,
            item_a.entity_id(),
            "After second go_back, should be at item A"
        );

        pane.read_with(cx, |pane, _| {
            assert!(pane.can_navigate_forward(), "Should be able to go forward");
        });
    }

    #[gpui::test]
    async fn test_activate_last_pane(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        workspace.update_in(cx, |workspace, window, cx| {
            let first_item = cx.new(|cx| {
                TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "1.txt", cx)])
            });
            workspace.add_item_to_active_pane(Box::new(first_item), None, true, window, cx);
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Right,
                window,
                cx,
            );
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Right,
                window,
                cx,
            );
        });

        let (first_pane_id, target_last_pane_id) = workspace.update(cx, |workspace, _cx| {
            let panes = workspace.center.panes();
            assert!(panes.len() >= 2);
            (
                panes.first().expect("at least one pane").entity_id(),
                panes.last().expect("at least one pane").entity_id(),
            )
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.activate_pane_at_index(&ActivatePane(0), window, cx);
        });
        workspace.update(cx, |workspace, _| {
            assert_eq!(workspace.active_pane().entity_id(), first_pane_id);
            assert_ne!(workspace.active_pane().entity_id(), target_last_pane_id);
        });

        cx.dispatch_action(ActivateLastPane);

        workspace.update(cx, |workspace, _| {
            assert_eq!(workspace.active_pane().entity_id(), target_last_pane_id);
        });
    }

    #[gpui::test]
    async fn test_reset_pane_sizes(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        // A horizontal split of three panes whose last child is itself a vertical
        // split, so equalizing has to recurse into the nested axis.
        workspace.update_in(cx, |workspace, window, cx| {
            let item = cx.new(|cx| {
                TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "1.txt", cx)])
            });
            workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Right,
                window,
                cx,
            );
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Right,
                window,
                cx,
            );
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Down,
                window,
                cx,
            );
        });

        let nested_axis = |workspace: &Workspace| {
            let Member::Axis(top) = &workspace.center.root else {
                panic!("expected the center to be a split axis");
            };
            let nested = top
                .members
                .iter()
                .find_map(|member| match member {
                    Member::Axis(axis) => Some(axis.clone()),
                    Member::Pane(_) => None,
                })
                .expect("expected a nested split axis");
            (top.clone(), nested)
        };

        // Skew every axis away from uniform sizes.
        workspace.update(cx, |workspace, _| {
            let (top, nested) = nested_axis(workspace);
            *top.flexes.lock() = vec![1.6, 0.7, 0.7];
            *nested.flexes.lock() = vec![1.3, 0.7];
        });

        cx.run_until_parked();
        cx.dispatch_action(ResetPaneSizes);

        workspace.update(cx, |workspace, _| {
            let (top, nested) = nested_axis(workspace);
            assert_eq!(*top.flexes.lock(), vec![1.0; top.members.len()]);
            assert_eq!(*nested.flexes.lock(), vec![1.0; nested.members.len()]);
        });
    }

    #[gpui::test]
    async fn test_toggle_docks_and_panels(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);

            workspace
                .right_dock()
                .update(cx, |right_dock, cx| right_dock.set_open(true, window, cx));

            panel
        });

        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        pane.update_in(cx, |pane, window, cx| {
            let item = cx.new(TestItem::new);
            pane.add_item(Box::new(item), true, true, None, window, cx);
        });

        // Transfer focus from center to panel
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(!panel.is_zoomed(window, cx));
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Transfer focus from panel to center
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(!panel.is_zoomed(window, cx));
            assert!(!panel.read(cx).focus_handle(cx).contains_focused(window, cx));
            assert!(pane.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Close the dock
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Right, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(!workspace.right_dock().read(cx).is_open());
            assert!(!panel.is_zoomed(window, cx));
            assert!(!panel.read(cx).focus_handle(cx).contains_focused(window, cx));
            assert!(pane.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Open the dock
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Right, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(!panel.is_zoomed(window, cx));
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Focus and zoom panel
        panel.update_in(cx, |panel, window, cx| {
            cx.focus_self(window);
            panel.set_zoomed(true, window, cx)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(panel.is_zoomed(window, cx));
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Transfer focus to the center closes the dock
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(!workspace.right_dock().read(cx).is_open());
            assert!(panel.is_zoomed(window, cx));
            assert!(!panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Transferring focus back to the panel keeps it zoomed
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(panel.is_zoomed(window, cx));
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Close the dock while it is zoomed
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Right, window, cx)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(!workspace.right_dock().read(cx).is_open());
            assert!(panel.is_zoomed(window, cx));
            assert!(workspace.zoomed.is_none());
            assert!(!panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Opening the dock, when it's zoomed, retains focus
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Right, window, cx)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(panel.is_zoomed(window, cx));
            assert!(workspace.zoomed.is_some());
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Unzoom and close the panel, zoom the active pane.
        panel.update_in(cx, |panel, window, cx| panel.set_zoomed(false, window, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Right, window, cx)
        });
        pane.update_in(cx, |pane, window, cx| {
            pane.toggle_zoom(&Default::default(), window, cx)
        });

        // Opening a dock unzooms the pane.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Right, window, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            let pane = pane.read(cx);
            assert!(!pane.is_zoomed());
            assert!(!pane.focus_handle(cx).is_focused(window));
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(workspace.zoomed.is_none());
        });
    }

    #[gpui::test]
    async fn test_panel_activation_and_region_navigation_focus_activation_handle(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel =
                cx.new(|cx| TestPanel::new_with_activation_child(DockPosition::Right, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        let activation_focus_handle = workspace.update_in(cx, |_workspace, window, cx| {
            let activation_focus_handle = panel
                .read(cx)
                .activation_focus_handle
                .clone()
                .expect("test panel should have an activation child");
            assert!(activation_focus_handle.is_focused(window));
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
            activation_focus_handle
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.active_pane().focus_handle(cx).focus(window, cx);
            workspace.move_part_focus(true, window, cx);
        });
        workspace.update_in(cx, |_workspace, window, _cx| {
            assert!(activation_focus_handle.is_focused(window));
        });
    }

    #[gpui::test]
    async fn test_close_panel_on_toggle(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        pane.update_in(cx, |pane, window, cx| {
            let item = cx.new(TestItem::new);
            pane.add_item(Box::new(item), true, true, None, window, cx);
        });

        // Enable close_panel_on_toggle
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings.workspace.close_panel_on_toggle = Some(true);
            });
        });

        // Panel starts closed. Toggling should open and focus it.
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(!workspace.right_dock().read(cx).is_open());
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(
                workspace.right_dock().read(cx).is_open(),
                "Dock should be open after toggling from center"
            );
            assert!(
                panel.read(cx).focus_handle(cx).contains_focused(window, cx),
                "Panel should be focused after toggling from center"
            );
        });

        // Panel is open and focused. Toggling should close the panel and
        // return focus to the center.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(
                !workspace.right_dock().read(cx).is_open(),
                "Dock should be closed after toggling from focused panel"
            );
            assert!(
                !panel.read(cx).focus_handle(cx).contains_focused(window, cx),
                "Panel should not be focused after toggling from focused panel"
            );
        });

        // Open the dock and focus something else so the panel is open but not
        // focused. Toggling should focus the panel (not close it).
        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .right_dock()
                .update(cx, |dock, cx| dock.set_open(true, window, cx));
            window.focus(&pane.read(cx).focus_handle(cx), cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(!panel.read(cx).focus_handle(cx).contains_focused(window, cx));
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(
                workspace.right_dock().read(cx).is_open(),
                "Dock should remain open when toggling focuses an open-but-unfocused panel"
            );
            assert!(
                panel.read(cx).focus_handle(cx).contains_focused(window, cx),
                "Panel should be focused after toggling an open-but-unfocused panel"
            );
        });

        // Now disable the setting and verify the original behavior: toggling
        // from a focused panel moves focus to center but leaves the dock open.
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings.workspace.close_panel_on_toggle = Some(false);
            });
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(
                workspace.right_dock().read(cx).is_open(),
                "Dock should remain open when setting is disabled"
            );
            assert!(
                !panel.read(cx).focus_handle(cx).contains_focused(window, cx),
                "Panel should not be focused after toggling with setting disabled"
            );
        });
    }

    #[gpui::test]
    async fn test_pane_zoom_in_out(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let pane = workspace.update_in(cx, |workspace, _window, _cx| {
            workspace.active_pane().clone()
        });

        // Add an item to the pane so it can be zoomed
        workspace.update_in(cx, |workspace, window, cx| {
            let item = cx.new(TestItem::new);
            workspace.add_item(pane.clone(), Box::new(item), None, true, true, window, cx);
        });

        // Initially not zoomed
        workspace.update_in(cx, |workspace, _window, cx| {
            assert!(!pane.read(cx).is_zoomed(), "Pane starts unzoomed");
            assert!(
                workspace.zoomed.is_none(),
                "Workspace should track no zoomed pane"
            );
            assert!(pane.read(cx).items_len() > 0, "Pane should have items");
        });

        // Zoom In
        pane.update_in(cx, |pane, window, cx| {
            pane.zoom_in(&crate::ZoomIn, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(
                pane.read(cx).is_zoomed(),
                "Pane should be zoomed after ZoomIn"
            );
            assert!(
                workspace.zoomed.is_some(),
                "Workspace should track the zoomed pane"
            );
            assert!(
                pane.read(cx).focus_handle(cx).contains_focused(window, cx),
                "ZoomIn should focus the pane"
            );
        });

        // Zoom In again is a no-op
        pane.update_in(cx, |pane, window, cx| {
            pane.zoom_in(&crate::ZoomIn, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(pane.read(cx).is_zoomed(), "Second ZoomIn keeps pane zoomed");
            assert!(
                workspace.zoomed.is_some(),
                "Workspace still tracks zoomed pane"
            );
            assert!(
                pane.read(cx).focus_handle(cx).contains_focused(window, cx),
                "Pane remains focused after repeated ZoomIn"
            );
        });

        // Zoom Out
        pane.update_in(cx, |pane, window, cx| {
            pane.zoom_out(&crate::ZoomOut, window, cx);
        });

        workspace.update_in(cx, |workspace, _window, cx| {
            assert!(
                !pane.read(cx).is_zoomed(),
                "Pane should unzoom after ZoomOut"
            );
            assert!(
                workspace.zoomed.is_none(),
                "Workspace clears zoom tracking after ZoomOut"
            );
        });

        // Zoom Out again is a no-op
        pane.update_in(cx, |pane, window, cx| {
            pane.zoom_out(&crate::ZoomOut, window, cx);
        });

        workspace.update_in(cx, |workspace, _window, cx| {
            assert!(
                !pane.read(cx).is_zoomed(),
                "Second ZoomOut keeps pane unzoomed"
            );
            assert!(
                workspace.zoomed.is_none(),
                "Workspace remains without zoomed pane"
            );
        });
    }

    #[gpui::test]
    async fn test_toggle_editor_zoom(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let pane_a = workspace.update_in(cx, |workspace, _window, _cx| {
            workspace.active_pane().clone()
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let item = cx.new(TestItem::new);
            workspace.add_item(pane_a.clone(), Box::new(item), None, true, true, window, cx);
        });

        let pane_b = split_pane(cx, &workspace);
        workspace.update_in(cx, |workspace, window, cx| {
            let item = cx.new(TestItem::new);
            workspace.add_item(pane_b.clone(), Box::new(item), None, true, true, window, cx);
        });

        // Toggle editor zoom maximizes the active pane
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_editor_zoom(&ToggleEditorZoom, window, cx);
            assert!(workspace.is_pane_maximized());
            assert_eq!(
                workspace.maximized_pane.as_ref().and_then(|w| w.upgrade()),
                Some(pane_b.clone()),
            );
        });

        // Toggle again un-maximizes
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_editor_zoom(&ToggleEditorZoom, window, cx);
            assert!(!workspace.is_pane_maximized());
        });

        // Toggle editor zoom while zoomed: should unzoom then maximize
        pane_b.update_in(cx, |pane, window, cx| {
            pane.zoom_in(&ZoomIn, window, cx);
        });
        workspace.update_in(cx, |_workspace, _window, cx| {
            assert!(pane_b.read(cx).is_zoomed());
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_editor_zoom(&ToggleEditorZoom, window, cx);
            assert!(!pane_b.read(cx).is_zoomed());
            assert!(workspace.zoomed.is_none());
            assert!(workspace.is_pane_maximized());
        });

        // Un-maximize for next test
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_editor_zoom(&ToggleEditorZoom, window, cx);
        });

        // Maximized pane cleared when it is removed
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_editor_zoom(&ToggleEditorZoom, window, cx);
            assert!(workspace.is_pane_maximized());
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.remove_pane(pane_b.clone(), None, window, cx);
            assert!(!workspace.is_pane_maximized());
        });
    }

    #[gpui::test]
    async fn test_toggle_all_docks(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            // A dock's open state is its set of open panels, so each side needs
            // a registered panel before it can be open at all.
            let left_panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(left_panel, window, cx);
            let right_panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 101, cx));
            workspace.add_panel(right_panel, window, cx);

            let left_dock = workspace.dock_at_position(DockPosition::Left);
            let right_dock = workspace.dock_at_position(DockPosition::Right);

            left_dock.update(cx, |dock, cx| dock.set_open(true, window, cx));
            right_dock.update(cx, |dock, cx| dock.set_open(true, window, cx));

            assert!(left_dock.read(cx).is_open());
            assert!(right_dock.read(cx).is_open());
        });

        workspace.update_in(cx, |workspace, window, cx| {
            // Toggle all docks - should close both
            workspace.toggle_all_docks(&ToggleAllDocks, window, cx);

            let left_dock = workspace.dock_at_position(DockPosition::Left);
            let right_dock = workspace.dock_at_position(DockPosition::Right);
            assert!(!left_dock.read(cx).is_open());
            assert!(!right_dock.read(cx).is_open());
        });

        workspace.update_in(cx, |workspace, window, cx| {
            // Toggle again - should reopen both
            workspace.toggle_all_docks(&ToggleAllDocks, window, cx);

            let left_dock = workspace.dock_at_position(DockPosition::Left);
            let right_dock = workspace.dock_at_position(DockPosition::Right);
            assert!(left_dock.read(cx).is_open());
            assert!(right_dock.read(cx).is_open());
        });
    }

    #[gpui::test]
    async fn test_toggle_all_with_manual_close(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            // A dock's open state is its set of open panels, so each side needs
            // a registered panel before it can be open at all.
            let left_panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(left_panel, window, cx);
            let right_panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 101, cx));
            workspace.add_panel(right_panel, window, cx);

            let left_dock = workspace.dock_at_position(DockPosition::Left);
            let right_dock = workspace.dock_at_position(DockPosition::Right);

            left_dock.update(cx, |dock, cx| dock.set_open(true, window, cx));
            right_dock.update(cx, |dock, cx| dock.set_open(true, window, cx));

            assert!(left_dock.read(cx).is_open());
            assert!(right_dock.read(cx).is_open());
        });

        workspace.update_in(cx, |workspace, window, cx| {
            // Close them manually
            workspace.toggle_dock(DockPosition::Left, window, cx);
            workspace.toggle_dock(DockPosition::Right, window, cx);

            let left_dock = workspace.dock_at_position(DockPosition::Left);
            let right_dock = workspace.dock_at_position(DockPosition::Right);
            assert!(!left_dock.read(cx).is_open());
            assert!(!right_dock.read(cx).is_open());
        });

        workspace.update_in(cx, |workspace, window, cx| {
            // Toggle all docks - only last closed (right dock) should reopen
            workspace.toggle_all_docks(&ToggleAllDocks, window, cx);

            let left_dock = workspace.dock_at_position(DockPosition::Left);
            let right_dock = workspace.dock_at_position(DockPosition::Right);
            assert!(!left_dock.read(cx).is_open());
            assert!(right_dock.read(cx).is_open());
        });
    }

    #[gpui::test]
    async fn test_toggle_all_docks_after_dock_move(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        // Open two docks (left and right) with one panel each
        let (left_panel, right_panel) = workspace.update_in(cx, |workspace, window, cx| {
            let left_panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(left_panel.clone(), window, cx);

            // A distinct component type: position is global per component, so
            // two instances of one type share a position and cannot model two
            // components moving independently.
            let right_panel = cx.new(|cx| TestPanelB::new(DockPosition::Right, 101, cx));
            workspace.add_panel(right_panel.clone(), window, cx);

            workspace.toggle_dock(DockPosition::Left, window, cx);
            workspace.toggle_dock(DockPosition::Right, window, cx);

            // Verify initial state
            assert!(
                workspace.left_dock().read(cx).is_open(),
                "Left dock should be open"
            );
            assert_eq!(
                workspace
                    .left_dock()
                    .read(cx)
                    .visible_panel()
                    .unwrap()
                    .panel_id(),
                left_panel.panel_id(),
                "Left panel should be visible in left dock"
            );
            assert!(
                workspace.right_dock().read(cx).is_open(),
                "Right dock should be open"
            );
            assert_eq!(
                workspace
                    .right_dock()
                    .read(cx)
                    .visible_panel()
                    .unwrap()
                    .panel_id(),
                right_panel.panel_id(),
                "Right panel should be visible in right dock"
            );
            assert!(
                !workspace.bottom_dock().read(cx).is_open(),
                "Bottom dock should be closed"
            );

            (left_panel, right_panel)
        });

        // Focus the left panel and move it to the next position (bottom dock)
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx); // Focus left panel
            assert!(
                left_panel.read(cx).focus_handle(cx).is_focused(window),
                "Left panel should be focused"
            );
        });

        cx.dispatch_action(MoveFocusedPanelToNextPosition);

        // Verify the left panel has moved to the bottom dock, and the bottom dock is now open
        workspace.update(cx, |workspace, cx| {
            assert!(
                !workspace.left_dock().read(cx).is_open(),
                "Left dock should be closed"
            );
            assert!(
                workspace.bottom_dock().read(cx).is_open(),
                "Bottom dock should now be open"
            );
            assert_eq!(
                left_panel.read(cx).position,
                DockPosition::Bottom,
                "Left panel should now be in the bottom dock"
            );
            assert_eq!(
                workspace
                    .bottom_dock()
                    .read(cx)
                    .visible_panel()
                    .unwrap()
                    .panel_id(),
                left_panel.panel_id(),
                "Left panel should be the visible panel in the bottom dock"
            );
        });

        // Toggle all docks off
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_all_docks(&ToggleAllDocks, window, cx);
            assert!(
                !workspace.left_dock().read(cx).is_open(),
                "Left dock should be closed"
            );
            assert!(
                !workspace.right_dock().read(cx).is_open(),
                "Right dock should be closed"
            );
            assert!(
                !workspace.bottom_dock().read(cx).is_open(),
                "Bottom dock should be closed"
            );
        });

        // Toggle all docks back on and verify positions are restored
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_all_docks(&ToggleAllDocks, window, cx);
            assert!(
                !workspace.left_dock().read(cx).is_open(),
                "Left dock should remain closed"
            );
            assert!(
                workspace.right_dock().read(cx).is_open(),
                "Right dock should remain open"
            );
            assert!(
                workspace.bottom_dock().read(cx).is_open(),
                "Bottom dock should remain open"
            );
            assert_eq!(
                left_panel.read(cx).position,
                DockPosition::Bottom,
                "Left panel should remain in the bottom dock"
            );
            assert_eq!(
                right_panel.read(cx).position,
                DockPosition::Right,
                "Right panel should remain in the right dock"
            );
            assert_eq!(
                workspace
                    .bottom_dock()
                    .read(cx)
                    .visible_panel()
                    .unwrap()
                    .panel_id(),
                left_panel.panel_id(),
                "Left panel should be the visible panel in the right dock"
            );
        });
    }

    fn set_component_order(cx: &mut VisualTestContext, order: &[&str]) {
        let order: Vec<String> = order.iter().map(|key| key.to_string()).collect();
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings
                    .workspace_layout
                    .get_or_insert_default()
                    .component_order = Some(order.clone());
            });
        });
    }

    fn set_panel_pinned(cx: &mut VisualTestContext, panel_key: &str, pinned: bool) {
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings
                    .workspace_layout
                    .get_or_insert_default()
                    .component_positions
                    .entry(panel_key.to_string())
                    .or_default()
                    .pinned = Some(pinned);
            });
        });
    }

    #[gpui::test]
    async fn test_pinned_panels_stay_open_in_icon_order(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // Pinning is keyed by panel type, so this pins every `TestPanel`.
        set_panel_pinned(cx, "TestPanel", true);

        let (left_low, left_high, right_low, right_high) =
            workspace.update_in(cx, |workspace, window, cx| {
                let left_low = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
                workspace.add_panel(left_low.clone(), window, cx);
                let left_high = cx.new(|cx| TestPanel::new(DockPosition::Left, 101, cx));
                workspace.add_panel(left_high.clone(), window, cx);
                let right_low = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
                workspace.add_panel(right_low.clone(), window, cx);
                let right_high = cx.new(|cx| TestPanel::new(DockPosition::Right, 101, cx));
                workspace.add_panel(right_high.clone(), window, cx);
                (left_low, left_high, right_low, right_high)
            });

        // Open each side's higher-priority panel first, so screen order can
        // only match entry order by following the entries rather than the clicks.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(1, window, cx);
                dock.activate_panel(0, window, cx);
            });
            workspace.right_dock().update(cx, |dock, cx| {
                dock.activate_panel(1, window, cx);
                dock.activate_panel(0, window, cx);
            });
        });

        workspace.update(cx, |workspace, cx| {
            let open_ids = |dock: &Entity<Dock>, cx: &App| -> Vec<EntityId> {
                dock.read(cx)
                    .open_panels()
                    .iter()
                    .map(|panel| panel.panel_id())
                    .collect()
            };

            assert_eq!(
                open_ids(&workspace.left_dock(), cx),
                vec![left_low.panel_id(), left_high.panel_id()],
                "pinned siblings stay open together, in entry order on the left"
            );
            assert_eq!(
                open_ids(&workspace.right_dock(), cx),
                vec![right_high.panel_id(), right_low.panel_id()],
                "the right dock reverses entry order so columns read like its reversed icons"
            );
        });
    }

    /// The order the user writes reaches a dock that is already populated, not
    /// only one still being built. `add_panel` holds the sorted invariant at
    /// insert time; nothing held it when the order moved underneath.
    #[gpui::test]
    async fn test_a_new_icon_order_reaches_a_dock_that_is_already_open(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second, window, cx);
        });

        let keys = |cx: &mut VisualTestContext| {
            workspace.update(cx, |workspace, cx| {
                let dock = workspace.left_dock().read(cx);
                (0..dock.panels_len())
                    .filter_map(|ix| dock.panel_at(ix).map(|panel| panel.panel_key()))
                    .collect::<Vec<_>>()
            })
        };

        assert_eq!(
            keys(cx),
            vec!["TestPanel", "TestPanelB"],
            "with no order set, the activation priorities decide"
        );

        set_component_order(cx, &["TestPanelB", "TestPanel"]);
        cx.run_until_parked();

        assert_eq!(
            keys(cx),
            vec!["TestPanelB", "TestPanel"],
            "the written sequence reorders the strip in place"
        );

        set_component_order(cx, &[]);
        cx.run_until_parked();

        assert_eq!(
            keys(cx),
            vec!["TestPanel", "TestPanelB"],
            "and clearing it hands the order back to the priorities"
        );
    }

    /// The right dock draws its icons and its columns reversed, so "later" there
    /// is the other direction along `panel_entries`. Reading the entry list
    /// directly would send both menu entries the wrong way on the one side nobody
    /// thinks to check.
    #[gpui::test]
    async fn test_later_means_later_as_seen_on_the_right_dock(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // The fixtures draw no icon by default, like a live panel whose feature is
        // off — and an entry that draws nothing is not on the strip at all.
        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| {
                let mut panel = TestPanelB::new(DockPosition::Right, 101, cx);
                panel.icon = Some(ui::IconName::GitBranch);
                panel
            });
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| {
                let mut panel = TestPanelC::new(DockPosition::Right, 102, cx);
                panel.icon = Some(ui::IconName::TerminalAlt);
                panel
            });
            workspace.add_panel(second, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let dock = workspace.right_dock().read(cx);
            assert_eq!(
                (0..dock.panels_len())
                    .filter_map(|ix| dock.panel_at(ix).map(|panel| panel.panel_key()))
                    .collect::<Vec<_>>(),
                vec!["TestPanelB", "TestPanelC"],
                "the entry list keeps the sorted order every side shares"
            );
            assert_eq!(
                dock.displayed_panel_keys(window, cx),
                vec!["TestPanelC".to_string(), "TestPanelB".to_string()],
                "and the right dock draws it reversed, so the later entry reads first"
            );
        });
    }

    /// A component that draws no icon is not on the strip, so it is not a
    /// neighbour either — moving past it would look like nothing happening. The
    /// agent panel with the feature off and the terminal panel with no terminals
    /// both reach this state live.
    #[gpui::test]
    async fn test_a_component_without_an_icon_is_not_a_neighbour(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            let drawn = cx.new(|cx| {
                let mut panel = TestPanel::new(DockPosition::Left, 100, cx);
                panel.icon = Some(ui::IconName::FileTree);
                panel
            });
            workspace.add_panel(drawn, window, cx);
            let undrawn = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(undrawn, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let dock = workspace.left_dock().read(cx);
            assert_eq!(dock.panels_len(), 2, "both components are on the side");
            assert_eq!(
                dock.displayed_panel_keys(window, cx),
                vec!["TestPanel".to_string()],
                "but only the one that draws an icon is on the strip"
            );
        });
    }

    /// Reordering must not disturb what is open or what has emphasis. The dock
    /// tracks both by `EntityId` and derives `active_panel_index` from the focused
    /// id, so a sort moves no state — this is the witness that keeps it that way.
    #[gpui::test]
    async fn test_reordering_icons_keeps_the_same_panel_open_and_focused(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let second = workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(1, window, cx);
            });
            second
        });

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock().read(cx);
            assert_eq!(dock.focused_panel_id(), Some(second.panel_id()));
            assert_eq!(dock.active_panel_index(), Some(1));
        });

        set_component_order(cx, &["TestPanelB", "TestPanel"]);
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock().read(cx);
            assert_eq!(
                dock.focused_panel_id(),
                Some(second.panel_id()),
                "the same panel keeps emphasis across the sort"
            );
            assert_eq!(
                dock.active_panel_index(),
                Some(0),
                "and its index follows the icon rather than the icon following the index"
            );
            assert!(dock.is_panel_open(second.panel_id()));
        });
    }

    #[gpui::test]
    async fn test_icon_click_closes_only_the_clicked_panel(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);

        let (first, second) = workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first.clone(), window, cx);
            let second = cx.new(|cx| TestPanel::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second.clone(), window, cx);
            // `first` is activated last so the dock holds it as the focused
            // panel — which is what the icon draws and what the click reads.
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(1, window, cx);
                dock.activate_panel(0, window, cx);
            });
            (first, second)
        });

        // The clicked panel only has to be open — the toggle does not consult focus.
        workspace.update_in(cx, |workspace, window, cx| {
            window.focus(&first.read(cx).focus_handle(cx), cx);
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(first.panel_id())
            );
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(first.panel_id()),
                "the clicked panel must be open, or there is nothing for the toggle to close"
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let dock = workspace.left_dock().clone();
            workspace.handle_panel_icon_click(&dock, 0, window, cx);
        });

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock();
            assert!(
                !dock.read(cx).is_panel_open(first.panel_id()),
                "clicking an open panel's icon closes that panel"
            );
            assert!(
                dock.read(cx).is_panel_open(second.panel_id()),
                "its sibling stays open"
            );
            assert!(
                dock.read(cx).is_open(),
                "and the side stays open because a panel remains"
            );
        });
    }

    #[gpui::test]
    async fn test_unpinned_panels_share_one_slot(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let (first, second) = workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first.clone(), window, cx);
            let second = cx.new(|cx| TestPanel::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second.clone(), window, cx);
            (first, second)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.activate_panel(0, window, cx));
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(first.panel_id()),
                "the first panel must actually be open, or the displacement has nothing to displace"
            );
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.activate_panel(1, window, cx));
        });

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock();
            assert!(
                !dock.read(cx).is_panel_open(first.panel_id()),
                "with nothing pinned, opening a panel displaces the unpinned one showing there"
            );
            assert!(dock.read(cx).is_panel_open(second.panel_id()));
        });
    }

    #[gpui::test]
    async fn test_pinned_panel_is_not_displaced_by_an_unpinned_sibling(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // Same fixture as `test_unpinned_panels_share_one_slot` with one flag
        // flipped, so the two tests differ only in the thing under test.
        set_panel_pinned(cx, "TestPanel", true);

        let (pinned, unpinned) = workspace.update_in(cx, |workspace, window, cx| {
            let pinned = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(pinned.clone(), window, cx);
            let unpinned = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(unpinned.clone(), window, cx);
            (pinned, unpinned)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.activate_panel(0, window, cx));
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(pinned.panel_id()),
                "the pinned panel must be open, or there is nothing to protect from displacement"
            );
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.activate_panel(1, window, cx));
        });

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock();
            assert!(
                dock.read(cx).is_panel_open(pinned.panel_id()),
                "an unpinned panel opening never displaces a pinned sibling"
            );
            assert!(
                dock.read(cx).is_panel_open(unpinned.panel_id()),
                "and the unpinned panel opens beside it"
            );
        });
    }

    #[gpui::test]
    async fn test_pinned_panel_opening_does_not_displace_an_unpinned_sibling(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // The mirror of `test_pinned_panel_is_not_displaced_by_an_unpinned_sibling`:
        // there the pinned panel opens first. Only an unpinned panel takes the
        // side's shared slot, so a pinned one opening later evicts nobody.
        set_panel_pinned(cx, "TestPanel", true);

        let (pinned, unpinned) = workspace.update_in(cx, |workspace, window, cx| {
            let pinned = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(pinned.clone(), window, cx);
            let unpinned = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(unpinned.clone(), window, cx);
            (pinned, unpinned)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.activate_panel(1, window, cx));
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(unpinned.panel_id()),
                "the unpinned panel must hold the shared slot before the pinned one opens"
            );
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.activate_panel(0, window, cx));
        });

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock();
            assert!(
                dock.read(cx).is_panel_open(unpinned.panel_id()),
                "opening a pinned panel does not take the side's unpinned slot"
            );
            assert!(
                dock.read(cx).is_panel_open(pinned.panel_id()),
                "and the pinned panel opens beside it"
            );
        });
    }

    #[gpui::test]
    async fn test_captured_open_component_set_survives_a_restore(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        let (first, second) = workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first.clone(), window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
            });
            (first, second)
        });

        let captured = workspace.read_with(cx, |workspace, cx| {
            let captured = workspace.capture_open_components(cx);
            assert_eq!(
                captured.len(),
                2,
                "both open components must be captured, or the restore below proves nothing"
            );
            captured
        });

        // Close both without remembering them, which is the state a dock is in
        // before a restore. `set_open(false)` would stash the set and let the
        // group toggle reopen it, hiding whether the saved rows did the work.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.left_dock().update(cx, |dock, cx| {
                dock.close_panel_at(1, window, cx);
                dock.close_panel_at(0, window, cx);
            });
            assert!(!workspace.left_dock().read(cx).is_open());
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.left_dock().update(cx, |dock, cx| {
                dock.set_serialized_state(
                    DockData {
                        visible: true,
                        active_panel: None,
                        zoom: false,
                    },
                    Some(captured),
                );
                dock.restore_state(window, cx);
            });
        });

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock();
            assert!(
                dock.read(cx).is_panel_open(first.panel_id()),
                "a restore reopens every saved component, not just the active one"
            );
            assert!(
                dock.read(cx).is_panel_open(second.panel_id()),
                "including the sibling the legacy single active_panel column could not name"
            );
        });
    }

    fn set_panel_hidden(cx: &mut VisualTestContext, panel_key: &str, hidden: bool) {
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings
                    .workspace_layout
                    .get_or_insert_default()
                    .component_positions
                    .entry(panel_key.to_string())
                    .or_default()
                    .hidden = Some(hidden);
            });
        });
    }

    #[gpui::test]
    async fn test_the_component_adapter_finds_a_registered_panel_by_its_key(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(panel, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanelB::new(DockPosition::Right, 101, cx));
            workspace.add_panel(panel, window, cx);
        });

        workspace.update(cx, |workspace, cx| {
            assert!(
                workspace.component_adapter("TestPanel", cx).is_some(),
                "a registered panel must be reachable by its registry id"
            );
            assert!(
                workspace.component_adapter("NoSuchPanel", cx).is_none(),
                "an id no panel registered under must not resolve to some other panel"
            );

            // The two identities are different jobs: `persistent_name` names the
            // active panel in serialized dock state, `panel_key` is the shared
            // layout id. They differ in production — "Project Panel" against
            // "ProjectPanel" — so a lookup keyed on the wrong one works in every
            // fixture where they happen to match, and fails for every real panel.
            assert!(
                workspace.component_adapter("TestPanelB", cx).is_some(),
                "the lookup must be keyed on panel_key"
            );
            assert!(
                workspace.component_adapter("Test Panel B", cx).is_none(),
                "and must not answer to persistent_name"
            );
        });
    }

    #[gpui::test]
    async fn test_the_component_adapter_reaches_a_hidden_component(cx: &mut gpui::TestAppContext) {
        // The whole surface design rests on this. `Customize Workspace Layout`
        // has to offer to un-hide something, and it can only do that if a hidden
        // component still answers for its position and its action. Registration
        // is unconditional — the hidden check only closes a panel — but that is a
        // property of `initialize_panels` and `add_panel`, not a guarantee anyone
        // stated, so it is witnessed here rather than assumed.
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(panel, window, cx);
        });

        set_panel_hidden(cx, "TestPanel", true);
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            let adapter = workspace
                .component_adapter("TestPanel", cx)
                .expect("a hidden component must still be reachable, or it can never be recovered");
            assert!(
                !adapter.is_open(),
                "hidden closes the component, so the adapter must not report it open"
            );
            assert_eq!(
                adapter.position(window, cx),
                DockPosition::Left,
                "hiding never writes position, so the adapter still reports where it will return to"
            );
        });
    }

    #[gpui::test]
    async fn test_the_component_adapter_tracks_open_and_focused(cx: &mut gpui::TestAppContext) {
        // The discriminating check. An adapter whose `is_open` were stuck at
        // `false` passes both tests above, because neither opens anything.
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        workspace.update(cx, |workspace, cx| {
            assert!(
                !workspace
                    .component_adapter("TestPanel", cx)
                    .expect("registered")
                    .is_open(),
                "a component that has not been opened is not open"
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_panel::<TestPanel>(window, cx);
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            let adapter = workspace
                .component_adapter("TestPanel", cx)
                .expect("registered");
            assert!(
                adapter.is_open(),
                "the adapter must see the open it just did"
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.close_panel_at(0, window, cx));
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            assert!(
                !workspace
                    .component_adapter("TestPanel", cx)
                    .expect("registered")
                    .is_open(),
                "and must see the close, so the value is read rather than latched"
            );
        });

        drop(panel);
    }

    #[gpui::test]
    async fn test_a_hidden_component_is_not_reopened_by_a_restore(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        // The saved rows say this component was open — the stale state a
        // workspace that was dormant across a hide keeps on disk.
        let restore = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| {
            workspace.update_in(cx, |workspace, window, cx| {
                workspace.left_dock().update(cx, |dock, cx| {
                    dock.set_serialized_state(
                        DockData {
                            visible: true,
                            active_panel: None,
                            zoom: false,
                        },
                        Some(vec!["TestPanel".to_string()]),
                    );
                    dock.restore_state(window, cx);
                });
            });
        };

        // Positive control, same fixture: while it is visible those exact rows
        // do reopen it. Without this, the assertion below would also pass if
        // restore were simply broken.
        restore(&workspace, cx);
        workspace.update(cx, |workspace, cx| {
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(panel.panel_id()),
                "the saved rows must reopen a visible component"
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.close_panel_at(0, window, cx));
        });
        set_panel_hidden(cx, "TestPanel", true);
        cx.run_until_parked();

        restore(&workspace, cx);
        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock();
            assert!(
                !dock.read(cx).is_panel_open(panel.panel_id()),
                "a hidden component stays out of the open set, not merely unrendered"
            );
            assert!(
                !dock.read(cx).is_open(),
                "and the side has nothing left to show"
            );
        });
    }

    #[gpui::test]
    async fn test_hiding_a_component_closes_it_in_a_live_workspace(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.activate_panel(0, window, cx));
            panel
        });

        workspace.update(cx, |workspace, cx| {
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(panel.panel_id()),
                "it must be open first, or hiding it has nothing to close"
            );
        });

        set_panel_hidden(cx, "TestPanel", true);
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            assert!(
                !workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(panel.panel_id()),
                "hiding a component closes it in every live workspace, without waiting for a restart"
            );
        });
    }

    #[gpui::test]
    async fn test_unhiding_scrubs_open_rows_but_hiding_leaves_them(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // The scrub deletes by key across every workspace, so this test uses a
        // key no other test can be holding rows under.
        const KEY: &str = "scrub-witness-component";

        let (db, fs) = workspace.read_with(cx, |workspace, cx| {
            (WorkspaceDb::global(cx), workspace.app_state().fs.clone())
        });
        let workspace_id = db.next_id().await.unwrap();
        db.save_open_components(workspace_id, vec![KEY.to_string()])
            .await
            .unwrap();

        // Hiding must leave the rows alone. They are exactly what a later
        // unhide has to find and delete; live workspaces close the component
        // through the settings observer instead.
        workspace.update_in(cx, |_, window, cx| {
            set_component_hidden(fs.clone(), KEY.to_string(), true, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            db.open_components(workspace_id).unwrap(),
            vec![KEY.to_string()],
            "hiding leaves the saved open rows in place"
        );

        workspace.update_in(cx, |_, window, cx| {
            set_component_hidden(fs.clone(), KEY.to_string(), false, window, cx);
        });
        cx.run_until_parked();
        assert!(
            db.open_components(workspace_id).unwrap().is_empty(),
            "unhiding forgets the component's openness in every serialized workspace"
        );
    }

    /// Puts a settings file in place that cannot be written, so every component
    /// write in the test fails where a read-only home or a full disk would fail
    /// it. A directory standing in for the file is what does it: the write's own
    /// read of the current text hits it first.
    async fn with_unwritable_settings(cx: &mut VisualTestContext, workspace: &Entity<Workspace>) {
        // Both filesystems: component writes take the workspace's, and
        // `set_status_control_location` reaches for the global one. In this harness
        // they are not the same object, and blocking only one made the status-half
        // witness pass for the wrong reason.
        let app_state_fs = workspace.read_with(cx, |workspace, _| workspace.app_state().fs.clone());
        let global_fs = cx.update(|_, cx| <dyn fs::Fs>::global(cx));
        for fs in [app_state_fs, global_fs] {
            fs.create_dir(paths::settings_file()).await.log_err();
        }
    }

    /// A component write that fails has to say so. Every one of these went
    /// through `update_settings_file`, which drops its result on the floor — so a
    /// settings file the app could not write left a user clicking a control that
    /// silently did nothing, which is the state Q13 exists to forbid.
    #[gpui::test]
    async fn test_a_failed_component_write_is_reported_and_changes_nothing(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        with_unwritable_settings(cx, &workspace).await;

        let fs = workspace.read_with(cx, |workspace, _| workspace.app_state().fs.clone());
        let key = crate::layout_registry::ids::PROJECT_PANEL;
        workspace.update_in(cx, |_, window, cx| {
            set_component_pinned(fs.clone(), key.to_string(), true, window, cx);
        });
        cx.run_until_parked();

        let (title, _) = cx.pending_prompt().expect("the failure is reported");
        assert_eq!(
            title, "Failed to pin Project Panel",
            "and the report names the gesture and the component, not the settings key"
        );
        cx.simulate_prompt_answer("OK");
        cx.run_until_parked();

        cx.update(|_, cx| {
            assert!(
                !WorkspaceLayoutSettings::get_global(cx)
                    .component(key)
                    .pinned,
                "and the setting the write could not make never takes effect"
            );
        });
    }

    /// One gesture, one dialog. `Restore Default Positions…` queues three writes
    /// per component and one per status control — around sixty. They share one
    /// serialized queue and fail for one reason, so a dialog per write would bury
    /// a user who has already read the first.
    #[gpui::test]
    async fn test_a_burst_of_failed_placement_writes_raises_one_prompt(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        with_unwritable_settings(cx, &workspace).await;

        let fs = workspace.read_with(cx, |workspace, _| workspace.app_state().fs.clone());
        let key = crate::layout_registry::ids::PROJECT_PANEL;
        workspace.update_in(cx, |_, window, cx| {
            set_component_pinned(fs.clone(), key.to_string(), true, window, cx);
            set_component_hidden(fs.clone(), key.to_string(), true, window, cx);
            set_status_control_location(
                crate::layout_registry::ids::CURSOR_POSITION,
                StatusBarItemLocation::Hidden,
                window,
                cx,
            );
        });
        cx.run_until_parked();

        assert!(cx.has_pending_prompt(), "the gesture is reported once");
        cx.simulate_prompt_answer("OK");
        cx.run_until_parked();
        assert!(
            !cx.has_pending_prompt(),
            "and the gesture's later failed writes do not stack more dialogs behind it"
        );
    }

    /// The status half of the same contract. It had a prompt before this, with a
    /// message naming the bottom bar rather than the item and no focus
    /// restoration — so it reported, but not what Q13 asks for. Both halves now
    /// go through one reporter, which is also what makes the single dialog above
    /// hold for a gesture that touches both kinds.
    #[gpui::test]
    async fn test_a_failed_status_control_write_names_the_item(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        with_unwritable_settings(cx, &workspace).await;

        workspace.update_in(cx, |_, window, cx| {
            set_status_control_location(
                crate::layout_registry::ids::CURSOR_POSITION,
                StatusBarItemLocation::Hidden,
                window,
                cx,
            );
        });
        cx.run_until_parked();

        let (title, _) = cx.pending_prompt().expect("the failure is reported");
        assert_eq!(
            title, "Failed to hide Cursor Position",
            "and names the item and the gesture, not the settings key or the bar"
        );
        cx.simulate_prompt_answer("OK");
        cx.run_until_parked();
    }

    /// Two focusables that are actually in the element tree. A handle that never
    /// renders is dropped from focus at the next draw, which would make a
    /// focus-restoration witness pass or fail for reasons unrelated to the code
    /// under test.
    struct TwoFocusables {
        chooser: FocusHandle,
        elsewhere: FocusHandle,
    }

    impl Render for TwoFocusables {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.chooser)
                .child(div().track_focus(&self.elsewhere))
        }
    }

    /// The prompt takes keyboard focus, and Q13 requires the surface that made
    /// the choice to still be driveable once the prompt is gone. Focus moving
    /// away while the write is in flight stands in for the prompt taking it — the
    /// test platform's prompt does not move GPUI focus, so without that move the
    /// witness would pass whether or not anything restored it.
    #[gpui::test]
    async fn test_a_failed_component_write_hands_focus_back(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        fs.create_dir(paths::settings_file()).await.unwrap();
        let fs: Arc<dyn fs::Fs> = fs;

        let (view, cx) = cx.add_window_view(|_, cx| TwoFocusables {
            chooser: cx.focus_handle(),
            elsewhere: cx.focus_handle(),
        });
        let (chooser, elsewhere) =
            view.read_with(cx, |view, _| (view.chooser.clone(), view.elsewhere.clone()));
        cx.update(|window, cx| window.focus(&chooser, cx));
        cx.run_until_parked();

        let key = crate::layout_registry::ids::PROJECT_PANEL;
        cx.update(|window, cx| {
            set_component_pinned(fs.clone(), key.to_string(), true, window, cx);
            window.focus(&elsewhere, cx);
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt(), "the fixture must fail the write");
        cx.simulate_prompt_answer("OK");
        cx.run_until_parked();

        cx.update(|window, cx| {
            assert_eq!(
                window.focused(cx),
                Some(chooser),
                "focus returns to what made the choice, not to wherever the prompt left it"
            );
        });
    }

    fn set_panel_position(
        cx: &mut VisualTestContext,
        panel_key: &str,
        position: settings::DockPosition,
    ) {
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings
                    .workspace_layout
                    .get_or_insert_default()
                    .component_positions
                    .entry(panel_key.to_string())
                    .or_default()
                    .position = Some(position);
            });
        });
    }

    #[gpui::test]
    async fn test_component_position_moves_the_actual_component(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.activate_panel(0, window, cx));
            panel
        });

        workspace.update(cx, |workspace, cx| {
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .panel_id_for_panel_key("TestPanel")
                    .is_some(),
                "the component must start on the left, or the move below proves nothing"
            );
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(panel.panel_id())
            );
        });

        set_panel_position(cx, "TestPanel", settings::DockPosition::Right);
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            // A dock's panel entries are what its icon strip renders from, so
            // the entry moving is the icon moving. This is the gesture the
            // icon-only build failed: the icon went right, the tree stayed left.
            assert!(
                workspace
                    .right_dock()
                    .read(cx)
                    .panel_id_for_panel_key("TestPanel")
                    .is_some(),
                "the real component moves to the side its position names"
            );
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .panel_id_for_panel_key("TestPanel")
                    .is_none(),
                "and it leaves the side it came from, icon included"
            );
            assert!(
                workspace
                    .right_dock()
                    .read(cx)
                    .is_panel_open(panel.panel_id()),
                "a component visible before the move is still visible after it"
            );
        });
    }

    #[gpui::test]
    async fn test_a_settings_file_unhide_scrubs_open_rows(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // Run the product's own wiring. Dogfood check J failed because the
        // scrub had callers in tests and none in the app, so registering the
        // observer here instead would prove the mechanism and miss the defect.
        workspace.update_in(cx, |_, _, cx| {
            let app_state = AppState::test(cx);
            init(app_state, cx);
        });

        const KEY: &str = "settings-unhide-witness";
        let db = workspace.read_with(cx, |_, cx| WorkspaceDb::global(cx));
        let workspace_id = db.next_id().await.unwrap();
        db.save_open_components(workspace_id, vec![KEY.to_string()])
            .await
            .unwrap();

        // Hide and unhide entirely through the settings store, which is the
        // path a hand edit takes. `set_component_hidden` is never called.
        set_panel_hidden(cx, KEY, true);
        cx.run_until_parked();
        assert_eq!(
            db.open_components(workspace_id).unwrap(),
            vec![KEY.to_string()],
            "hiding leaves the rows in place for the unhide to find"
        );

        set_panel_hidden(cx, KEY, false);
        cx.run_until_parked();
        assert!(
            db.open_components(workspace_id).unwrap().is_empty(),
            "an unhide written straight to settings still scrubs every serialized workspace"
        );
    }

    #[gpui::test]
    async fn test_a_closed_component_is_not_reopened_when_another_panel_registers(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        let restored = workspace.update_in(cx, |workspace, window, cx| {
            let restored = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(restored.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.set_serialized_state(
                    DockData {
                        visible: true,
                        active_panel: None,
                        zoom: false,
                    },
                    Some(vec!["TestPanel".to_string()]),
                );
                dock.restore_state(window, cx);
            });
            restored
        });

        workspace.update(cx, |workspace, cx| {
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(restored.panel_id()),
                "the restore must have opened it, or closing it below proves nothing"
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.close_panel_at(0, window, cx));
            assert!(
                !workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(restored.panel_id())
            );
        });

        // Registering a panel re-runs `restore_state`, and a dock move does
        // exactly this. A saved set that stayed armed would reopen what the
        // user just closed.
        workspace.update_in(cx, |workspace, window, cx| {
            let later = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(later, window, cx);
        });

        workspace.update(cx, |workspace, cx| {
            assert!(
                !workspace
                    .left_dock()
                    .read(cx)
                    .is_panel_open(restored.panel_id()),
                "a component closed after its restore stays closed when another panel registers"
            );
        });
    }

    #[gpui::test]
    async fn test_icon_click_closes_an_open_panel_that_does_not_have_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        let (unfocused, focused) = workspace.update_in(cx, |workspace, window, cx| {
            let unfocused = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(unfocused.clone(), window, cx);
            let focused = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(focused.clone(), window, cx);
            // Activated last, so the dock's emphasis sits on the *other* panel.
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
            });
            (unfocused, focused)
        });

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock();
            assert!(dock.read(cx).is_panel_open(unfocused.panel_id()));
            assert_eq!(
                dock.read(cx).focused_panel_id(),
                Some(focused.panel_id()),
                "the panel under test must NOT hold emphasis — that is the whole case"
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let dock = workspace.left_dock().clone();
            workspace.handle_panel_icon_click(&dock, 0, window, cx);
        });

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.left_dock();
            assert!(
                !dock.read(cx).is_panel_open(unfocused.panel_id()),
                "the icon is a plain toggle: one click closes an open panel, focused or not"
            );
            assert!(
                dock.read(cx).is_panel_open(focused.panel_id()),
                "and leaves its sibling alone"
            );
        });
    }

    #[gpui::test]
    async fn test_join_pane_into_next(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // Let's arrange the panes like this:
        //
        // +-----------------------+
        // |         top           |
        // +------+--------+-------+
        // | left | center | right |
        // +------+--------+-------+
        // |        bottom         |
        // +-----------------------+

        let top_item = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "top.txt", cx)])
        });
        let bottom_item = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(2, "bottom.txt", cx)])
        });
        let left_item = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(3, "left.txt", cx)])
        });
        let right_item = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(4, "right.txt", cx)])
        });
        let center_item = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(5, "center.txt", cx)])
        });

        let top_pane_id = workspace.update_in(cx, |workspace, window, cx| {
            let top_pane_id = workspace.active_pane().entity_id();
            workspace.add_item_to_active_pane(Box::new(top_item.clone()), None, false, window, cx);
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Down,
                window,
                cx,
            );
            top_pane_id
        });
        let bottom_pane_id = workspace.update_in(cx, |workspace, window, cx| {
            let bottom_pane_id = workspace.active_pane().entity_id();
            workspace.add_item_to_active_pane(
                Box::new(bottom_item.clone()),
                None,
                false,
                window,
                cx,
            );
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Up,
                window,
                cx,
            );
            bottom_pane_id
        });
        let left_pane_id = workspace.update_in(cx, |workspace, window, cx| {
            let left_pane_id = workspace.active_pane().entity_id();
            workspace.add_item_to_active_pane(Box::new(left_item.clone()), None, false, window, cx);
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Right,
                window,
                cx,
            );
            left_pane_id
        });
        let right_pane_id = workspace.update_in(cx, |workspace, window, cx| {
            let right_pane_id = workspace.active_pane().entity_id();
            workspace.add_item_to_active_pane(
                Box::new(right_item.clone()),
                None,
                false,
                window,
                cx,
            );
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Left,
                window,
                cx,
            );
            right_pane_id
        });
        let center_pane_id = workspace.update_in(cx, |workspace, window, cx| {
            let center_pane_id = workspace.active_pane().entity_id();
            workspace.add_item_to_active_pane(
                Box::new(center_item.clone()),
                None,
                false,
                window,
                cx,
            );
            center_pane_id
        });
        cx.executor().run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(center_pane_id, workspace.active_pane().entity_id());

            // Join into next from center pane into right
            workspace.join_pane_into_next(workspace.active_pane().clone(), window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let active_pane = workspace.active_pane();
            assert_eq!(right_pane_id, active_pane.entity_id());
            assert_eq!(2, active_pane.read(cx).items_len());
            let item_ids_in_pane =
                HashSet::from_iter(active_pane.read(cx).items().map(|item| item.item_id()));
            assert!(item_ids_in_pane.contains(&center_item.item_id()));
            assert!(item_ids_in_pane.contains(&right_item.item_id()));

            // Join into next from right pane into bottom
            workspace.join_pane_into_next(workspace.active_pane().clone(), window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let active_pane = workspace.active_pane();
            assert_eq!(bottom_pane_id, active_pane.entity_id());
            assert_eq!(3, active_pane.read(cx).items_len());
            let item_ids_in_pane =
                HashSet::from_iter(active_pane.read(cx).items().map(|item| item.item_id()));
            assert!(item_ids_in_pane.contains(&center_item.item_id()));
            assert!(item_ids_in_pane.contains(&right_item.item_id()));
            assert!(item_ids_in_pane.contains(&bottom_item.item_id()));

            // Join into next from bottom pane into left
            workspace.join_pane_into_next(workspace.active_pane().clone(), window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let active_pane = workspace.active_pane();
            assert_eq!(left_pane_id, active_pane.entity_id());
            assert_eq!(4, active_pane.read(cx).items_len());
            let item_ids_in_pane =
                HashSet::from_iter(active_pane.read(cx).items().map(|item| item.item_id()));
            assert!(item_ids_in_pane.contains(&center_item.item_id()));
            assert!(item_ids_in_pane.contains(&right_item.item_id()));
            assert!(item_ids_in_pane.contains(&bottom_item.item_id()));
            assert!(item_ids_in_pane.contains(&left_item.item_id()));

            // Join into next from left pane into top
            workspace.join_pane_into_next(workspace.active_pane().clone(), window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let active_pane = workspace.active_pane();
            assert_eq!(top_pane_id, active_pane.entity_id());
            assert_eq!(5, active_pane.read(cx).items_len());
            let item_ids_in_pane =
                HashSet::from_iter(active_pane.read(cx).items().map(|item| item.item_id()));
            assert!(item_ids_in_pane.contains(&center_item.item_id()));
            assert!(item_ids_in_pane.contains(&right_item.item_id()));
            assert!(item_ids_in_pane.contains(&bottom_item.item_id()));
            assert!(item_ids_in_pane.contains(&left_item.item_id()));
            assert!(item_ids_in_pane.contains(&top_item.item_id()));

            // Single pane left: no-op
            workspace.join_pane_into_next(workspace.active_pane().clone(), window, cx)
        });

        workspace.update(cx, |workspace, _cx| {
            let active_pane = workspace.active_pane();
            assert_eq!(top_pane_id, active_pane.entity_id());
        });
    }

    fn add_an_item_to_active_pane(
        cx: &mut VisualTestContext,
        workspace: &Entity<Workspace>,
        item_id: u64,
    ) -> Entity<TestItem> {
        let item = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(
                item_id,
                "item{item_id}.txt",
                cx,
            )])
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item.clone()), None, false, window, cx);
        });
        item
    }

    fn split_pane(cx: &mut VisualTestContext, workspace: &Entity<Workspace>) -> Entity<Pane> {
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Right,
                window,
                cx,
            )
        })
    }

    fn render_center_group(
        workspace: &Workspace,
        window: &mut Window,
        cx: &mut App,
    ) -> pane_group::PaneRenderResult {
        workspace.center.root.render(
            0,
            None,
            None,
            &PaneRenderContext {
                follower_states: &workspace.follower_states,
                active_call: workspace.active_call(),
                active_pane: &workspace.active_pane,
                app_state: &workspace.app_state,
                project: &workspace.project,
                workspace: &workspace.weak_self,
            },
            window,
            cx,
        )
    }

    #[gpui::test]
    async fn test_active_pane_decorations_follow_window_focus(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // Decorations are only painted for splits, so a second pane is needed.
        add_an_item_to_active_pane(cx, &workspace, 1);
        let second_pane = split_pane(cx, &workspace);
        add_an_item_to_active_pane(cx, &workspace, 2);

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 0, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(workspace.active_pane(), &second_pane);
            let result = render_center_group(workspace, window, cx);
            assert!(result.contains_active_pane);
            assert_eq!(
                result.decorated_pane_ix,
                Some(1),
                "the active pane should be decorated while it has focus"
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });
        cx.run_until_parked();
        workspace.update_in(cx, |_, window, cx| {
            assert!(panel.focus_handle(cx).is_focused(window));
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(workspace.active_pane(), &second_pane);
            let result = render_center_group(workspace, window, cx);
            assert!(result.contains_active_pane);
            assert_eq!(
                result.decorated_pane_ix, None,
                "an unfocused group should not decorate its active pane"
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let focus_handle = workspace.active_pane().read(cx).focus_handle(cx);
            window.focus(&focus_handle, cx);
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            let result = render_center_group(workspace, window, cx);
            assert_eq!(result.decorated_pane_ix, Some(1));
        });
    }

    #[gpui::test]
    async fn test_join_all_panes(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, None, cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        add_an_item_to_active_pane(cx, &workspace, 1);
        split_pane(cx, &workspace);
        add_an_item_to_active_pane(cx, &workspace, 2);
        split_pane(cx, &workspace); // empty pane
        split_pane(cx, &workspace);
        let last_item = add_an_item_to_active_pane(cx, &workspace, 3);

        cx.executor().run_until_parked();

        workspace.update(cx, |workspace, cx| {
            let num_panes = workspace.panes().len();
            let num_items_in_current_pane = workspace.active_pane().read(cx).items().count();
            let active_item = workspace
                .active_pane()
                .read(cx)
                .active_item()
                .expect("item is in focus");

            assert_eq!(num_panes, 4);
            assert_eq!(num_items_in_current_pane, 1);
            assert_eq!(active_item.item_id(), last_item.item_id());
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.join_all_panes(window, cx);
        });

        workspace.update(cx, |workspace, cx| {
            let num_panes = workspace.panes().len();
            let num_items_in_current_pane = workspace.active_pane().read(cx).items().count();
            let active_item = workspace
                .active_pane()
                .read(cx)
                .active_item()
                .expect("item is in focus");

            assert_eq!(num_panes, 1);
            assert_eq!(num_items_in_current_pane, 3);
            assert_eq!(active_item.item_id(), last_item.item_id());
        });
    }

    #[gpui::test]
    async fn test_flexible_dock_sizing(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new_flexible(DockPosition::Right, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            workspace.toggle_dock(DockPosition::Right, window, cx);

            let right_dock = workspace.right_dock().clone();
            right_dock.update(cx, |dock, cx| {
                dock.set_panel_size_state(
                    &panel,
                    dock::PanelSizeState {
                        size: None,
                        flex: Some(1.0),
                        weight: None,
                    },
                    cx,
                );
            });
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let item = cx.new(|cx| {
                TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "one.txt", cx)])
            });
            workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
            workspace.bounds.size.width = px(1920.);

            let dock = workspace.right_dock().read(cx);
            let initial_width = workspace
                .dock_size(&dock, window, cx)
                .expect("flexible dock should have an initial width");

            assert_eq!(initial_width, px(960.));
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Right,
                window,
                cx,
            );

            let center_column_count = workspace.center.full_height_column_count();
            assert_eq!(center_column_count, 2);

            let dock = workspace.right_dock().read(cx);
            assert_eq!(workspace.dock_size(&dock, window, cx).unwrap(), px(640.));

            workspace.bounds.size.width = px(2400.);

            let dock = workspace.right_dock().read(cx);
            assert_eq!(workspace.dock_size(&dock, window, cx).unwrap(), px(800.));
        });
    }

    #[gpui::test]
    async fn test_clamp_panel_size_only_skips_flexible_width(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            let bottom_panel = cx.new(|cx| TestPanel::new_flexible(DockPosition::Bottom, 100, cx));
            let right_panel = cx.new(|cx| TestPanel::new_flexible(DockPosition::Right, 100, cx));
            workspace.add_panel(bottom_panel.clone(), window, cx);
            workspace.add_panel(right_panel.clone(), window, cx);

            let max_size = px(200.);
            workspace.bottom_dock().update(cx, |dock, cx| {
                dock.clamp_panel_size(max_size, window, cx);
                assert_eq!(
                    dock.stored_panel_size_state(&bottom_panel)
                        .and_then(|state| state.size),
                    Some(max_size - dock::RESIZE_HANDLE_SIZE)
                );
            });
            workspace.right_dock().update(cx, |dock, cx| {
                dock.clamp_panel_size(max_size, window, cx);
                assert_eq!(
                    dock.stored_panel_size_state(&right_panel)
                        .and_then(|state| state.size),
                    None
                );
            });
        });
    }

    /// Every witness below drives `resize_dock`, which is what the divider drag
    /// and the keyboard resize both call. Asserting against the distribution
    /// directly would pass whether or not anything reached it.
    ///
    /// A wholly-fixed side has the floor twice over, and that redundancy hid a
    /// defect for a day: removing the clamp in `resize_*_dock` alone left this
    /// witness green, because `resize_open_panels` also gives each column
    /// `min + its share of the slack`. Both are load-bearing again by the
    /// 2026-09-05 ruling, which put the drag floor back after three narrower
    /// floors were built and dogfooded and each left a column off-window.
    #[gpui::test]
    async fn test_a_side_drag_stops_where_all_its_columns_still_fit(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
            });

            workspace.resize_dock(DockPosition::Left, px(10.), window, cx);
            let dock = workspace.left_dock().read(cx);
            assert_eq!(
                workspace.dock_size(&dock, window, cx),
                Some(crate::layout_registry::SIDE_COLUMN_MIN_WIDTH),
                "one column: the viewport floor is that column's own minimum, so \
                 the side stops exactly where its single component does"
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(1, window, cx);
            });

            workspace.resize_dock(DockPosition::Left, px(10.), window, cx);
            let dock = workspace.left_dock().read(cx);
            // The whole point of the change, and the half a one-column fixture
            // cannot see: a second column must not raise the floor. Summing is
            // what stopped the divider dead at the contents' width.
            assert_eq!(
                workspace.dock_size(&dock, window, cx),
                Some(crate::layout_registry::SIDE_COLUMN_MIN_WIDTH * 2.),
                "two columns: the floor is summed over them, so neither is left \
                 off-window by a drag"
            );
        });
    }

    /// A declared `min_size` no longer raises a floor on the *side* — nothing does
    /// (Decision 2026-09-05). What it still governs is the width its own column
    /// holds once the side is narrower than its contents, which is the half that
    /// decides whether the component stays usable. That is a draw-time fact:
    /// `min_w` is what flexbox honours, and the stored number says nothing about
    /// it, so this asserts on rendered bounds rather than on `size_state`.
    #[gpui::test]
    async fn test_a_declared_minimum_is_what_its_column_holds_when_the_side_overflows(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        let declared = px(320.);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let declaring =
                cx.new(|cx| TestPanel::new_with_min_size(DockPosition::Left, 100, declared, cx));
            workspace.add_panel(declaring, window, cx);
            let silent = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(silent, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
            });

            // The window-shrink path, not a drag: the drag is floored at the
            // columns' summed width now, so it can no longer produce an overflow.
            // `clamp_panel_size` still has no floor, which is what keeps per-area
            // scrolling reachable at all.
            workspace.left_dock().update(cx, |dock, cx| {
                dock.clamp_panel_size(px(120.), window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let registry_floor = crate::layout_registry::SIDE_COLUMN_MIN_WIDTH;
        let side = cx
            .debug_bounds("DOCK-PANEL-Left")
            .expect("the left side renders");
        let declaring = cx
            .debug_bounds("DOCK-COLUMN-TestPanel")
            .expect("the declaring column renders");
        let silent = cx
            .debug_bounds("DOCK-COLUMN-TestPanelB")
            .expect("the silent column renders");

        assert!(
            declaring.size.width >= declared - px(1.),
            "the column that asked for {declared:?} must hold it; taking the \
             registry floor for every component would draw it at {registry_floor:?} \
             (got {:?})",
            declaring.size.width
        );
        assert!(
            silent.size.width >= registry_floor - px(1.),
            "the silent column must survive at the registry floor rather than \
             absorbing what the declaring one refused to give up (got {:?})",
            silent.size.width
        );
        assert!(
            declaring.size.width + silent.size.width > side.size.width + px(1.),
            "the columns must together exceed the side: that overflow is the thing \
             there is to scroll, and without it the side merely stopped early \
             (columns {:?} + {:?}, side {:?})",
            declaring.size.width,
            silent.size.width,
            side.size.width
        );
    }

    #[gpui::test]
    async fn test_a_side_drag_above_the_floor_is_left_alone(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
            });

            workspace.resize_dock(DockPosition::Left, px(700.), window, cx);
            let dock = workspace.left_dock().read(cx);
            assert_eq!(
                workspace.dock_size(&dock, window, cx),
                Some(px(700.)),
                "a width the components can honour must pass through untouched; a \
                 floor that also raises comfortable widths is over-clamping"
            );
        });
    }

    /// A flexible column reports no stored width, and the *opposite* dock's resize
    /// ceiling is the one place that mattered: reading zero there let one side be
    /// dragged clean over the other. Found by dogfood, not by a test — Tom:
    /// "it lets me drag it past right side bar".
    #[gpui::test]
    async fn test_a_side_cannot_be_dragged_over_a_flexible_opposite_dock(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            // 300px wide by `default_size`, and flexible, so it stores no width.
            let right = cx.new(|cx| TestPanel::new_flexible(DockPosition::Right, 100, cx));
            workspace.add_panel(right, window, cx);
            workspace.right_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
            });

            let left = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(left, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
            });

            workspace.resize_dock(DockPosition::Left, px(1600.), window, cx);
            let dock = workspace.left_dock().read(cx);
            // 1600 window - 300 opposite - 200 editor floor. The 1300 this
            // asserted until 2026-09-05 left the editor nothing: the opposite dock
            // was the only term in the ceiling. Both are in it now.
            assert_eq!(
                workspace.dock_size(&dock, window, cx),
                Some(px(1100.)),
                "the left side was allowed past the right one, or past the \
                 editor's floor; a flexible column reporting no width must still \
                 be counted at its default size, or the ceiling reads the \
                 opposite side as empty"
            );
        });

        // What must survive: a width that clears the opposite dock is untouched.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.resize_dock(DockPosition::Left, px(1000.), window, cx);
            let dock = workspace.left_dock().read(cx);
            assert_eq!(
                workspace.dock_size(&dock, window, cx),
                Some(px(1000.)),
                "a width with room to spare was clamped; the ceiling is firing on \
                 sizes that never reached it"
            );
        });
    }

    /// Two fixed columns on the left, each 400 of an 800px side. The seam is
    /// driven through `resize_dock_seam`, which is what the drag-move listener
    /// calls; `Dock::resize_seam` on its own would skip the geometry entirely.
    /// `x` is an absolute window coordinate, so a target is written as
    /// `bounds.left() + <the width the leading column should end up>`.
    async fn two_fixed_left_columns(
        cx: &mut gpui::TestAppContext,
    ) -> (
        Entity<Workspace>,
        Entity<TestPanel>,
        Entity<TestPanelB>,
        &mut VisualTestContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        let (leading, trailing) = workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);
            let leading = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(leading.clone(), window, cx);
            let trailing = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(trailing.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
            });
            workspace.resize_dock(DockPosition::Left, px(800.), window, cx);
            (leading, trailing)
        });
        (workspace, leading, trailing, cx)
    }

    #[gpui::test]
    async fn test_a_seam_moves_width_between_its_two_neighbours_only(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, leading, trailing, cx) = two_fixed_left_columns(cx).await;

        workspace.update_in(cx, |workspace, window, cx| {
            let dock = workspace.left_dock().read(cx);
            let before = workspace.dock_size(&dock, window, cx);
            assert_eq!(
                before,
                Some(px(800.)),
                "the fixture did not set the side up"
            );

            let left = workspace.bounds.left();
            workspace.resize_dock_seam(DockPosition::Left, 1, left + px(460.), window, cx);

            workspace.left_dock().update(cx, |dock, _cx| {
                assert_eq!(
                    dock.stored_panel_size_state(&leading)
                        .and_then(|state| state.size),
                    Some(px(460.)),
                    "the leading column did not land where the pointer put the seam"
                );
                assert_eq!(
                    dock.stored_panel_size_state(&trailing)
                        .and_then(|state| state.size),
                    Some(px(340.)),
                    "the trailing column did not give up what the leading one gained"
                );
            });

            let dock = workspace.left_dock().read(cx);
            assert_eq!(
                workspace.dock_size(&dock, window, cx),
                before,
                "the side's total width moved; a seam trades inside the side, it \
                 does not resize it"
            );
        });
    }

    #[gpui::test]
    async fn test_a_seam_stops_at_its_neighbours_minimum(cx: &mut gpui::TestAppContext) {
        let (workspace, leading, trailing, cx) = two_fixed_left_columns(cx).await;
        let floor = crate::layout_registry::SIDE_COLUMN_MIN_WIDTH;

        workspace.update_in(cx, |workspace, window, cx| {
            let left = workspace.bounds.left();
            workspace.resize_dock_seam(DockPosition::Left, 1, left + px(800.), window, cx);

            workspace.left_dock().update(cx, |dock, _cx| {
                assert_eq!(
                    dock.stored_panel_size_state(&trailing)
                        .and_then(|state| state.size),
                    Some(floor),
                    "the trailing column was pushed under its own minimum"
                );
                assert_eq!(
                    dock.stored_panel_size_state(&leading)
                        .and_then(|state| state.size),
                    Some(px(800.) - floor),
                    "the leading column took less than the seam had left to give"
                );
            });
        });
    }

    /// The seam is derived from geometry every frame rather than from a
    /// remembered origin, so overshooting a clamp costs nothing: come back and it
    /// is under the pointer again.
    ///
    /// No mutation kills this witness on its own, and that is the honest reading:
    /// under absolute positioning the property falls out of the design rather than
    /// living in a mechanism of its own. It stays because the first build of this
    /// feature *did* remember an origin and jumped for it — `on_drag` hands its
    /// constructor `event.position - hitbox.origin`, an offset inside the handle
    /// rather than a window coordinate — so this is the regression that would come
    /// back if anyone reintroduced drag state.
    #[gpui::test]
    async fn test_a_seam_returns_to_the_pointer_after_being_clamped(cx: &mut gpui::TestAppContext) {
        let (workspace, leading, _trailing, cx) = two_fixed_left_columns(cx).await;

        workspace.update_in(cx, |workspace, window, cx| {
            let left = workspace.bounds.left();
            workspace.resize_dock_seam(DockPosition::Left, 1, left + px(800.), window, cx);
            workspace.resize_dock_seam(DockPosition::Left, 1, left + px(460.), window, cx);

            workspace.left_dock().update(cx, |dock, _cx| {
                assert_eq!(
                    dock.stored_panel_size_state(&leading)
                        .and_then(|state| state.size),
                    Some(px(460.)),
                    "the seam did not come back to the pointer"
                );
            });
        });
    }

    /// Three columns is the smallest side where a seam has anything before it to
    /// account for. With two, the columns preceding the seam sum to zero and a
    /// build that forgets them entirely passes every other witness here.
    #[gpui::test]
    async fn test_a_seam_accounts_for_the_columns_before_it(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);
        set_panel_pinned(cx, "TestPanelC", true);

        let (first, second, third) = workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);
            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first.clone(), window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second.clone(), window, cx);
            let third = cx.new(|cx| TestPanelC::new(DockPosition::Left, 102, cx));
            workspace.add_panel(third.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.activate_panel(2, window, cx);
            });
            workspace.resize_dock(DockPosition::Left, px(900.), window, cx);
            (first, second, third)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let left = workspace.bounds.left();
            // Each column starts at 300, so the pair either side of the second
            // seam is 600 wide and its leading column may be 200..=400. The target
            // is chosen to land *strictly inside* that range: at 650 into the side
            // the answer is 350, while forgetting to subtract the 300 before the
            // seam asks for 650 and the clamp saturates it to 400. An earlier
            // version of this test asked for 400 exactly, which both arms produce,
            // and the mutation that drops `preceding` survived it.
            workspace.resize_dock_seam(DockPosition::Left, 2, left + px(650.), window, cx);

            workspace.left_dock().update(cx, |dock, _cx| {
                assert_eq!(
                    dock.stored_panel_size_state(&first)
                        .and_then(|state| state.size),
                    Some(px(300.)),
                    "a column outside the pair was resized"
                );
                assert_eq!(
                    dock.stored_panel_size_state(&second)
                        .and_then(|state| state.size),
                    Some(px(350.)),
                    "the seam ignored the column before it; without subtracting \
                     that width the target is 650, which the clamp turns into 400"
                );
                assert_eq!(
                    dock.stored_panel_size_state(&third)
                        .and_then(|state| state.size),
                    Some(px(250.)),
                    "the trailing column did not give up exactly what was taken"
                );
            });
        });
    }

    /// The right side docks against the opposite edge, so its seams are measured
    /// from `bounds.right() - dock_total`. A build that uses `bounds.left()` for
    /// both passes every left-side witness.
    #[gpui::test]
    async fn test_a_right_side_seam_is_measured_from_the_right_edge(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        let leading = workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);
            let first = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Right, 101, cx));
            workspace.add_panel(second.clone(), window, cx);
            workspace.right_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
            });
            workspace.resize_dock(DockPosition::Right, px(800.), window, cx);
            // The right dock draws its entries reversed, so the column drawn first
            // is the one added last.
            second
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let dock_left = workspace.bounds.right() - px(800.);
            workspace.resize_dock_seam(DockPosition::Right, 1, dock_left + px(460.), window, cx);

            workspace.right_dock().update(cx, |dock, _cx| {
                assert_eq!(
                    dock.stored_panel_size_state(&leading)
                        .and_then(|state| state.size),
                    Some(px(460.)),
                    "the right side's seam was measured from the wrong edge"
                );
            });
        });
    }

    /// The shape that used to be refused. `agent.flexible` and `terminal.flexible`
    /// both default to `true`, so a flexible neighbour is the common pair rather
    /// than an edge case, and this is the contract that replaced the refusal:
    /// the seam moves, the pair trades width, and the side's total does not budge.
    #[gpui::test]
    async fn test_a_seam_beside_a_flexible_column_trades_width_with_it(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);
            let flexible = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(flexible, window, cx);
            let fixed = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(fixed.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_panel_size_state(
                    &fixed,
                    dock::PanelSizeState {
                        size: Some(px(300.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            workspace.left_dock().update(cx, |dock, inner_cx| {
                assert!(
                    dock.seam_is_resizable(1, window, inner_cx),
                    "a seam next to a flexible column offers a handle now"
                );
            });

            let total_before = workspace
                .dock_size(workspace.left_dock().read(cx), window, cx)
                .expect("the mixed side has a width");

            // The flexible column leads, the fixed one trails. Drag the seam so
            // the flexible column is 200px narrower than it renders today.
            let widths = workspace
                .left_dock()
                .read(cx)
                .rendered_widths(total_before, window, cx);
            let leading_now = widths[0];
            let left = workspace.bounds.left();
            workspace.resize_dock_seam(
                DockPosition::Left,
                1,
                left + leading_now - px(200.),
                window,
                cx,
            );

            let fixed_after = workspace
                .left_dock()
                .read(cx)
                .stored_panel_size_state(&fixed)
                .and_then(|state| state.size)
                .expect("the fixed column still stores a width");
            assert!(
                (fixed_after - (px(300.) + px(200.))).abs() < px(1.5),
                "the fixed column takes the width the flexible one gave up: \
                 expected about {:?}, got {fixed_after:?}",
                px(500.)
            );

            let total_after = workspace
                .dock_size(workspace.left_dock().read(cx), window, cx)
                .expect("the mixed side still has a width");
            assert!(
                (total_after - total_before).abs() < px(1.5),
                "a seam trades width inside the side; it must not move the side's \
                 total, which is what separates it from the editor-facing divider. \
                 Went from {total_before:?} to {total_after:?}"
            );
        });
    }

    /// `ResetOpenDocksSize` reset `visible_panel` — one component per side. That was
    /// the whole side when a side held one component; this feature made sides hold
    /// several, and the command silently stopped covering them. The trailing column
    /// is the one that catches it: a fixture with a single open component passes
    /// against both the old code and the new.
    #[gpui::test]
    async fn test_resetting_a_sides_size_reaches_every_column_on_it(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first.clone(), window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second.clone(), window, cx);

            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
                dock.set_panel_size_state(
                    &first,
                    dock::PanelSizeState {
                        size: Some(px(511.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
                dock.set_panel_size_state(
                    &second,
                    dock::PanelSizeState {
                        size: Some(px(377.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            workspace.reset_open_docks_size(cx);

            let dock = workspace.left_dock().read(cx);
            assert_eq!(
                dock.stored_panel_size_state(&first),
                Some(dock::PanelSizeState::default()),
                "the visible column resets"
            );
            assert_eq!(
                dock.stored_panel_size_state(&second),
                Some(dock::PanelSizeState::default()),
                "and so does the column beside it — the one the old code missed"
            );
        });
    }

    /// Every other sizing witness in this file grades the *model* — `rendered_widths`,
    /// `dock_size`, the stored flex — and every one of them would pass while the panel
    /// inside a flexible column drew at zero width beside a full-width column. A fixed
    /// column gets `w(size)`, so the cached view's `size_full` resolves against a width
    /// the layout engine already knows; a flexible column's width is decided by flexbox
    /// during the same pass, and a percentage against a container that is not yet
    /// resolved is the thing this pins. Found by looking, not by the suite — Tom, on the
    /// two panels that declare `has_flexible_size` and on no others: *"the right side of
    /// the terminal has a grey unfilled area"*.
    #[gpui::test]
    async fn test_a_flexible_column_is_filled_by_the_panel_it_holds(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelC", true);

        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelC::new_flexible(DockPosition::Left, 101, cx));
            workspace.add_panel(second, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
            });
        });
        cx.run_until_parked();

        // The workspace is the window's root view, so the window draws it for us.
        // Re-drawing it through `cx.draw` as a detached element panics the arena:
        // the dock's resize handles are deferred draws, and the second pass tries to
        // prepaint ArenaRefs the first pass already cleared.
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        for (column_selector, panel_selector) in [
            ("DOCK-COLUMN-TestPanel", "TEST-PANEL-ROOT"),
            ("DOCK-COLUMN-TestPanelC", "TEST-PANEL-C-ROOT"),
        ] {
            let column = cx
                .debug_bounds(column_selector)
                .unwrap_or_else(|| panic!("{column_selector} renders"));
            let panel = cx
                .debug_bounds(panel_selector)
                .unwrap_or_else(|| panic!("{panel_selector} renders"));

            assert!(
                column.size.width > px(1.),
                "{column_selector} must have a width for this witness to mean \
                 anything, got {:?}",
                column.size.width
            );
            // Every column after the first carries `border_l_1`, which takes a
            // pixel out of the content box the panel gets. One pixel of slack is
            // that border and nothing else — the failure this pins is a panel
            // drawing at its own content width inside a column sized by flexbox,
            // which is a gap of tens or hundreds of pixels, not one.
            let shortfall = column.size.width - panel.size.width;
            assert!(
                shortfall >= px(0.) && shortfall <= px(1.),
                "{panel_selector} must fill {column_selector}: column {:?}, panel {:?}",
                column.size.width,
                panel.size.width
            );
        }
    }

    /// Piece 1 created a state nothing accounted for: on a side too narrow for its
    /// components, a column can sit outside the viewport. Opening or focusing one
    /// then did its work invisibly — the component took focus somewhere the user
    /// could not see, which reads as a button that did nothing.
    ///
    /// The witness activates the *last* column, the one furthest off-screen, and
    /// asserts it lands inside the side's bounds. The first column would pass
    /// without any reveal at all, since it is already where the viewport starts.
    /// The brief said for days that the bottom row holds its minimums but does not
    /// scroll. It does: `Dock::render` applies `overflow_x_scroll().track_scroll(..)`
    /// **before** it branches on position, so the viewport exists at Bottom exactly
    /// as at the sides, and `activate_panel` calls `scroll_to_item` with no position
    /// gate. What was missing was evidence — the witness above is side-only, so the
    /// bottom half of that mechanism has never been asserted on.
    #[gpui::test]
    async fn test_opening_a_component_scrolls_it_into_view_on_a_narrow_bottom_row(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);
        set_panel_pinned(cx, "TestPanelC", true);

        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Bottom, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Bottom, 101, cx));
            workspace.add_panel(second, window, cx);
            let third = cx.new(|cx| TestPanelC::new(DockPosition::Bottom, 102, cx));
            workspace.add_panel(third, window, cx);
            workspace.bottom_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.activate_panel(2, window, cx);
                dock.set_open(true, window, cx);
            });
        });

        // Three segments at the 160px floor need 480px. The row's width is the
        // window's, so the window is what has to be too narrow — unlike a side,
        // where the dock's own size is the constrained axis.
        cx.simulate_resize(size(px(300.), px(700.)));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .bottom_dock()
                .update(cx, |dock, cx| dock.activate_panel(2, window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let row = cx
            .debug_bounds("DOCK-PANEL-Bottom")
            .expect("the bottom row renders");
        let third = cx
            .debug_bounds("DOCK-COLUMN-TestPanelC")
            .expect("the third segment renders");
        let first = cx
            .debug_bounds("DOCK-COLUMN-TestPanel")
            .expect("the first segment renders");

        // The premise, asserted rather than assumed. Without it every check below
        // passes on a row that simply fits, which is how the side-only witness
        // above was carried as vacuous for a day.
        assert!(
            first.size.width + third.size.width * 2. > row.size.width,
            "the row must be too narrow for its segments, or this proves nothing: \
             row {:?}, segments {:?} each",
            row.size.width,
            third.size.width
        );
        assert!(
            first.origin.x < row.origin.x,
            "and the first segment must actually be scrolled out of view: {:?} vs \
             row {:?}",
            first.origin.x,
            row.origin.x
        );

        assert!(
            third.size.width >= crate::layout_registry::BOTTOM_SEGMENT_MIN_WIDTH - px(1.),
            "the segment must hold its floor rather than squash: got {:?}, floor {:?}",
            third.size.width,
            crate::layout_registry::BOTTOM_SEGMENT_MIN_WIDTH
        );
        assert!(
            third.origin.x >= row.origin.x - px(1.)
                && third.origin.x + third.size.width <= row.origin.x + row.size.width + px(1.),
            "activating the third segment must scroll it into the row's viewport: \
             segment {:?}..{:?}, row {:?}..{:?}",
            third.origin.x,
            third.origin.x + third.size.width,
            row.origin.x,
            row.origin.x + row.size.width
        );
    }

    #[gpui::test]
    async fn test_opening_a_component_scrolls_it_into_view_on_a_narrow_side(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);
        set_panel_pinned(cx, "TestPanelC", true);

        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second, window, cx);
            let third = cx.new(|cx| TestPanelC::new(DockPosition::Left, 102, cx));
            workspace.add_panel(third, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.activate_panel(2, window, cx);
                dock.set_open(true, window, cx);
                // Three columns at the 200px floor cannot fit; the side overflows.
                dock.clamp_panel_size(px(150.), window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .left_dock()
                .update(cx, |dock, cx| dock.activate_panel(2, window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let side = cx
            .debug_bounds("DOCK-PANEL-Left")
            .expect("the left side renders");
        let third = cx
            .debug_bounds("DOCK-COLUMN-TestPanelC")
            .expect("the third column renders");

        assert!(
            third.origin.x >= side.origin.x - px(1.)
                && third.origin.x + third.size.width <= side.origin.x + side.size.width + px(1.),
            "activating the last column must bring it inside the side: side {:?} wide from \
             {:?}, column {:?} wide from {:?}",
            side.size.width,
            side.origin.x,
            third.size.width,
            third.origin.x
        );
    }

    /// A window narrow enough to squeeze a side used to make its components
    /// unreachable: every column was inside `overflow_hidden`, so whatever did not
    /// fit was clipped rather than scrolled to. The drag path cannot get here —
    /// `open_min_size` floors it — so the route in is `clamp_panel_size`, which runs
    /// on window resize and deliberately has no floor of its own.
    ///
    /// The witness is that the columns keep their effective minimums while the side
    /// is narrower than their sum: that overflow is the thing there is to scroll.
    #[gpui::test]
    async fn test_a_side_too_narrow_for_its_components_overflows_instead_of_squashing(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            // Neither declares a `min_size`, so both take the registry's
            // `SIDE_COLUMN_MIN_WIDTH`. That is the case with no `min_w` at all
            // before this change, and the one a fixture declaring minimums hides.
            let first = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(second, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
                // The window-shrink path, not a drag.
                dock.clamp_panel_size(px(120.), window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let floor = crate::layout_registry::SIDE_COLUMN_MIN_WIDTH;
        let mut total = Pixels::ZERO;
        for selector in ["DOCK-COLUMN-TestPanel", "DOCK-COLUMN-TestPanelB"] {
            let column = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} renders"));
            assert!(
                column.size.width >= floor - px(1.),
                "{selector} must hold its effective minimum so the side overflows \
                 rather than squashing: got {:?}, floor {floor:?}",
                column.size.width
            );
            total += column.size.width;
        }

        // Columns at their minimums are only half the claim, and on their own they
        // cannot see a floor on the container: restore one and they still measure
        // 200px each, inside a side widened to hold them exactly. The overflow is
        // what there is to scroll, so it is what has to be asserted.
        let side = cx
            .debug_bounds("DOCK-PANEL-Left")
            .expect("the left side renders");
        assert!(
            total > side.size.width + px(1.),
            "the columns must together exceed the side: columns {total:?}, \
             side {:?}",
            side.size.width
        );
    }

    /// The floor's counterpart, and the hole a cold read found on 2026-09-05 after
    /// the floor was already settled: `CENTER_MIN_WIDTH` is a layout `min_w` with
    /// no term in the drag clamp, so a side dragged near the old
    /// `workspace_width - RESIZE_HANDLE_SIZE` ceiling stored widths the viewport
    /// could not hold. The editor kept its 200px, the dock container shrank to fit,
    /// the columns are `flex_none` at their stored widths and did not — and the
    /// side overflowed and scrolled. A drag reaching the scroll state is exactly
    /// what the 2026-09-05 Decision says cannot happen.
    ///
    /// `test_the_editor_keeps_its_floor_when_a_side_is_dragged_wide` could not see
    /// it: it asserts the editor's width and never the side's. This asserts the
    /// side's, which is the half that was missing.
    #[gpui::test]
    async fn test_a_wide_drag_cannot_store_more_than_the_viewport_can_hold(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let first = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Right, 101, cx));
            workspace.add_panel(second, window, cx);
            workspace.right_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
            });

            // Drag far past anything the window can honour.
            workspace.resize_dock(DockPosition::Right, px(5000.), window, cx);

            let dock = workspace.right_dock().read(cx);
            let size = workspace
                .dock_size(&dock, window, cx)
                .expect("an open dock reports a size");
            let ceiling = px(1600.) - crate::layout_registry::CENTER_MIN_WIDTH;
            assert!(
                size <= ceiling + px(1.),
                "a drag must not store a side wider than the window minus the \
                 editor's floor, or the columns overflow a viewport that shrank \
                 without them: got {size:?}, ceiling {ceiling:?}"
            );
        });
    }
    /// Tom, dogfooding the viewport floor: *"Dragging left works fine. Dragging
    /// right is where the other panes go outside of the window."* Then, on the
    /// cause: *"we don't apply the MIN 200 on the editor pane. I'm guessing we
    /// should."* He was right, and it is why per-area scrolling could not work.
    ///
    /// Every side column carries `min_w`; the editor's chain carries
    /// `overflow_hidden`, which sets `min-width: 0` and makes it the item flexbox
    /// drains first. So a wide side did not overflow *within* its own viewport — it
    /// ate the editor, and once the editor was gone the dock's box ran past the
    /// window edge and its columns rendered off-screen, unreachable by scrolling
    /// because there was no viewport left to scroll in.
    ///
    /// The witness is that the editor holds its floor while a side is dragged wider
    /// than the space left for it. No model-level assertion can see this:
    /// `dock_size` happily reports a side wider than the window.
    #[gpui::test]
    async fn test_the_editor_keeps_its_floor_when_a_side_is_dragged_wide(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Right, 101, cx));
            workspace.add_panel(second, window, cx);
            workspace.right_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
            });
            // Ask for a side far wider than the window can spare — the gesture that
            // stored 480px per column on Tom's 1536px window.
            workspace.resize_dock(DockPosition::Right, px(5000.), window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let editor = cx
            .debug_bounds("EDITOR-REGION")
            .expect("the editor region renders");
        assert!(
            editor.size.width >= crate::layout_registry::CENTER_MIN_WIDTH - px(1.),
            "the editor must keep its floor rather than being drained to pay for \
             the side: got {:?}, floor {:?}",
            editor.size.width,
            crate::layout_registry::CENTER_MIN_WIDTH
        );
    }
    /// Tom, watching the real thing: *"it looks correct initially, but when I make
    /// the width smaller, the grey area grows."* A gap that widens as the side
    /// narrows is not a panel drawing at its content width — that would be a
    /// constant shortfall. It is the dock painting its own background across space
    /// its columns have stopped claiming, which no model-level witness can see
    /// because `rendered_widths` reports what the columns were *told*, not what
    /// flexbox gave them.
    #[gpui::test]
    async fn test_a_narrowed_side_is_still_filled_by_its_columns(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelC", true);

        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelC::new_flexible(DockPosition::Left, 101, cx));
            workspace.add_panel(second, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
            });
        });
        cx.run_until_parked();

        for width in [px(900.), px(700.), px(500.), px(400.)] {
            workspace.update_in(cx, |workspace, window, cx| {
                workspace.resize_dock(DockPosition::Left, width, window, cx);
            });
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();

            let dock = cx
                .debug_bounds("DOCK-PANEL-Left")
                .expect("the left dock renders");
            let first = cx
                .debug_bounds("DOCK-COLUMN-TestPanel")
                .expect("the first column renders");
            let second = cx
                .debug_bounds("DOCK-COLUMN-TestPanelC")
                .expect("the second column renders");

            let claimed = first.size.width + second.size.width;
            let unclaimed = dock.size.width - claimed;
            assert!(
                unclaimed <= px(1.),
                "at side width {width:?} the columns must fill the side: side {:?}, \
                 columns {:?} + {:?} = {claimed:?}, unclaimed {unclaimed:?}",
                dock.size.width,
                first.size.width,
                second.size.width
            );
        }
    }

    /// Two flexible neighbours are the case needing no geometry at all: the fixed
    /// total does not move and the pair's combined share does not either, so the
    /// seam is a pure redistribution of flex between them.
    #[gpui::test]
    async fn test_a_seam_between_two_flexible_columns_redistributes_flex(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelC", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);
            let first = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelC::new_flexible(DockPosition::Left, 101, cx));
            workspace.add_panel(second, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
            });

            let total_before = workspace
                .dock_size(workspace.left_dock().read(cx), window, cx)
                .expect("the side has a width");
            let widths = workspace
                .left_dock()
                .read(cx)
                .rendered_widths(total_before, window, cx);
            let left = workspace.bounds.left();
            workspace.resize_dock_seam(
                DockPosition::Left,
                1,
                left + widths[0] - px(150.),
                window,
                cx,
            );

            let total_after = workspace
                .dock_size(workspace.left_dock().read(cx), window, cx)
                .expect("the side still has a width");
            assert!(
                (total_after - total_before).abs() < px(1.5),
                "two flexible columns trade inside the side: total went from \
                 {total_before:?} to {total_after:?}"
            );

            let after = workspace
                .left_dock()
                .read(cx)
                .rendered_widths(total_after, window, cx);
            assert!(
                (after[0] - (widths[0] - px(150.))).abs() < px(1.5),
                "the leading column gives up 150px: {:?} -> {:?}",
                widths[0],
                after[0]
            );
            assert!(
                (after[1] - (widths[1] + px(150.))).abs() < px(1.5),
                "and the trailing column takes it: {:?} -> {:?}",
                widths[1],
                after[1]
            );
        });
    }

    /// The trap the design note names: flexible columns divide the leftover in
    /// proportion to their flex, so moving a seam that changes the *fixed* total
    /// moves every flexible column — not only the one in the pair — unless each
    /// one's flex is re-derived to hold its width. Three columns, and the seam is
    /// nowhere near the third.
    #[gpui::test]
    async fn test_a_seam_leaves_a_flexible_column_outside_the_pair_alone(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);
        set_panel_pinned(cx, "TestPanelC", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);
            // Order follows activation priority: flexible, fixed, flexible.
            let bystander = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(bystander, window, cx);
            let fixed = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(fixed.clone(), window, cx);
            let partner = cx.new(|cx| TestPanelC::new_flexible(DockPosition::Left, 102, cx));
            workspace.add_panel(partner, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.activate_panel(2, window, cx);
                dock.set_panel_size_state(
                    &fixed,
                    dock::PanelSizeState {
                        size: Some(px(300.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            let total_before = workspace
                .dock_size(workspace.left_dock().read(cx), window, cx)
                .expect("the side has a width");
            let before = workspace
                .left_dock()
                .read(cx)
                .rendered_widths(total_before, window, cx);
            // Prove the fixture before trusting the assertion: three open columns,
            // and the one outside the pair is genuinely flexible. A two-column
            // side cannot express this failure at all.
            assert_eq!(
                before.len(),
                3,
                "this witness needs three open columns; got {before:?}"
            );
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .open_flex(None, window, cx)
                    .is_some_and(|total| total > 1.0),
                "two flexible columns must be open, or the bystander is not one"
            );

            // Seam 2 sits between the fixed column and the trailing flexible one,
            // so it changes the fixed total. Column 0 is not in the pair.
            let left = workspace.bounds.left();
            workspace.resize_dock_seam(
                DockPosition::Left,
                2,
                left + before[0] + before[1] + px(120.),
                window,
                cx,
            );

            let total_after = workspace
                .dock_size(workspace.left_dock().read(cx), window, cx)
                .expect("the side still has a width");
            let after = workspace
                .left_dock()
                .read(cx)
                .rendered_widths(total_after, window, cx);

            assert!(
                (after[0] - before[0]).abs() < px(1.5),
                "the flexible column outside the pair must not move: {:?} -> {:?}. \
                 Its flex has to be re-derived against the new fixed total, or the \
                 leftover it shares changes underneath it",
                before[0],
                after[0]
            );
            assert!(
                (total_after - total_before).abs() < px(1.5),
                "and the side's total holds: {total_before:?} -> {total_after:?}"
            );
        });
    }

    /// A side holding *both* a fixed and a flexible column is what upstream's math
    /// never had to describe: with one panel per dock a dock was wholly fixed or
    /// wholly flexible, so `fixed` was zero whenever flex mattered. These three
    /// pin the mixed case in both directions.
    ///
    /// The pre-rework code shares the whole workspace width among flex factors and
    /// then adds `fixed` on top, so a mixed side claims its own fixed pixels twice.
    #[gpui::test]
    async fn test_a_mixed_side_does_not_claim_its_fixed_pixels_twice(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            // Left: one flexible column (flex 1.0) beside one fixed column at 300px.
            let flexible = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(flexible, window, cx);
            let fixed = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(fixed.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_panel_size_state(
                    &fixed,
                    dock::PanelSizeState {
                        size: Some(px(300.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            // Right: one flexible column, so the opposite side contributes flex and
            // this exercises the branch that used to divide the whole width.
            let right = cx.new(|cx| TestPanel::new_flexible(DockPosition::Right, 102, cx));
            workspace.add_panel(right, window, cx);
            workspace.toggle_dock(DockPosition::Right, window, cx);

            let left_dock = workspace.left_dock().read(cx);
            let width = workspace
                .dock_size(&left_dock, window, cx)
                .expect("an open mixed left dock has a width");

            // available = 1600 - 300 (own fixed) - 0 (right is wholly flexible)
            // share     = 1/(1 + 1 centre + 1 right) * 1300
            // width     = 300 + 433.33..
            let expected = px(300.) + px(1300.) / 3.;
            assert!(
                (width - expected).abs() < px(0.5),
                "a mixed side must share only what its fixed columns leave: \
                 expected about {expected:?}, got {width:?}. Dividing the whole \
                 workspace width and then adding `fixed` gives {:?}",
                px(300.) + px(1600.) / 3.
            );
        });
    }

    /// `dock_size` and `dock_flex_for_size` are inverses, and this is the identity
    /// that says so: laying a dock out and asking what flex produces that layout
    /// must return the flex it already has. The pre-rework inverse omits `fixed`
    /// from both its numerator and its denominator, so it fails here by roughly
    /// the ratio the fixed column contributes.
    #[gpui::test]
    async fn test_dock_size_and_dock_flex_for_size_round_trip_on_a_mixed_side(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let flexible = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(flexible, window, cx);
            let fixed = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(fixed.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_panel_size_state(
                    &fixed,
                    dock::PanelSizeState {
                        size: Some(px(420.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            let right = cx.new(|cx| TestPanel::new_flexible(DockPosition::Right, 102, cx));
            workspace.add_panel(right, window, cx);
            workspace.toggle_dock(DockPosition::Right, window, cx);

            let left_dock = workspace.left_dock().read(cx);
            let width = workspace
                .dock_size(&left_dock, window, cx)
                .expect("an open mixed left dock has a width");
            let stored_flex = left_dock
                .open_flex(workspace.default_dock_flex(DockPosition::Left), window, cx)
                .expect("a mixed left dock has a flexible column");

            let derived = workspace
                .dock_flex_for_size(DockPosition::Left, width, window, cx)
                .expect("a horizontal dock derives a flex");

            assert!(
                (derived - stored_flex).abs() < 0.01,
                "laying the dock out at {width:?} and asking what flex produces it \
                 must return the flex it already has: expected {stored_flex}, got \
                 {derived}. A round trip that does not close means one direction \
                 counts the fixed column and the other does not"
            );
        });
    }

    /// The branch that was already correct — an opposite side with no flexible
    /// column — must not move. `competing_flex` is the centre alone there, which
    /// is what the replaced `flex / (flex + centre) * available` computed.
    #[gpui::test]
    async fn test_a_mixed_side_is_unchanged_when_the_opposite_side_is_wholly_fixed(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let flexible = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(flexible, window, cx);
            let fixed = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(fixed.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_panel_size_state(
                    &fixed,
                    dock::PanelSizeState {
                        size: Some(px(300.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            // Right holds a fixed column, so it contributes width but no flex.
            let right = cx.new(|cx| TestPanelC::new(DockPosition::Right, 102, cx));
            workspace.add_panel(right.clone(), window, cx);
            workspace.toggle_dock(DockPosition::Right, window, cx);
            workspace.right_dock().update(cx, |dock, cx| {
                dock.set_panel_size_state(
                    &right,
                    dock::PanelSizeState {
                        size: Some(px(200.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            let left_dock = workspace.left_dock().read(cx);
            let width = workspace
                .dock_size(&left_dock, window, cx)
                .expect("an open mixed left dock has a width");

            // available = 1600 - 300 (own fixed) - 200 (opposite fixed) = 1100
            // share     = 1/(1 + 1 centre) * 1100
            let expected = px(300.) + px(1100.) / 2.;
            assert!(
                (width - expected).abs() < px(0.5),
                "the wholly-fixed-opposite branch must be unchanged: expected about \
                 {expected:?}, got {width:?}"
            );
        });
    }

    /// Dragging the editor-facing divider on a mixed side leaves the fixed column
    /// alone and moves the whole change into the flexible one (Tom, 2026-09-03).
    /// The wholly-fixed side keeps its slack distribution, which the floor
    /// witnesses above pin; this is the case where the two rules differ.
    #[gpui::test]
    async fn test_a_fixed_column_holds_its_width_beside_a_flexible_one(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let flexible = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(flexible.clone(), window, cx);
            let fixed = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(fixed.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_panel_size_state(
                    &fixed,
                    dock::PanelSizeState {
                        size: Some(px(300.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            let before = workspace
                .dock_size(workspace.left_dock().read(cx), window, cx)
                .expect("the mixed side has a width");
            let flex_before = workspace
                .left_dock()
                .read(cx)
                .stored_panel_size_state(&flexible)
                .and_then(|state| state.flex)
                .unwrap_or(1.0);

            workspace.resize_dock(DockPosition::Left, before + px(120.), window, cx);

            let fixed_after = workspace
                .left_dock()
                .read(cx)
                .stored_panel_size_state(&fixed)
                .and_then(|state| state.size);
            assert_eq!(
                fixed_after,
                Some(px(300.)),
                "the editor-facing divider must not rewrite a pinned width; the \
                 internal seam is the control for that"
            );

            let flex_after = workspace
                .left_dock()
                .read(cx)
                .stored_panel_size_state(&flexible)
                .and_then(|state| state.flex)
                .expect("the flexible column still stores a flex");
            assert!(
                flex_after > flex_before,
                "the flexible column must absorb the whole change: flex went from \
                 {flex_before} to {flex_after}"
            );

            let after = workspace
                .dock_size(workspace.left_dock().read(cx), window, cx)
                .expect("the mixed side still has a width");
            assert!(
                (after - (before + px(120.))).abs() < px(1.),
                "the side must actually reach the requested width: asked for \
                 {:?}, got {after:?}",
                before + px(120.)
            );
        });
    }

    /// Toggling a column from fixed to flexible must leave it the width it had.
    /// `test_flexible_panel_left_dock_sizing` cannot witness this: it reads the
    /// stored flex back and checks the layout agrees with it, which is true of any
    /// self-consistent pair of numbers. This pins the value.
    #[gpui::test]
    async fn test_toggling_a_mixed_column_to_flexible_keeps_its_width(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let flexible = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(flexible, window, cx);
            let fixed = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(fixed.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_panel_size_state(
                    &fixed,
                    dock::PanelSizeState {
                        size: Some(px(360.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            let dock = workspace.left_dock().clone();
            workspace.toggle_dock_panel_flexible_size(&dock, &fixed, window, cx);

            let stored = dock
                .read(cx)
                .stored_panel_size_state(&fixed)
                .expect("the toggled column keeps a size state");
            let flex = stored
                .flex
                .expect("it is flexible now, so it stores a flex");

            // Its own 360px is what becomes the flexible share, and releasing it
            // grows the space that share competes for: available goes from
            // 1600 - 360 - 0 to 1600, and the sibling flexible column plus the
            // centre are what it competes with.
            //   flex = 360 * (1 centre + 1 sibling) / (1600 - 360)
            let expected = 360.0 * 2.0 / (1600.0 - 360.0);
            assert!(
                (flex - expected).abs() < 0.01,
                "a column becoming flexible keeps its width: expected flex about \
                 {expected}, got {flex}. Leaving it counted as fixed while deriving \
                 its own share is what makes this drift"
            );
        });
    }

    /// A fixed column beside a flexible one holds its stored width (Decision
    /// 2026-09-03, which the 2026-09-05 scroll ruling leaves standing — it removed
    /// the stop-dead floor, not fixed-means-fixed). So the drag can take away only
    /// what the flexible column contributes, and the side bottoms out at the fixed
    /// column's stored width rather than at anything summed from minimums.
    ///
    /// This is not the old floor wearing a new name. The old one refused to go
    /// below what every column needed; this one is a consequence of one column
    /// declining to move, and the flexible column beside it is still 200px wide at
    /// draw time inside a side that no longer has room for it.
    #[gpui::test]
    async fn test_a_mixed_side_gives_up_only_what_its_flexible_column_holds(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let flexible = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(flexible, window, cx);
            let fixed = cx.new(|cx| TestPanelB::new(DockPosition::Left, 101, cx));
            workspace.add_panel(fixed.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_panel_size_state(
                    &fixed,
                    dock::PanelSizeState {
                        size: Some(px(300.)),
                        flex: None,
                        weight: None,
                    },
                    cx,
                );
            });

            workspace.resize_dock(DockPosition::Left, px(10.), window, cx);
            let dock = workspace.left_dock().read(cx);
            let size = workspace
                .dock_size(&dock, window, cx)
                .expect("an open dock reports a size");
            assert_eq!(
                size,
                px(300.) + crate::layout_registry::SIDE_COLUMN_MIN_WIDTH,
                "the floor must pay for the fixed column's stored 300px, not its \
                 200px minimum: summing minimums floors 100px too low and the side \
                 shrinks into a gap its contents cannot occupy"
            );
        });
    }

    /// One column on the side, so the viewport floor *is* that column's minimum
    /// and the side stops there — there is no second column to scroll to, so
    /// going further would only hide the one component behind a sliver. The
    /// multi-column case is the interesting one and lives above: the floor is the
    /// widest single minimum, well below their sum.
    #[gpui::test]
    async fn test_a_flexible_left_component_drag_floors_at_its_one_whole_column(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let panel = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(panel, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
            });

            workspace.resize_dock(DockPosition::Left, px(10.), window, cx);
            let dock = workspace.left_dock().read(cx);
            assert_eq!(
                workspace.dock_size(&dock, window, cx),
                Some(crate::layout_registry::SIDE_COLUMN_MIN_WIDTH),
                "a lone column's minimum is the viewport floor: below it the side \
                 is a sliver onto the only thing it holds, which is what dogfooding \
                 the unfloored build found"
            );
        });
    }

    #[gpui::test]
    async fn test_a_flexible_right_component_drag_floors_at_its_one_whole_column(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.width = px(1600.);

            let panel = cx.new(|cx| TestPanel::new_flexible(DockPosition::Right, 100, cx));
            workspace.add_panel(panel, window, cx);
            workspace.right_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
            });

            workspace.resize_dock(DockPosition::Right, px(10.), window, cx);
            let dock = workspace.right_dock().read(cx);
            assert_eq!(
                workspace.dock_size(&dock, window, cx),
                Some(crate::layout_registry::SIDE_COLUMN_MIN_WIDTH),
                "the right side needs its own witness; the left one passed while \
                 the right clamp was missing entirely"
            );
        });
    }

    /// The bottom row divided its width equally whatever the user had set, and
    /// `min_w_0` let a narrow window squash every segment to nothing — the sliver
    /// the registry floor exists to prevent, still live on this axis long after the
    /// sides were fixed. Both are draw-time facts, so both are asserted on bounds.
    #[gpui::test]
    async fn test_bottom_segments_keep_their_weights_and_their_minimum(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| TestPanel::new(DockPosition::Bottom, 100, cx));
            workspace.add_panel(first.clone(), window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Bottom, 101, cx));
            workspace.add_panel(second.clone(), window, cx);
            workspace.bottom_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
                // Three parts to one, and both weights present — which is the
                // only shape a seam produces: `resize_row_seam` writes fractions
                // summing to 1 for *every* open entry. Leaving the second absent
                // used to give it the old 1.0 default and made this a test of the
                // default rather than of the saved weights, which is why the
                // 2026-09-07 change to that default broke it.
                dock.set_panel_size_state(
                    &first,
                    dock::PanelSizeState {
                        size: None,
                        flex: None,
                        weight: Some(0.75),
                    },
                    cx,
                );
                dock.set_panel_size_state(
                    &second,
                    dock::PanelSizeState {
                        size: None,
                        flex: None,
                        weight: Some(0.25),
                    },
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let wide = cx
            .debug_bounds("DOCK-COLUMN-TestPanel")
            .expect("the weighted segment renders");
        let narrow = cx
            .debug_bounds("DOCK-COLUMN-TestPanelB")
            .expect("the unweighted segment renders");
        assert!(
            wide.size.width > narrow.size.width * 2.,
            "saved weights of 0.75 against 0.25 must draw wider than double; equal \
             shares would draw them the same (got {:?} and {:?})",
            wide.size.width,
            narrow.size.width
        );
        let floor = crate::layout_registry::BOTTOM_SEGMENT_MIN_WIDTH;
        assert!(
            narrow.size.width >= floor - px(1.),
            "the lighter segment must still hold the registry bottom floor rather \
             than being squeezed toward nothing (got {:?}, floor {floor:?})",
            narrow.size.width
        );
    }

    /// The bottom row drew `cursor_col_resize` seam handles that did nothing:
    /// `Dock::render` adds one on every axis, while `resize_dock_seam` returned
    /// early for Bottom. A control that looks live and does nothing is the defect
    /// class this feature already fixed twice elsewhere.
    ///
    /// The seam works in weight space, because the row has no px to store — it
    /// divides whatever width its `bottom_dock_layout` geometry gives it.
    #[gpui::test]
    async fn test_a_bottom_seam_moves_width_between_its_two_segments(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        set_panel_pinned(cx, "TestPanel", true);
        set_panel_pinned(cx, "TestPanelB", true);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds = gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::size(px(1600.), px(900.)),
            };

            let first = cx.new(|cx| TestPanel::new(DockPosition::Bottom, 100, cx));
            workspace.add_panel(first, window, cx);
            let second = cx.new(|cx| TestPanelB::new(DockPosition::Bottom, 101, cx));
            workspace.add_panel(second, window, cx);
            workspace.bottom_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
            });

            // Equal weights to start: the seam sits at 800 on a 1600 row.
            workspace.resize_dock_seam(DockPosition::Bottom, 1, px(500.), window, cx);

            let dock = workspace.bottom_dock().read(cx);
            let widths = dock.rendered_widths(px(1600.), window, cx);
            assert_eq!(widths.len(), 2, "two open segments");
            assert!(
                (widths[0] - px(500.)).abs() < px(2.),
                "the leading segment must take the seam's target (got {:?})",
                widths[0]
            );
            assert!(
                (widths[0] + widths[1] - px(1600.)).abs() < px(2.),
                "the row's total must be untouched — a seam trades between its two \
                 neighbours and moves nothing else (got {:?} + {:?})",
                widths[0],
                widths[1]
            );
        });
    }
    /// The bottom dock resizes on height, while the registry publishes a bottom
    /// *segment width* for the row's internal dividers. No height floor has been
    /// designed, so this records the gap rather than letting it read as covered.
    #[gpui::test]
    async fn test_the_bottom_dock_drag_stops_at_its_height_floor(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.height = px(1000.);

            // The panel declares 250px, and the floor is 120px. The declared number
            // is deliberately ignored here: `Panel::min_size` is a width — Agent
            // returns `Some` at the sides and `None` at Bottom for exactly this
            // reason — so a bottom component cannot raise its own height floor.
            let panel =
                cx.new(|cx| TestPanel::new_with_min_size(DockPosition::Bottom, 100, px(250.), cx));
            workspace.add_panel(panel, window, cx);
            workspace.bottom_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
            });

            workspace.resize_dock(DockPosition::Bottom, px(1.), window, cx);
            let dock = workspace.bottom_dock().read(cx);
            assert_eq!(
                workspace.dock_size(&dock, window, cx),
                Some(crate::layout_registry::BOTTOM_DOCK_MIN_HEIGHT),
                "the drag stops at the floor instead of the 6px sliver it used to reach"
            );
        });
    }

    /// The floor must not win against a window too short to hold it. Floor then
    /// ceiling is the order both side drags use, and the reason is the same: a dock
    /// that overflows its window to satisfy a minimum is worse than a short one.
    /// The width half of this was found by dogfooding on 2026-09-05 — the editor
    /// had no minimum while every side column had one, so flexbox drained it to pay
    /// for them. The fix went onto the four geometries as `min_w`, and the bottom
    /// dock resizes on *height*, so the identical hole stayed open on the axis that
    /// session never touched. Dragging the divider to the top took the editor to
    /// nothing.
    #[gpui::test]
    async fn test_the_bottom_drag_leaves_the_editor_its_height(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let height = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Bottom, 100, cx));
            workspace.add_panel(panel, window, cx);
            workspace.bottom_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.set_open(true, window, cx);
            });
            // Ask for a dock taller than the window, which is what dragging the
            // divider to the top asks for.
            workspace.resize_dock(DockPosition::Bottom, px(5000.), window, cx);
            let dock = workspace.bottom_dock().read(cx);
            workspace.dock_size(&dock, window, cx).unwrap()
        });

        let workspace_height = workspace.update(cx, |workspace, _| workspace.bounds.size.height);
        assert!(
            workspace_height - height >= crate::layout_registry::CENTER_MIN_HEIGHT,
            "the drag must leave the editor its floor rather than storing a height \
             the window cannot spare: dock {height:?} of {workspace_height:?}, floor {:?}",
            crate::layout_registry::CENTER_MIN_HEIGHT
        );
    }

    /// The drag is only one route in. `clamp_panel_size` on window shrink carries no
    /// floor by design, so the render has to hold the editor's height itself — and
    /// on the width axis that floor shipped with no witness at all, found only by
    /// mutation. This is the one it was missing, on the other axis.
    #[gpui::test]
    async fn test_the_editor_keeps_its_height_when_the_bottom_dock_is_tall(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Bottom, 100, cx));
            workspace.add_panel(panel, window, cx);
            workspace.bottom_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.set_open(true, window, cx);
                // Straight past the drag clamp into the stored state, the way a
                // window shrink reaches it.
                dock.resize_active_panel(Some(px(5000.)), None, window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let editor = cx
            .debug_bounds("EDITOR-REGION")
            .expect("the editor region renders");
        assert!(
            editor.size.height >= crate::layout_registry::CENTER_MIN_HEIGHT - px(1.),
            "the editor must keep its floor rather than being drained to pay for the \
             bottom dock: got {:?}, floor {:?}",
            editor.size.height,
            crate::layout_registry::CENTER_MIN_HEIGHT
        );
    }

    #[gpui::test]
    async fn test_a_short_window_beats_the_bottom_height_floor(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.bounds.size.height = px(80.);

            let panel = cx.new(|cx| TestPanel::new(DockPosition::Bottom, 100, cx));
            workspace.add_panel(panel, window, cx);
            workspace.bottom_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
            });

            workspace.resize_dock(DockPosition::Bottom, px(1.), window, cx);
            let dock = workspace.bottom_dock().read(cx);
            let height = workspace.dock_size(&dock, window, cx).unwrap();
            assert!(
                height < crate::layout_registry::BOTTOM_DOCK_MIN_HEIGHT,
                "an 80px window cannot hold a 120px floor, and the window wins: got {height:?}"
            );
        });
    }

    #[gpui::test]
    async fn test_panel_size_state_persistence(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        // Fixed-width panel: pixel size is persisted to KVP and restored on re-add.
        {
            let project = Project::test(fs.clone(), [], cx).await;
            let (multi_workspace, cx) =
                cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
            let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

            workspace.update(cx, |workspace, _cx| {
                workspace.set_random_database_id();
                workspace.bounds.size.width = px(800.);
            });

            let panel = workspace.update_in(cx, |workspace, window, cx| {
                let panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
                workspace.add_panel(panel.clone(), window, cx);
                workspace.toggle_dock(DockPosition::Left, window, cx);
                panel
            });

            workspace.update_in(cx, |workspace, window, cx| {
                workspace.resize_left_dock(px(350.), window, cx);
            });

            cx.run_until_parked();

            let persisted = workspace.read_with(cx, |workspace, cx| {
                workspace.persisted_panel_size_state(TestPanel::panel_key(), cx)
            });
            assert_eq!(
                persisted.and_then(|s| s.size),
                Some(px(350.)),
                "fixed-width panel size should be persisted to KVP"
            );

            // Remove the panel and re-add a fresh instance with the same key.
            // The new instance should have its size state restored from KVP.
            workspace.update_in(cx, |workspace, window, cx| {
                workspace.remove_panel(&panel, window, cx);
            });

            workspace.update_in(cx, |workspace, window, cx| {
                let new_panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
                workspace.add_panel(new_panel, window, cx);

                let left_dock = workspace.left_dock().read(cx);
                let size_state = left_dock
                    .panel::<TestPanel>()
                    .and_then(|p| left_dock.stored_panel_size_state(&p));
                assert_eq!(
                    size_state.and_then(|s| s.size),
                    Some(px(350.)),
                    "re-added fixed-width panel should restore persisted size from KVP"
                );
            });
        }

        // Flexible panel: both pixel size and ratio are persisted and restored.
        {
            let project = Project::test(fs.clone(), [], cx).await;
            let (multi_workspace, cx) =
                cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
            let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

            workspace.update(cx, |workspace, _cx| {
                workspace.set_random_database_id();
                workspace.bounds.size.width = px(800.);
            });

            let panel = workspace.update_in(cx, |workspace, window, cx| {
                let item = cx.new(|cx| {
                    TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "one.txt", cx)])
                });
                workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);

                let panel = cx.new(|cx| TestPanel::new_flexible(DockPosition::Right, 100, cx));
                workspace.add_panel(panel.clone(), window, cx);
                workspace.toggle_dock(DockPosition::Right, window, cx);
                panel
            });

            workspace.update_in(cx, |workspace, window, cx| {
                workspace.resize_right_dock(px(300.), window, cx);
            });

            cx.run_until_parked();

            let persisted = workspace
                .read_with(cx, |workspace, cx| {
                    workspace.persisted_panel_size_state(TestPanel::panel_key(), cx)
                })
                .expect("flexible panel state should be persisted to KVP");
            assert_eq!(
                persisted.size, None,
                "flexible panel should not persist a redundant pixel size"
            );
            let original_ratio = persisted.flex.expect("panel's flex should be persisted");

            // Remove the panel and re-add: both size and ratio should be restored.
            workspace.update_in(cx, |workspace, window, cx| {
                workspace.remove_panel(&panel, window, cx);
            });

            workspace.update_in(cx, |workspace, window, cx| {
                let new_panel = cx.new(|cx| TestPanel::new_flexible(DockPosition::Right, 100, cx));
                workspace.add_panel(new_panel, window, cx);

                let right_dock = workspace.right_dock().read(cx);
                let size_state = right_dock
                    .panel::<TestPanel>()
                    .and_then(|p| right_dock.stored_panel_size_state(&p))
                    .expect("re-added flexible panel should have restored size state from KVP");
                assert_eq!(
                    size_state.size, None,
                    "re-added flexible panel should not have a persisted pixel size"
                );
                assert_eq!(
                    size_state.flex,
                    Some(original_ratio),
                    "re-added flexible panel should restore persisted flex"
                );
            });
        }
    }

    #[gpui::test]
    async fn test_default_size_change_resets_saved_size(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs.clone(), [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
            workspace.bounds.size.width = px(800.);
        });

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            workspace.toggle_dock(DockPosition::Left, window, cx);
            panel
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.resize_left_dock(px(350.), window, cx);
        });

        cx.run_until_parked();

        let dock_size = workspace.read_with(cx, |workspace, cx| {
            workspace
                .left_dock()
                .read(cx)
                .panel::<TestPanel>()
                .and_then(|panel| {
                    workspace
                        .left_dock()
                        .read(cx)
                        .stored_panel_size_state(&panel)
                })
                .and_then(|s| s.size)
        });
        assert_eq!(dock_size, Some(px(350.)));

        panel.update(cx, |panel, cx| {
            panel.default_size = px(500.);
            cx.update_global::<SettingsStore, _>(|_, _| {});
        });

        cx.run_until_parked();

        let dock_size = workspace.read_with(cx, |workspace, cx| {
            workspace
                .left_dock()
                .read(cx)
                .panel::<TestPanel>()
                .and_then(|panel| {
                    workspace
                        .left_dock()
                        .read(cx)
                        .stored_panel_size_state(&panel)
                })
                .and_then(|s| s.size)
        });
        assert_eq!(
            dock_size, None,
            "saved size should be cleared after default_size changes"
        );
    }

    #[gpui::test]
    async fn test_flexible_panel_left_dock_sizing(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        workspace.update(cx, |workspace, _cx| {
            workspace.bounds.size.width = px(900.);
        });

        // Step 1: Add a tab to the center pane then open a flexible panel in the left
        // dock. With one full-width center pane the default ratio is 0.5, so the panel
        // and the center pane each take half the workspace width.
        workspace.update_in(cx, |workspace, window, cx| {
            let item = cx.new(|cx| {
                TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "one.txt", cx)])
            });
            workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);

            let panel = cx.new(|cx| TestPanel::new_flexible(DockPosition::Left, 100, cx));
            workspace.add_panel(panel, window, cx);
            workspace.toggle_dock(DockPosition::Left, window, cx);

            let left_dock = workspace.left_dock().read(cx);
            let left_width = workspace
                .dock_size(&left_dock, window, cx)
                .expect("left dock should have an active panel");

            assert_eq!(
                left_width,
                workspace.bounds.size.width / 2.,
                "flexible left panel should split evenly with the center pane"
            );
        });

        // Step 2: Split the center pane left/right. The flexible panel is treated as one
        // average center column, so with two center columns it should take one third of
        // the workspace width.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Right,
                window,
                cx,
            );

            let left_dock = workspace.left_dock().read(cx);
            let left_width = workspace
                .dock_size(&left_dock, window, cx)
                .expect("left dock should still have an active panel after horizontal split");

            assert_eq!(
                left_width,
                workspace.bounds.size.width / 3.,
                "flexible left panel width should match the average center column width"
            );
        });

        // Step 3: Split the active center pane vertically (top/bottom). Vertical splits do
        // not change the number of center columns, so the flexible panel width stays the same.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.split_pane(
                workspace.active_pane().clone(),
                SplitDirection::Down,
                window,
                cx,
            );

            let left_dock = workspace.left_dock().read(cx);
            let left_width = workspace
                .dock_size(&left_dock, window, cx)
                .expect("left dock should still have an active panel after vertical split");

            assert_eq!(
                left_width,
                workspace.bounds.size.width / 3.,
                "flexible left panel width should still match the average center column width"
            );
        });

        // Step 4: Open a fixed-width panel in the right dock. The right dock's default
        // size reduces the available width, so the flexible left panel keeps matching one
        // average center column within the remaining space.
        workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 200, cx));
            workspace.add_panel(panel, window, cx);
            workspace.toggle_dock(DockPosition::Right, window, cx);

            let right_dock = workspace.right_dock().read(cx);
            let right_width = workspace
                .dock_size(&right_dock, window, cx)
                .expect("right dock should have an active panel");

            let left_dock = workspace.left_dock().read(cx);
            let left_width = workspace
                .dock_size(&left_dock, window, cx)
                .expect("left dock should still have an active panel");

            let available_width = workspace.bounds.size.width - right_width;
            assert_eq!(
                left_width,
                available_width / 3.,
                "flexible left panel should keep matching one average center column"
            );
        });

        // Step 5: Toggle the right dock's panel to flexible. Now both docks use
        // column-equivalent flex sizing and the workspace width is divided among
        // left-flex, two center columns, and right-flex.
        workspace.update_in(cx, |workspace, window, cx| {
            let right_dock = workspace.right_dock().clone();
            let right_panel = right_dock
                .read(cx)
                .visible_panel()
                .expect("right dock should have a visible panel")
                .clone();
            workspace.toggle_dock_panel_flexible_size(
                &right_dock,
                right_panel.as_ref(),
                window,
                cx,
            );

            let right_dock = right_dock.read(cx);
            let right_panel = right_dock
                .visible_panel()
                .expect("right dock should still have a visible panel");
            assert!(
                right_panel.has_flexible_size(window, cx),
                "right panel should now be flexible"
            );

            let right_size_state = right_dock
                .stored_panel_size_state(right_panel.as_ref())
                .expect("right panel should have a stored size state after toggling");
            let right_flex = right_size_state
                .flex
                .expect("right panel should have a flex value after toggling");

            let left_dock = workspace.left_dock().read(cx);
            let left_width = workspace
                .dock_size(&left_dock, window, cx)
                .expect("left dock should still have an active panel");
            let right_width = workspace
                .dock_size(&right_dock, window, cx)
                .expect("right dock should still have an active panel");

            let left_flex = workspace
                .default_dock_flex(DockPosition::Left)
                .expect("left dock should have a default flex");
            let center_column_count = workspace.center.full_height_column_count() as f32;

            let total_flex = left_flex + center_column_count + right_flex;
            let expected_left = left_flex / total_flex * workspace.bounds.size.width;
            let expected_right = right_flex / total_flex * workspace.bounds.size.width;
            assert_eq!(
                left_width, expected_left,
                "flexible left panel should share workspace width via flex ratios"
            );
            assert_eq!(
                right_width, expected_right,
                "flexible right panel should share workspace width via flex ratios"
            );
        });
    }

    struct TestModal(FocusHandle);

    impl TestModal {
        fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
            Self(cx.focus_handle())
        }
    }

    impl EventEmitter<DismissEvent> for TestModal {}

    impl Focusable for TestModal {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.0.clone()
        }
    }

    impl ModalView for TestModal {}

    impl Render for TestModal {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<TestModal>,
        ) -> impl IntoElement {
            div().track_focus(&self.0)
        }
    }

    // Registers its focus handle as a reopenable picker on construction, like a real
    // `Picker` does, so the modal layer recognizes it by focus identity.
    struct ReopenableTestModal(FocusHandle);

    impl ReopenableTestModal {
        fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
            let focus_handle = cx.focus_handle();
            register_reopenable_picker(&focus_handle, cx);
            Self(focus_handle)
        }
    }

    impl EventEmitter<DismissEvent> for ReopenableTestModal {}

    impl Focusable for ReopenableTestModal {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.0.clone()
        }
    }

    impl ModalView for ReopenableTestModal {}

    impl Render for ReopenableTestModal {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<ReopenableTestModal>,
        ) -> impl IntoElement {
            div().track_focus(&self.0)
        }
    }

    #[gpui::test]
    async fn test_reopen_last_picker(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // A non-reopenable modal is dropped on dismissal and cannot be revealed.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_modal(window, cx, TestModal::new);
        });
        cx.executor().run_until_parked();
        assert!(workspace.read_with(cx, |workspace, cx| {
            workspace.active_modal::<TestModal>(cx).is_some()
        }));
        workspace.update_in(cx, |workspace, window, cx| {
            let revealed = workspace.modal_layer.update(cx, |modal_layer, cx| {
                modal_layer.hide_modal(window, cx);
                modal_layer.reveal_stashed_modal(window, cx)
            });
            assert!(!revealed, "a non-reopenable modal should not be revealable");
        });

        // A reopenable modal is stashed on dismissal and revealed as the *same*
        // entity, so its prior state is preserved exactly.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_modal(window, cx, ReopenableTestModal::new);
        });
        cx.executor().run_until_parked();
        let original_id = workspace.read_with(cx, |workspace, cx| {
            workspace
                .active_modal::<ReopenableTestModal>(cx)
                .unwrap()
                .entity_id()
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .modal_layer
                .update(cx, |modal_layer, cx| modal_layer.hide_modal(window, cx));
        });
        cx.executor().run_until_parked();
        assert!(workspace.read_with(cx, |workspace, cx| {
            workspace.active_modal::<ReopenableTestModal>(cx).is_none()
        }));
        workspace.update_in(cx, |workspace, window, cx| {
            let revealed = workspace.modal_layer.update(cx, |modal_layer, cx| {
                modal_layer.reveal_stashed_modal(window, cx)
            });
            assert!(revealed, "a reopenable modal should be revealable");
        });
        cx.executor().run_until_parked();
        let revealed_id = workspace.read_with(cx, |workspace, cx| {
            workspace
                .active_modal::<ReopenableTestModal>(cx)
                .unwrap()
                .entity_id()
        });
        assert_eq!(
            original_id, revealed_id,
            "reveal should restore the same modal entity rather than building a new one"
        );

        workspace.update_in(cx, |workspace, window, cx| {
            let revealed = workspace.modal_layer.update(cx, |modal_layer, cx| {
                modal_layer.reveal_stashed_modal(window, cx)
            });
            assert!(!revealed, "reveal should be a no-op while a modal is open");
        });

        // A non-reopenable modal must not discard the stash, which is what lets the
        // command palette be used to trigger the reopen.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .modal_layer
                .update(cx, |modal_layer, cx| modal_layer.hide_modal(window, cx));
            workspace.toggle_modal(window, cx, TestModal::new);
        });
        cx.executor().run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            let revealed = workspace.modal_layer.update(cx, |modal_layer, cx| {
                modal_layer.hide_modal(window, cx);
                modal_layer.reveal_stashed_modal(window, cx)
            });
            assert!(
                revealed,
                "a non-reopenable modal must not discard the stash"
            );
        });
        cx.executor().run_until_parked();

        // Reopen triggered from within a modal that dismisses asynchronously and
        // dispatches the action in the same cycle, as the command palette does.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .modal_layer
                .update(cx, |modal_layer, cx| modal_layer.hide_modal(window, cx));
            workspace.toggle_modal(window, cx, TestModal::new);
        });
        cx.executor().run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            // Mirror the command palette's confirm: emit DismissEvent on itself and
            // dispatch the reopen action within the same update.
            let palette = workspace.active_modal::<TestModal>(cx).unwrap();
            palette.update(cx, |_, cx| cx.emit(DismissEvent));
            workspace.reopen_last_picker(&ReopenLastPicker, window, cx);
        });
        cx.executor().run_until_parked();
        assert!(
            workspace.read_with(cx, |workspace, cx| workspace
                .active_modal::<ReopenableTestModal>(cx)
                .is_some()),
            "reopen triggered from within a dismissing modal should reveal the stash"
        );
    }

    #[gpui::test]
    async fn test_reopen_last_picker_with_active_leader_modal(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // Stash a reopenable modal.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_modal(window, cx, ReopenableTestModal::new);
        });
        cx.executor().run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace
                .modal_layer
                .update(cx, |modal_layer, cx| modal_layer.hide_modal(window, cx));
        });
        cx.executor().run_until_parked();

        // A non-reopenable modal (e.g. the which-key popup) is already
        // active when the reopen fires, and it dismisses *after* the action rather
        // than before it (which-key dismisses when pending input clears).
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_modal(window, cx, TestModal::new);
        });
        cx.executor().run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            // Reopen fires first (chord completes and dispatches the action)...
            workspace.reopen_last_picker(&ReopenLastPicker, window, cx);
            // ...then the modal dismisses
            let modal = workspace.active_modal::<TestModal>(cx).unwrap();
            modal.update(cx, |_, cx| cx.emit(DismissEvent));
        });
        cx.executor().run_until_parked();
        assert!(
            workspace.read_with(cx, |workspace, cx| workspace
                .active_modal::<ReopenableTestModal>(cx)
                .is_some()),
            "reopen with an active modal that dismisses after the action should reveal the stash"
        );
    }

    #[gpui::test]
    async fn test_panels(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        let (panel_1, panel_2) = workspace.update_in(cx, |workspace, window, cx| {
            let panel_1 = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            workspace.add_panel(panel_1.clone(), window, cx);
            workspace.toggle_dock(DockPosition::Left, window, cx);
            // A distinct component type, for the same reason as above.
            let panel_2 = cx.new(|cx| TestPanelB::new(DockPosition::Right, 101, cx));
            workspace.add_panel(panel_2.clone(), window, cx);
            workspace.toggle_dock(DockPosition::Right, window, cx);

            let left_dock = workspace.left_dock();
            assert_eq!(
                left_dock.read(cx).visible_panel().unwrap().panel_id(),
                panel_1.panel_id()
            );
            assert_eq!(
                workspace.dock_size(&left_dock.read(cx), window, cx),
                Some(px(300.))
            );

            workspace.resize_left_dock(px(1337.), window, cx);
            assert_eq!(
                workspace
                    .right_dock()
                    .read(cx)
                    .visible_panel()
                    .unwrap()
                    .panel_id(),
                panel_2.panel_id(),
            );

            (panel_1, panel_2)
        });

        // Move panel_1 to the right
        panel_1.update_in(cx, |panel_1, window, cx| {
            panel_1.set_position(DockPosition::Right, window, cx)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            // Since panel_1 was visible on the left, it should now be visible now that it's been moved to the right.
            // Since it was the only panel on the left, the left dock should now be closed.
            assert!(!workspace.left_dock().read(cx).is_open());
            assert!(workspace.left_dock().read(cx).visible_panel().is_none());
            let right_dock = workspace.right_dock();
            assert_eq!(
                right_dock.read(cx).visible_panel().unwrap().panel_id(),
                panel_1.panel_id()
            );
            assert_eq!(
                right_dock
                    .read(cx)
                    .active_panel_size()
                    .unwrap()
                    .size
                    .unwrap(),
                px(1337.)
            );

            // Now we move panel_2 to the left
            panel_2.set_position(DockPosition::Left, window, cx);
        });

        workspace.update(cx, |workspace, cx| {
            // Since panel_2 was not visible on the right, we don't open the left dock.
            assert!(!workspace.left_dock().read(cx).is_open());
            // And the right dock is unaffected in its displaying of panel_1
            assert!(workspace.right_dock().read(cx).is_open());
            assert_eq!(
                workspace
                    .right_dock()
                    .read(cx)
                    .visible_panel()
                    .unwrap()
                    .panel_id(),
                panel_1.panel_id(),
            );
        });

        // Move panel_1 back to the left
        panel_1.update_in(cx, |panel_1, window, cx| {
            panel_1.set_position(DockPosition::Left, window, cx)
        });

        workspace.update_in(cx, |workspace, window, cx| {
            // Since panel_1 was visible on the right, we open the left dock and make panel_1 active.
            let left_dock = workspace.left_dock();
            assert!(left_dock.read(cx).is_open());
            assert_eq!(
                left_dock.read(cx).visible_panel().unwrap().panel_id(),
                panel_1.panel_id()
            );
            assert_eq!(
                workspace.dock_size(&left_dock.read(cx), window, cx),
                Some(px(1337.))
            );
            // And the right dock should be closed as it no longer has any panels.
            assert!(!workspace.right_dock().read(cx).is_open());

            // Now we move panel_1 to the bottom
            panel_1.set_position(DockPosition::Bottom, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            // Since panel_1 was visible on the left, we close the left dock.
            assert!(!workspace.left_dock().read(cx).is_open());
            // The bottom dock is sized based on the panel's default size,
            // since the panel orientation changed from vertical to horizontal.
            let bottom_dock = workspace.bottom_dock();
            assert_eq!(
                workspace.dock_size(&bottom_dock.read(cx), window, cx),
                Some(px(300.))
            );
            // Close bottom dock and move panel_1 back to the left.
            bottom_dock.update(cx, |bottom_dock, cx| {
                bottom_dock.set_open(false, window, cx)
            });
            panel_1.set_position(DockPosition::Left, window, cx);
        });

        // Emit activated event on panel 1
        panel_1.update(cx, |_, cx| cx.emit(PanelEvent::Activate));

        // Now the left dock is open and panel_1 is active and focused.
        workspace.update_in(cx, |workspace, window, cx| {
            let left_dock = workspace.left_dock();
            assert!(left_dock.read(cx).is_open());
            assert_eq!(
                left_dock.read(cx).visible_panel().unwrap().panel_id(),
                panel_1.panel_id(),
            );
            assert!(panel_1.focus_handle(cx).is_focused(window));
        });

        // Emit closed event on panel 2, which is not active
        panel_2.update(cx, |_, cx| cx.emit(PanelEvent::Close));

        // Wo don't close the left dock, because panel_2 wasn't the active panel
        workspace.update(cx, |workspace, cx| {
            let left_dock = workspace.left_dock();
            assert!(left_dock.read(cx).is_open());
            assert_eq!(
                left_dock.read(cx).visible_panel().unwrap().panel_id(),
                panel_1.panel_id(),
            );
        });

        // Emitting a ZoomIn event shows the panel as zoomed.
        panel_1.update(cx, |_, cx| cx.emit(PanelEvent::ZoomIn));
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.zoomed, Some(panel_1.to_any().downgrade()));
            assert_eq!(workspace.zoomed_position, Some(DockPosition::Left));
        });

        // Move panel to another dock while it is zoomed. This goes through the
        // panel handle, the way a placement gesture does: `component_positions`
        // is authoritative, so writing only the panel's own dock setting would
        // be shadowed by the shared position set earlier in this test.
        workspace.update_in(cx, |_, window, cx| {
            panel_1.set_position(DockPosition::Right, window, cx);
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.zoomed, Some(panel_1.to_any().downgrade()));

            assert_eq!(workspace.zoomed_position, Some(DockPosition::Right));
        });

        // This is a helper for getting a:
        // - valid focus on an element,
        // - that isn't a part of the panes and panels system of the Workspace,
        // - and doesn't trigger the 'on_focus_lost' API.
        let focus_other_view = {
            let workspace = workspace.clone();
            move |cx: &mut VisualTestContext| {
                workspace.update_in(cx, |workspace, window, cx| {
                    if workspace.active_modal::<TestModal>(cx).is_some() {
                        workspace.toggle_modal(window, cx, TestModal::new);
                        workspace.toggle_modal(window, cx, TestModal::new);
                    } else {
                        workspace.toggle_modal(window, cx, TestModal::new);
                    }
                })
            }
        };

        // If focus is transferred to another view that's not a panel or another pane, we still show
        // the panel as zoomed.
        focus_other_view(cx);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.zoomed, Some(panel_1.to_any().downgrade()));
            assert_eq!(workspace.zoomed_position, Some(DockPosition::Right));
        });

        // If focus is transferred elsewhere in the workspace, the panel is no longer zoomed.
        workspace.update_in(cx, |_workspace, window, cx| {
            cx.focus_self(window);
        });
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.zoomed, None);
            assert_eq!(workspace.zoomed_position, None);
        });

        // If focus is transferred again to another view that's not a panel or a pane, we won't
        // show the panel as zoomed because it wasn't zoomed before.
        focus_other_view(cx);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.zoomed, None);
            assert_eq!(workspace.zoomed_position, None);
        });

        // When the panel is activated, it is zoomed again.
        cx.dispatch_action(ToggleRightDock);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.zoomed, Some(panel_1.to_any().downgrade()));
            assert_eq!(workspace.zoomed_position, Some(DockPosition::Right));
        });

        // Emitting a ZoomOut event unzooms the panel.
        panel_1.update(cx, |_, cx| cx.emit(PanelEvent::ZoomOut));
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.zoomed, None);
            assert_eq!(workspace.zoomed_position, None);
        });

        // Emit closed event on panel 1, which is active
        panel_1.update(cx, |_, cx| cx.emit(PanelEvent::Close));

        // Now the left dock is closed, because panel_1 was the active panel
        workspace.update(cx, |workspace, cx| {
            let right_dock = workspace.right_dock();
            assert!(!right_dock.read(cx).is_open());
        });
    }

    #[gpui::test]
    async fn test_no_save_prompt_when_multi_buffer_dirty_items_closed(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.background_executor.clone());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        let dirty_regular_buffer = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_label("1.txt")
                .with_project_items(&[dirty_project_item(1, "1.txt", cx)])
        });
        let dirty_regular_buffer_2 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_label("2.txt")
                .with_project_items(&[dirty_project_item(2, "2.txt", cx)])
        });
        let dirty_multi_buffer_with_both = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_buffer_kind(ItemBufferKind::Multibuffer)
                .with_label("Fake Project Search")
                .with_project_items(&[
                    dirty_regular_buffer.read(cx).project_items[0].clone(),
                    dirty_regular_buffer_2.read(cx).project_items[0].clone(),
                ])
        });
        let multi_buffer_with_both_files_id = dirty_multi_buffer_with_both.item_id();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item(
                pane.clone(),
                Box::new(dirty_regular_buffer.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
            workspace.add_item(
                pane.clone(),
                Box::new(dirty_regular_buffer_2.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
            workspace.add_item(
                pane.clone(),
                Box::new(dirty_multi_buffer_with_both.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
        });

        pane.update_in(cx, |pane, window, cx| {
            pane.activate_item(2, true, true, window, cx);
            assert_eq!(
                pane.active_item().unwrap().item_id(),
                multi_buffer_with_both_files_id,
                "Should select the multi buffer in the pane"
            );
        });
        let close_all_but_multi_buffer_task = pane.update_in(cx, |pane, window, cx| {
            pane.close_other_items(
                &CloseOtherItems {
                    save_intent: Some(SaveIntent::Save),
                    close_pinned: true,
                },
                None,
                window,
                cx,
            )
        });
        cx.background_executor.run_until_parked();
        assert!(!cx.has_pending_prompt());
        close_all_but_multi_buffer_task
            .await
            .expect("Closing all buffers but the multi buffer failed");
        pane.update(cx, |pane, cx| {
            assert_eq!(dirty_regular_buffer.read(cx).save_count, 1);
            assert_eq!(dirty_multi_buffer_with_both.read(cx).save_count, 0);
            assert_eq!(dirty_regular_buffer_2.read(cx).save_count, 1);
            assert_eq!(pane.items_len(), 1);
            assert_eq!(
                pane.active_item().unwrap().item_id(),
                multi_buffer_with_both_files_id,
                "Should have only the multi buffer left in the pane"
            );
            assert!(
                dirty_multi_buffer_with_both.read(cx).is_dirty,
                "The multi buffer containing the unsaved buffer should still be dirty"
            );
        });

        dirty_regular_buffer.update(cx, |buffer, cx| {
            buffer.project_items[0].update(cx, |pi, _| pi.is_dirty = true)
        });

        let close_multi_buffer_task = pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(
                &CloseActiveItem {
                    save_intent: Some(SaveIntent::Close),
                    close_pinned: false,
                },
                window,
                cx,
            )
        });
        cx.background_executor.run_until_parked();
        assert!(
            cx.has_pending_prompt(),
            "Dirty multi buffer should prompt a save dialog"
        );
        cx.simulate_prompt_answer("Save");
        cx.background_executor.run_until_parked();
        close_multi_buffer_task
            .await
            .expect("Closing the multi buffer failed");
        pane.update(cx, |pane, cx| {
            assert_eq!(
                dirty_multi_buffer_with_both.read(cx).save_count,
                1,
                "Multi buffer item should get be saved"
            );
            // Test impl does not save inner items, so we do not assert them
            assert_eq!(
                pane.items_len(),
                0,
                "No more items should be left in the pane"
            );
            assert!(pane.active_item().is_none());
        });
    }

    #[gpui::test]
    async fn test_save_prompt_when_dirty_multi_buffer_closed_with_some_of_its_dirty_items_not_present_in_the_pane(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);

        let fs = FakeFs::new(cx.background_executor.clone());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        let dirty_regular_buffer = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_label("1.txt")
                .with_project_items(&[dirty_project_item(1, "1.txt", cx)])
        });
        let dirty_regular_buffer_2 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_label("2.txt")
                .with_project_items(&[dirty_project_item(2, "2.txt", cx)])
        });
        let clear_regular_buffer = cx.new(|cx| {
            TestItem::new(cx)
                .with_label("3.txt")
                .with_project_items(&[TestProjectItem::new(3, "3.txt", cx)])
        });

        let dirty_multi_buffer_with_both = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_buffer_kind(ItemBufferKind::Multibuffer)
                .with_label("Fake Project Search")
                .with_project_items(&[
                    dirty_regular_buffer.read(cx).project_items[0].clone(),
                    dirty_regular_buffer_2.read(cx).project_items[0].clone(),
                    clear_regular_buffer.read(cx).project_items[0].clone(),
                ])
        });
        let multi_buffer_with_both_files_id = dirty_multi_buffer_with_both.item_id();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item(
                pane.clone(),
                Box::new(dirty_regular_buffer.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
            workspace.add_item(
                pane.clone(),
                Box::new(dirty_multi_buffer_with_both.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
        });

        pane.update_in(cx, |pane, window, cx| {
            pane.activate_item(1, true, true, window, cx);
            assert_eq!(
                pane.active_item().unwrap().item_id(),
                multi_buffer_with_both_files_id,
                "Should select the multi buffer in the pane"
            );
        });
        let _close_multi_buffer_task = pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(
                &CloseActiveItem {
                    save_intent: None,
                    close_pinned: false,
                },
                window,
                cx,
            )
        });
        cx.background_executor.run_until_parked();
        assert!(
            cx.has_pending_prompt(),
            "With one dirty item from the multi buffer not being in the pane, a save prompt should be shown"
        );
    }

    /// Tests that when `close_on_file_delete` is enabled, files are automatically
    /// closed when they are deleted from disk.
    #[gpui::test]
    async fn test_close_on_disk_deletion_enabled(cx: &mut TestAppContext) {
        init_test(cx);

        // Enable the close_on_disk_deletion setting
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings.workspace.close_on_file_delete = Some(true);
            });
        });

        let fs = FakeFs::new(cx.background_executor.clone());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        // Create a test item that simulates a file
        let item = cx.new(|cx| {
            TestItem::new(cx)
                .with_label("test.txt")
                .with_project_items(&[TestProjectItem::new(1, "test.txt", cx)])
        });

        // Add item to workspace
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item(
                pane.clone(),
                Box::new(item.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
        });

        // Verify the item is in the pane
        pane.read_with(cx, |pane, _| {
            assert_eq!(pane.items().count(), 1);
        });

        // Simulate file deletion by setting the item's deleted state
        item.update(cx, |item, _| {
            item.set_has_deleted_file(true);
        });

        // Emit UpdateTab event to trigger the close behavior
        cx.run_until_parked();
        item.update(cx, |_, cx| {
            cx.emit(ItemEvent::UpdateTab);
        });

        // Allow the close operation to complete
        cx.run_until_parked();

        // Verify the item was automatically closed
        pane.read_with(cx, |pane, _| {
            assert_eq!(
                pane.items().count(),
                0,
                "Item should be automatically closed when file is deleted"
            );
        });
    }

    /// Tests that when `close_on_file_delete` is disabled (default), files remain
    /// open with a strikethrough when they are deleted from disk.
    #[gpui::test]
    async fn test_close_on_disk_deletion_disabled(cx: &mut TestAppContext) {
        init_test(cx);

        // Ensure close_on_disk_deletion is disabled (default)
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings.workspace.close_on_file_delete = Some(false);
            });
        });

        let fs = FakeFs::new(cx.background_executor.clone());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        // Create a test item that simulates a file
        let item = cx.new(|cx| {
            TestItem::new(cx)
                .with_label("test.txt")
                .with_project_items(&[TestProjectItem::new(1, "test.txt", cx)])
        });

        // Add item to workspace
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item(
                pane.clone(),
                Box::new(item.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
        });

        // Verify the item is in the pane
        pane.read_with(cx, |pane, _| {
            assert_eq!(pane.items().count(), 1);
        });

        // Simulate file deletion
        item.update(cx, |item, _| {
            item.set_has_deleted_file(true);
        });

        // Emit UpdateTab event
        cx.run_until_parked();
        item.update(cx, |_, cx| {
            cx.emit(ItemEvent::UpdateTab);
        });

        // Allow any potential close operation to complete
        cx.run_until_parked();

        // Verify the item remains open (with strikethrough)
        pane.read_with(cx, |pane, _| {
            assert_eq!(
                pane.items().count(),
                1,
                "Item should remain open when close_on_disk_deletion is disabled"
            );
        });

        // Verify the item shows as deleted
        item.read_with(cx, |item, _| {
            assert!(
                item.has_deleted_file,
                "Item should be marked as having deleted file"
            );
        });
    }

    /// Tests that dirty files are not automatically closed when deleted from disk,
    /// even when `close_on_file_delete` is enabled. This ensures users don't lose
    /// unsaved changes without being prompted.
    #[gpui::test]
    async fn test_close_on_disk_deletion_with_dirty_file(cx: &mut TestAppContext) {
        init_test(cx);

        // Enable the close_on_file_delete setting
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings.workspace.close_on_file_delete = Some(true);
            });
        });

        let fs = FakeFs::new(cx.background_executor.clone());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        // Create a dirty test item
        let item = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_label("test.txt")
                .with_project_items(&[TestProjectItem::new(1, "test.txt", cx)])
        });

        // Add item to workspace
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item(
                pane.clone(),
                Box::new(item.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
        });

        // Simulate file deletion
        item.update(cx, |item, _| {
            item.set_has_deleted_file(true);
        });

        // Emit UpdateTab event to trigger the close behavior
        cx.run_until_parked();
        item.update(cx, |_, cx| {
            cx.emit(ItemEvent::UpdateTab);
        });

        // Allow any potential close operation to complete
        cx.run_until_parked();

        // Verify the item remains open (dirty files are not auto-closed)
        pane.read_with(cx, |pane, _| {
            assert_eq!(
                pane.items().count(),
                1,
                "Dirty items should not be automatically closed even when file is deleted"
            );
        });

        // Verify the item is marked as deleted and still dirty
        item.read_with(cx, |item, _| {
            assert!(
                item.has_deleted_file,
                "Item should be marked as having deleted file"
            );
            assert!(item.is_dirty, "Item should still be dirty");
        });
    }

    /// Tests that navigation history is cleaned up when files are auto-closed
    /// due to deletion from disk.
    #[gpui::test]
    async fn test_close_on_disk_deletion_cleans_navigation_history(cx: &mut TestAppContext) {
        init_test(cx);

        // Enable the close_on_file_delete setting
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings.workspace.close_on_file_delete = Some(true);
            });
        });

        let fs = FakeFs::new(cx.background_executor.clone());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        // Create test items
        let item1 = cx.new(|cx| {
            TestItem::new(cx)
                .with_label("test1.txt")
                .with_project_items(&[TestProjectItem::new(1, "test1.txt", cx)])
        });
        let item1_id = item1.item_id();

        let item2 = cx.new(|cx| {
            TestItem::new(cx)
                .with_label("test2.txt")
                .with_project_items(&[TestProjectItem::new(2, "test2.txt", cx)])
        });

        // Add items to workspace
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item(
                pane.clone(),
                Box::new(item1.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
            workspace.add_item(
                pane.clone(),
                Box::new(item2.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
        });

        // Activate item1 to ensure it gets navigation entries
        pane.update_in(cx, |pane, window, cx| {
            pane.activate_item(0, true, true, window, cx);
        });

        // Switch to item2 and back to create navigation history
        pane.update_in(cx, |pane, window, cx| {
            pane.activate_item(1, true, true, window, cx);
        });
        cx.run_until_parked();

        pane.update_in(cx, |pane, window, cx| {
            pane.activate_item(0, true, true, window, cx);
        });
        cx.run_until_parked();

        // Simulate file deletion for item1
        item1.update(cx, |item, _| {
            item.set_has_deleted_file(true);
        });

        // Emit UpdateTab event to trigger the close behavior
        item1.update(cx, |_, cx| {
            cx.emit(ItemEvent::UpdateTab);
        });
        cx.run_until_parked();

        // Verify item1 was closed
        pane.read_with(cx, |pane, _| {
            assert_eq!(
                pane.items().count(),
                1,
                "Should have 1 item remaining after auto-close"
            );
        });

        // Check navigation history after close
        let has_item = pane.read_with(cx, |pane, cx| {
            let mut has_item = false;
            pane.nav_history().for_each_entry(cx, &mut |entry, _| {
                if entry.item.id() == item1_id {
                    has_item = true;
                }
            });
            has_item
        });

        assert!(
            !has_item,
            "Navigation history should not contain closed item entries"
        );
    }

    #[gpui::test]
    async fn test_reopen_closed_item_skips_items_without_paths(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.background_executor.clone());

        let project = Project::test(fs, [], cx).await;

        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        let reopenable_item = cx.new(TestItem::new);

        let active_item = cx.new(TestItem::new);
        let unreopenable_item = cx.new(TestItem::new);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(
                Box::new(reopenable_item.clone()),
                None,
                true,
                window,
                cx,
            );
            workspace.add_item_to_active_pane(
                Box::new(active_item.clone()),
                None,
                true,
                window,
                cx,
            );
        });

        pane.update(cx, |pane, _| {
            pane.nav_history_mut().set_mode(NavigationMode::ClosingItem);
        });

        reopenable_item.update_in(cx, |item, window, cx| {
            item.deactivated(window, cx);
        });

        pane.update(cx, |pane, _| {
            pane.nav_history_mut().set_mode(NavigationMode::Normal);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(
                Box::new(unreopenable_item.clone()),
                None,
                true,
                window,
                cx,
            );
        });

        pane.update_in(cx, |pane, window, cx| {
            pane.close_item_by_id(unreopenable_item.item_id(), SaveIntent::Skip, window, cx)
                .detach_and_log_err(cx);
        });

        cx.run_until_parked();

        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.reopen_closed_item(window, cx)
            })
            .await
            .unwrap();

        pane.read_with(cx, |pane, _| {
            assert_eq!(
                pane.active_item().unwrap().item_id(),
                reopenable_item.item_id()
            );
        });
    }

    #[gpui::test]
    async fn test_no_save_prompt_when_dirty_multi_buffer_closed_with_all_of_its_dirty_items_present_in_the_pane(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);

        let fs = FakeFs::new(cx.background_executor.clone());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        let dirty_regular_buffer = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_label("1.txt")
                .with_project_items(&[dirty_project_item(1, "1.txt", cx)])
        });
        let dirty_regular_buffer_2 = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_label("2.txt")
                .with_project_items(&[dirty_project_item(2, "2.txt", cx)])
        });
        let clear_regular_buffer = cx.new(|cx| {
            TestItem::new(cx)
                .with_label("3.txt")
                .with_project_items(&[TestProjectItem::new(3, "3.txt", cx)])
        });

        let dirty_multi_buffer = cx.new(|cx| {
            TestItem::new(cx)
                .with_dirty(true)
                .with_buffer_kind(ItemBufferKind::Multibuffer)
                .with_label("Fake Project Search")
                .with_project_items(&[
                    dirty_regular_buffer.read(cx).project_items[0].clone(),
                    dirty_regular_buffer_2.read(cx).project_items[0].clone(),
                    clear_regular_buffer.read(cx).project_items[0].clone(),
                ])
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item(
                pane.clone(),
                Box::new(dirty_regular_buffer.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
            workspace.add_item(
                pane.clone(),
                Box::new(dirty_regular_buffer_2.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
            workspace.add_item(
                pane.clone(),
                Box::new(dirty_multi_buffer.clone()),
                None,
                false,
                false,
                window,
                cx,
            );
        });

        pane.update_in(cx, |pane, window, cx| {
            pane.activate_item(2, true, true, window, cx);
            assert_eq!(
                pane.active_item().unwrap().item_id(),
                dirty_multi_buffer.item_id(),
                "Should select the multi buffer in the pane"
            );
        });
        let close_multi_buffer_task = pane.update_in(cx, |pane, window, cx| {
            pane.close_active_item(
                &CloseActiveItem {
                    save_intent: None,
                    close_pinned: false,
                },
                window,
                cx,
            )
        });
        cx.background_executor.run_until_parked();
        assert!(
            !cx.has_pending_prompt(),
            "All dirty items from the multi buffer are in the pane still, no save prompts should be shown"
        );
        close_multi_buffer_task
            .await
            .expect("Closing multi buffer failed");
        pane.update(cx, |pane, cx| {
            assert_eq!(dirty_regular_buffer.read(cx).save_count, 0);
            assert_eq!(dirty_multi_buffer.read(cx).save_count, 0);
            assert_eq!(dirty_regular_buffer_2.read(cx).save_count, 0);
            assert_eq!(
                pane.items()
                    .map(|item| item.item_id())
                    .sorted()
                    .collect::<Vec<_>>(),
                vec![
                    dirty_regular_buffer.item_id(),
                    dirty_regular_buffer_2.item_id(),
                ],
                "Should have no multi buffer left in the pane"
            );
            assert!(dirty_regular_buffer.read(cx).is_dirty);
            assert!(dirty_regular_buffer_2.read(cx).is_dirty);
        });
    }

    #[gpui::test]
    async fn test_move_focused_panel_to_next_position(cx: &mut gpui::TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        // Add a new panel to the right dock, opening the dock and setting the
        // focus to the new panel.
        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);

            workspace
                .right_dock()
                .update(cx, |right_dock, cx| right_dock.set_open(true, window, cx));

            workspace.toggle_panel_focus::<TestPanel>(window, cx);

            panel
        });

        // Dispatch the `MoveFocusedPanelToNextPosition` action, moving the
        // panel to the next valid position which, in this case, is the left
        // dock.
        cx.dispatch_action(MoveFocusedPanelToNextPosition);
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.left_dock().read(cx).is_open());
            assert_eq!(panel.read(cx).position, DockPosition::Left);
        });

        // Dispatch the `MoveFocusedPanelToNextPosition` action, moving the
        // panel to the next valid position which, in this case, is the bottom
        // dock.
        cx.dispatch_action(MoveFocusedPanelToNextPosition);
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.bottom_dock().read(cx).is_open());
            assert_eq!(panel.read(cx).position, DockPosition::Bottom);
        });

        // Dispatch the `MoveFocusedPanelToNextPosition` action again, this time
        // around moving the panel to its initial position, the right dock.
        cx.dispatch_action(MoveFocusedPanelToNextPosition);
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert_eq!(panel.read(cx).position, DockPosition::Right);
        });

        // Remove focus from the panel, ensuring that, if the panel is not
        // focused, the `MoveFocusedPanelToNextPosition` action does not update
        // the panel's position, so the panel is still in the right dock.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        cx.dispatch_action(MoveFocusedPanelToNextPosition);
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert_eq!(panel.read(cx).position, DockPosition::Right);
        });
    }

    #[gpui::test]
    async fn test_active_pane_updates_to_focus_target_on_removal(cx: &mut TestAppContext) {
        assert_active_pane_is_replaced_after_removal(cx, true).await;
    }

    #[gpui::test]
    async fn test_active_pane_updates_to_fallback_on_removal(cx: &mut TestAppContext) {
        assert_active_pane_is_replaced_after_removal(cx, false).await;
    }

    async fn assert_active_pane_is_replaced_after_removal(
        cx: &mut TestAppContext,
        use_focus_target: bool,
    ) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        workspace.update_in(cx, |workspace, window, cx| {
            let first_pane = workspace.active_pane().clone();
            let second_pane =
                workspace.split_pane(first_pane.clone(), SplitDirection::Right, window, cx);
            workspace.set_active_pane(&second_pane, window, cx);

            let focus_target = use_focus_target.then(|| first_pane.clone());
            workspace.remove_pane(second_pane, focus_target, window, cx);

            assert_eq!(workspace.active_pane(), &first_pane);
            assert!(
                workspace
                    .panes()
                    .iter()
                    .any(|pane| pane == workspace.active_pane()),
                "active pane should be one of the remaining workspace panes"
            );
        });
    }

    #[gpui::test]
    async fn test_moving_items_create_panes(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let item_1 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "first.txt", cx)])
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item_1), None, true, window, cx);
            workspace.move_item_to_pane_in_direction(
                &MoveItemToPaneInDirection {
                    direction: SplitDirection::Right,
                    focus: true,
                    clone: false,
                },
                window,
                cx,
            );
            workspace.move_item_to_pane_at_index(
                &MoveItemToPane {
                    destination: 3,
                    focus: true,
                    clone: false,
                },
                window,
                cx,
            );

            assert_eq!(workspace.panes.len(), 1, "No new panes were created");
            assert_eq!(
                pane_items_paths(&workspace.active_pane, cx),
                vec!["first.txt".to_string()],
                "Single item was not moved anywhere"
            );
        });

        let item_2 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(2, "second.txt", cx)])
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item_2), None, true, window, cx);
            assert_eq!(
                pane_items_paths(&workspace.panes[0], cx),
                vec!["first.txt".to_string(), "second.txt".to_string()],
            );
            workspace.move_item_to_pane_in_direction(
                &MoveItemToPaneInDirection {
                    direction: SplitDirection::Right,
                    focus: true,
                    clone: false,
                },
                window,
                cx,
            );

            assert_eq!(workspace.panes.len(), 2, "A new pane should be created");
            assert_eq!(
                pane_items_paths(&workspace.panes[0], cx),
                vec!["first.txt".to_string()],
                "After moving, one item should be left in the original pane"
            );
            assert_eq!(
                pane_items_paths(&workspace.panes[1], cx),
                vec!["second.txt".to_string()],
                "New item should have been moved to the new pane"
            );
        });

        let item_3 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(3, "third.txt", cx)])
        });
        workspace.update_in(cx, |workspace, window, cx| {
            let original_pane = workspace.panes[0].clone();
            workspace.set_active_pane(&original_pane, window, cx);
            workspace.add_item_to_active_pane(Box::new(item_3), None, true, window, cx);
            assert_eq!(workspace.panes.len(), 2, "No new panes were created");
            assert_eq!(
                pane_items_paths(&workspace.active_pane, cx),
                vec!["first.txt".to_string(), "third.txt".to_string()],
                "New pane should be ready to move one item out"
            );

            workspace.move_item_to_pane_at_index(
                &MoveItemToPane {
                    destination: 3,
                    focus: true,
                    clone: false,
                },
                window,
                cx,
            );
            assert_eq!(workspace.panes.len(), 3, "A new pane should be created");
            assert_eq!(
                pane_items_paths(&workspace.active_pane, cx),
                vec!["first.txt".to_string()],
                "After moving, one item should be left in the original pane"
            );
            assert_eq!(
                pane_items_paths(&workspace.panes[1], cx),
                vec!["second.txt".to_string()],
                "Previously created pane should be unchanged"
            );
            assert_eq!(
                pane_items_paths(&workspace.panes[2], cx),
                vec!["third.txt".to_string()],
                "New item should have been moved to the new pane"
            );
        });
    }

    #[gpui::test]
    async fn test_moving_items_can_clone_panes(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let item_1 = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "first.txt", cx)])
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item_1), None, true, window, cx);
            workspace.move_item_to_pane_in_direction(
                &MoveItemToPaneInDirection {
                    direction: SplitDirection::Right,
                    focus: true,
                    clone: true,
                },
                window,
                cx,
            );
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.move_item_to_pane_at_index(
                &MoveItemToPane {
                    destination: 3,
                    focus: true,
                    clone: true,
                },
                window,
                cx,
            );
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            assert_eq!(workspace.panes.len(), 3, "Two new panes were created");
            for pane in workspace.panes() {
                assert_eq!(
                    pane_items_paths(pane, cx),
                    vec!["first.txt".to_string()],
                    "Single item exists in all panes"
                );
            }
        });

        // verify that the active pane has been updated after waiting for the
        // pane focus event to fire and resolve
        workspace.read_with(cx, |workspace, _app| {
            assert_eq!(
                workspace.active_pane(),
                &workspace.panes[2],
                "The third pane should be the active one: {:?}",
                workspace.panes
            );
        })
    }

    #[gpui::test]
    async fn test_close_item_in_all_panes(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/root", json!({ "test.txt": "" })).await;

        let project = Project::test(fs, ["root".as_ref()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let pane_a = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        // Add item to pane A with project path
        let item_a = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "test.txt", cx)])
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item_a.clone()), None, true, window, cx)
        });

        // Split to create pane B
        let pane_b = workspace.update_in(cx, |workspace, window, cx| {
            workspace.split_pane(pane_a.clone(), SplitDirection::Right, window, cx)
        });

        // Add item with SAME project path to pane B, and pin it
        let item_b = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "test.txt", cx)])
        });
        pane_b.update_in(cx, |pane, window, cx| {
            pane.add_item(Box::new(item_b.clone()), true, true, None, window, cx);
            pane.set_pinned_count(1);
        });

        assert_eq!(pane_a.read_with(cx, |pane, _| pane.items_len()), 1);
        assert_eq!(pane_b.read_with(cx, |pane, _| pane.items_len()), 1);

        // close_pinned: false should only close the unpinned copy
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.close_item_in_all_panes(
                &CloseItemInAllPanes {
                    save_intent: Some(SaveIntent::Close),
                    close_pinned: false,
                },
                window,
                cx,
            )
        });
        cx.executor().run_until_parked();

        let item_count_a = pane_a.read_with(cx, |pane, _| pane.items_len());
        let item_count_b = pane_b.read_with(cx, |pane, _| pane.items_len());
        assert_eq!(item_count_a, 0, "Unpinned item in pane A should be closed");
        assert_eq!(item_count_b, 1, "Pinned item in pane B should remain");

        // Split again, seeing as closing the previous item also closed its
        // pane, so only pane remains, which does not allow us to properly test
        // that both items close when `close_pinned: true`.
        let pane_c = workspace.update_in(cx, |workspace, window, cx| {
            workspace.split_pane(pane_b.clone(), SplitDirection::Right, window, cx)
        });

        // Add an item with the same project path to pane C so that
        // close_item_in_all_panes can determine what to close across all panes
        // (it reads the active item from the active pane, and split_pane
        // creates an empty pane).
        let item_c = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new(1, "test.txt", cx)])
        });
        pane_c.update_in(cx, |pane, window, cx| {
            pane.add_item(Box::new(item_c.clone()), true, true, None, window, cx);
        });

        // close_pinned: true should close the pinned copy too
        workspace.update_in(cx, |workspace, window, cx| {
            let panes_count = workspace.panes().len();
            assert_eq!(panes_count, 2, "Workspace should have two panes (B and C)");

            workspace.close_item_in_all_panes(
                &CloseItemInAllPanes {
                    save_intent: Some(SaveIntent::Close),
                    close_pinned: true,
                },
                window,
                cx,
            )
        });
        cx.executor().run_until_parked();

        let item_count_b = pane_b.read_with(cx, |pane, _| pane.items_len());
        let item_count_c = pane_c.read_with(cx, |pane, _| pane.items_len());
        assert_eq!(item_count_b, 0, "Pinned item in pane B should be closed");
        assert_eq!(item_count_c, 0, "Unpinned item in pane C should be closed");
    }

    mod register_project_item_tests {

        use super::*;

        // View
        struct TestPngItemView {
            focus_handle: FocusHandle,
            project_path: ProjectPath,
        }
        // Model
        struct TestPngItem {
            project_path: ProjectPath,
        }

        impl project::ProjectItem for TestPngItem {
            fn try_open(
                _project: &Entity<Project>,
                path: &ProjectPath,
                cx: &mut App,
            ) -> Option<Task<anyhow::Result<Entity<Self>>>> {
                if path.path.extension().unwrap() == "png" {
                    let project_path = path.clone();
                    Some(cx.spawn(async move |cx| Ok(cx.new(|_| TestPngItem { project_path }))))
                } else {
                    None
                }
            }

            fn entry_id(&self, _: &App) -> Option<ProjectEntryId> {
                None
            }

            fn project_path(&self, _: &App) -> Option<ProjectPath> {
                Some(self.project_path.clone())
            }

            fn is_dirty(&self) -> bool {
                false
            }
        }

        impl Item for TestPngItemView {
            type Event = ();
            fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
                "".into()
            }
        }
        impl EventEmitter<()> for TestPngItemView {}
        impl Focusable for TestPngItemView {
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus_handle.clone()
            }
        }

        impl Render for TestPngItemView {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                Empty
            }
        }

        impl ProjectItem for TestPngItemView {
            type Item = TestPngItem;

            fn for_project_item(
                _project: Entity<Project>,
                _pane: Option<&Pane>,
                item: Entity<Self::Item>,
                _: &mut Window,
                cx: &mut Context<Self>,
            ) -> Self
            where
                Self: Sized,
            {
                Self {
                    focus_handle: cx.focus_handle(),
                    project_path: item.read(cx).project_path.clone(),
                }
            }
        }

        // View
        struct TestIpynbItemView {
            focus_handle: FocusHandle,
        }
        // Model
        struct TestIpynbItem {}

        impl project::ProjectItem for TestIpynbItem {
            fn try_open(
                _project: &Entity<Project>,
                path: &ProjectPath,
                cx: &mut App,
            ) -> Option<Task<anyhow::Result<Entity<Self>>>> {
                if path.path.extension().unwrap() == "ipynb" {
                    Some(cx.spawn(async move |cx| Ok(cx.new(|_| TestIpynbItem {}))))
                } else {
                    None
                }
            }

            fn entry_id(&self, _: &App) -> Option<ProjectEntryId> {
                None
            }

            fn project_path(&self, _: &App) -> Option<ProjectPath> {
                None
            }

            fn is_dirty(&self) -> bool {
                false
            }
        }

        impl Item for TestIpynbItemView {
            type Event = ();
            fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
                "".into()
            }
        }
        impl EventEmitter<()> for TestIpynbItemView {}
        impl Focusable for TestIpynbItemView {
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus_handle.clone()
            }
        }

        impl Render for TestIpynbItemView {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                Empty
            }
        }

        impl ProjectItem for TestIpynbItemView {
            type Item = TestIpynbItem;

            fn for_project_item(
                _project: Entity<Project>,
                _pane: Option<&Pane>,
                _item: Entity<Self::Item>,
                _: &mut Window,
                cx: &mut Context<Self>,
            ) -> Self
            where
                Self: Sized,
            {
                Self {
                    focus_handle: cx.focus_handle(),
                }
            }
        }

        struct TestAlternatePngItemView {
            focus_handle: FocusHandle,
        }

        impl Item for TestAlternatePngItemView {
            type Event = ();
            fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
                "".into()
            }
        }

        impl EventEmitter<()> for TestAlternatePngItemView {}
        impl Focusable for TestAlternatePngItemView {
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus_handle.clone()
            }
        }

        impl Render for TestAlternatePngItemView {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                Empty
            }
        }

        impl ProjectItem for TestAlternatePngItemView {
            type Item = TestPngItem;

            fn for_project_item(
                _project: Entity<Project>,
                _pane: Option<&Pane>,
                _item: Entity<Self::Item>,
                _: &mut Window,
                cx: &mut Context<Self>,
            ) -> Self
            where
                Self: Sized,
            {
                Self {
                    focus_handle: cx.focus_handle(),
                }
            }
        }

        #[gpui::test]
        async fn test_register_project_item(cx: &mut TestAppContext) {
            init_test(cx);

            cx.update(|cx| {
                register_project_item::<TestPngItemView>(cx);
                register_project_item::<TestIpynbItemView>(cx);
            });

            let fs = FakeFs::new(cx.executor());
            fs.insert_tree(
                "/root1",
                json!({
                    "one.png": "BINARYDATAHERE",
                    "two.ipynb": "{ totally a notebook }",
                    "three.txt": "editing text, sure why not?"
                }),
            )
            .await;

            let project = Project::test(fs, ["root1".as_ref()], cx).await;
            let (workspace, cx) =
                cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

            let worktree_id = project.update(cx, |project, cx| {
                project.worktrees(cx).next().unwrap().read(cx).id()
            });

            let handle = workspace
                .update_in(cx, |workspace, window, cx| {
                    let project_path = (worktree_id, rel_path("one.png"));
                    workspace.open_path(project_path, None, true, window, cx)
                })
                .await
                .unwrap();

            // Now we can check if the handle we got back errored or not
            assert_eq!(
                handle.to_any_view().entity_type(),
                TypeId::of::<TestPngItemView>()
            );

            let handle = workspace
                .update_in(cx, |workspace, window, cx| {
                    let project_path = (worktree_id, rel_path("two.ipynb"));
                    workspace.open_path(project_path, None, true, window, cx)
                })
                .await
                .unwrap();

            assert_eq!(
                handle.to_any_view().entity_type(),
                TypeId::of::<TestIpynbItemView>()
            );

            let handle = workspace
                .update_in(cx, |workspace, window, cx| {
                    let project_path = (worktree_id, rel_path("three.txt"));
                    workspace.open_path(project_path, None, true, window, cx)
                })
                .await;
            assert!(handle.is_err());
        }

        #[gpui::test]
        async fn test_open_url_or_file_resolves_remote_base_path(cx: &mut TestAppContext) {
            init_test(cx);
            cx.update(register_project_item::<TestPngItemView>);

            let project = Project::test(FakeFs::new(cx.executor()), [], cx).await;
            let worktree = project.update(cx, |project, cx| {
                let worktree = project.add_test_remote_worktree("/remote/project", cx);
                project.mark_as_collab_for_testing();
                worktree
            });
            let worktree_id = worktree.read_with(cx, |worktree, _| worktree.id());
            let (workspace, cx) =
                cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

            workspace.update_in(cx, |workspace, window, cx| {
                workspace.open_url_or_file(
                    "./sibling.png",
                    Some(Path::new("/remote/project/docs")),
                    window,
                    cx,
                );
            });
            cx.run_until_parked();

            let opened_item = workspace
                .read_with(cx, |workspace, cx| {
                    workspace
                        .active_item(cx)
                        .and_then(|item| item.downcast::<TestPngItemView>())
                })
                .expect("resolved remote project item should be opened");
            let project_path = opened_item.read_with(cx, |item, _| item.project_path.clone());
            assert_eq!(project_path.worktree_id, worktree_id);
            assert_eq!(project_path.path.as_ref(), rel_path("docs/sibling.png"));
        }

        #[gpui::test]
        async fn test_register_project_item_two_enter_one_leaves(cx: &mut TestAppContext) {
            init_test(cx);

            cx.update(|cx| {
                register_project_item::<TestPngItemView>(cx);
                register_project_item::<TestAlternatePngItemView>(cx);
            });

            let fs = FakeFs::new(cx.executor());
            fs.insert_tree(
                "/root1",
                json!({
                    "one.png": "BINARYDATAHERE",
                    "two.ipynb": "{ totally a notebook }",
                    "three.txt": "editing text, sure why not?"
                }),
            )
            .await;
            let project = Project::test(fs, ["root1".as_ref()], cx).await;
            let (workspace, cx) =
                cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
            let worktree_id = project.update(cx, |project, cx| {
                project.worktrees(cx).next().unwrap().read(cx).id()
            });

            let handle = workspace
                .update_in(cx, |workspace, window, cx| {
                    let project_path = (worktree_id, rel_path("one.png"));
                    workspace.open_path(project_path, None, true, window, cx)
                })
                .await
                .unwrap();

            // This _must_ be the second item registered
            assert_eq!(
                handle.to_any_view().entity_type(),
                TypeId::of::<TestAlternatePngItemView>()
            );

            let handle = workspace
                .update_in(cx, |workspace, window, cx| {
                    let project_path = (worktree_id, rel_path("three.txt"));
                    workspace.open_path(project_path, None, true, window, cx)
                })
                .await;
            assert!(handle.is_err());
        }
    }

    #[gpui::test]
    async fn test_status_bar_visibility(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, _cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        // Test with status bar shown (default)
        workspace.read_with(cx, |workspace, cx| {
            let visible = workspace.status_bar_visible(cx);
            assert!(visible, "Status bar should be visible by default");
        });

        // Test with status bar hidden
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings.status_bar.get_or_insert_default().show = Some(false);
            });
        });

        workspace.read_with(cx, |workspace, cx| {
            let visible = workspace.status_bar_visible(cx);
            assert!(!visible, "Status bar should be hidden when show is false");
        });

        // Test with status bar shown explicitly
        cx.update_global(|store: &mut SettingsStore, cx| {
            store.update_user_settings(cx, |settings| {
                settings.status_bar.get_or_insert_default().show = Some(true);
            });
        });

        workspace.read_with(cx, |workspace, cx| {
            let visible = workspace.status_bar_visible(cx);
            assert!(visible, "Status bar should be visible when show is true");
        });
    }

    #[gpui::test]
    async fn test_pane_close_active_item(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);

            workspace
                .right_dock()
                .update(cx, |right_dock, cx| right_dock.set_open(true, window, cx));

            panel
        });

        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        let item_a = cx.new(TestItem::new);
        let item_b = cx.new(TestItem::new);
        let item_a_id = item_a.entity_id();
        let item_b_id = item_b.entity_id();

        pane.update_in(cx, |pane, window, cx| {
            pane.add_item(Box::new(item_a.clone()), true, true, None, window, cx);
            pane.add_item(Box::new(item_b.clone()), true, true, None, window, cx);
        });

        pane.read_with(cx, |pane, _| {
            assert_eq!(pane.items_len(), 2);
            assert_eq!(pane.active_item().unwrap().item_id(), item_b_id);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |_, window, cx| {
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Assert that the `pane::CloseActiveItem` action is handled at the
        // workspace level when one of the dock panels is focused and, in that
        // case, the center pane's active item is closed but the focus is not
        // moved.
        cx.dispatch_action(pane::CloseActiveItem::default());
        cx.run_until_parked();

        pane.read_with(cx, |pane, _| {
            assert_eq!(pane.items_len(), 1);
            assert_eq!(pane.active_item().unwrap().item_id(), item_a_id);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });
    }

    #[gpui::test]
    async fn test_pane_close_active_item_from_dock_with_all_tabs_pinned(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);

            workspace
                .right_dock()
                .update(cx, |right_dock, cx| right_dock.set_open(true, window, cx));

            panel
        });

        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        let item_a = cx.new(TestItem::new);
        let item_b = cx.new(TestItem::new);
        let item_b_id = item_b.entity_id();

        pane.update_in(cx, |pane, window, cx| {
            pane.add_item(Box::new(item_a.clone()), true, true, None, window, cx);
            pane.add_item(Box::new(item_b.clone()), true, true, None, window, cx);
            pane.set_pinned_count(2);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        workspace.update_in(cx, |_, window, cx| {
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Every center tab is pinned, so with `close_pinned` unset this must be
        // a no-op. Dispatching it from the focused dock used to panic by
        // re-entering the workspace while it was still mid-update.
        cx.dispatch_action(pane::CloseActiveItem::default());
        cx.run_until_parked();

        pane.read_with(cx, |pane, _| {
            assert_eq!(pane.items_len(), 2);
            assert_eq!(pane.active_item().unwrap().item_id(), item_b_id);
        });
    }

    #[gpui::test]
    async fn test_panel_zoom_preserved_across_workspace_switch(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project_a = Project::test(fs.clone(), [], cx).await;
        let project_b = Project::test(fs, [], cx).await;

        let multi_workspace_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));
        cx.run_until_parked();

        multi_workspace_handle
            .update(cx, |mw, _window, cx| {
                mw.open_sidebar(cx);
            })
            .unwrap();

        let workspace_a = multi_workspace_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let _workspace_b = multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.test_add_workspace(project_b, window, cx)
            })
            .unwrap();

        // Switch to workspace A
        multi_workspace_handle
            .update(cx, |mw, window, cx| {
                let workspace = mw.workspaces().next().unwrap().clone();
                mw.activate(workspace, None, window, cx);
            })
            .unwrap();

        let cx = &mut VisualTestContext::from_window(multi_workspace_handle.into(), cx);

        // Add a panel to workspace A's right dock and open the dock
        let panel = workspace_a.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            workspace
                .right_dock()
                .update(cx, |dock, cx| dock.set_open(true, window, cx));
            panel
        });

        // Focus the panel through the workspace (matching existing test pattern)
        workspace_a.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        // Zoom the panel
        panel.update_in(cx, |panel, window, cx| {
            panel.set_zoomed(true, window, cx);
        });

        // Verify the panel is zoomed and the dock is open
        workspace_a.update_in(cx, |workspace, window, cx| {
            assert!(
                workspace.right_dock().read(cx).is_open(),
                "dock should be open before switch"
            );
            assert!(
                panel.is_zoomed(window, cx),
                "panel should be zoomed before switch"
            );
            assert!(
                panel.read(cx).focus_handle(cx).contains_focused(window, cx),
                "panel should be focused before switch"
            );
        });

        // Switch to workspace B
        multi_workspace_handle
            .update(cx, |mw, window, cx| {
                let workspace = mw.workspaces().nth(1).unwrap().clone();
                mw.activate(workspace, None, window, cx);
            })
            .unwrap();
        cx.run_until_parked();

        // Switch back to workspace A
        multi_workspace_handle
            .update(cx, |mw, window, cx| {
                let workspace = mw.workspaces().next().unwrap().clone();
                mw.activate(workspace, None, window, cx);
            })
            .unwrap();
        cx.run_until_parked();

        // Verify the panel is still zoomed and the dock is still open
        workspace_a.update_in(cx, |workspace, window, cx| {
            assert!(
                workspace.right_dock().read(cx).is_open(),
                "dock should still be open after switching back"
            );
            assert!(
                panel.is_zoomed(window, cx),
                "panel should still be zoomed after switching back"
            );
        });
    }

    #[gpui::test]
    async fn test_window_title_follows_active_workspace(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(path!("/root1"), json!({ "a.txt": "" }))
            .await;
        fs.insert_tree(path!("/root2"), json!({ "b.txt": "" }))
            .await;

        let project_a = Project::test(fs.clone(), [path!("/root1").as_ref()], cx).await;
        let project_b = Project::test(fs, [path!("/root2").as_ref()], cx).await;

        let multi_workspace_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));
        let workspace_a = multi_workspace_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let cx = &mut VisualTestContext::from_window(multi_workspace_handle.into(), cx);
        cx.run_until_parked();
        assert_eq!(cx.window_title().as_deref(), Some("root1"));

        // Activating a second workspace must update the shared window's title.
        multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.test_add_workspace(project_b.clone(), window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.window_title().as_deref(), Some("root2"));

        // Switching back must update the title again, even though workspace A's
        // own computed title hasn't changed since it was last active. This is
        // the regression: the per-workspace title cache was stale relative to
        // the shared window title.
        multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.activate(workspace_a.clone(), None, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.window_title().as_deref(), Some("root1"));
    }

    #[gpui::test]
    async fn test_window_title_includes_rbf_version(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| RbfVersion::set_global("0.26.0".to_string(), cx));

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(path!("/root1"), json!({ "a.txt": "" }))
            .await;

        let project = Project::test(fs, [path!("/root1").as_ref()], cx).await;
        let (_workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
        cx.run_until_parked();

        assert_eq!(cx.window_title().as_deref(), Some("root1 (rbf v0.26.0)"));
    }

    #[gpui::test]
    async fn test_background_workspace_does_not_clobber_window_title(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(path!("/root1"), json!({ "a.txt": "" }))
            .await;
        fs.insert_tree(path!("/root2"), json!({ "b.txt": "" }))
            .await;
        fs.insert_tree(path!("/root3"), json!({ "c.txt": "" }))
            .await;

        let project_a = Project::test(fs.clone(), [path!("/root1").as_ref()], cx).await;
        let project_b = Project::test(fs.clone(), [path!("/root2").as_ref()], cx).await;

        let multi_workspace_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));

        let cx = &mut VisualTestContext::from_window(multi_workspace_handle.into(), cx);
        cx.run_until_parked();
        assert_eq!(cx.window_title().as_deref(), Some("root1"));

        // Switch to workspace B; workspace A becomes a background workspace whose
        // event subscriptions are still live.
        multi_workspace_handle
            .update(cx, |mw, window, cx| {
                mw.test_add_workspace(project_b.clone(), window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.window_title().as_deref(), Some("root2"));

        // A title-affecting change in the background workspace A must not touch
        // the shared window's title, which belongs to the active workspace B.
        project_a
            .update(cx, |project, cx| {
                project.find_or_create_worktree(path!("/root3"), true, cx)
            })
            .await
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.window_title().as_deref(), Some("root2"));
    }

    fn pane_items_paths(pane: &Entity<Pane>, cx: &App) -> Vec<String> {
        pane.read(cx)
            .items()
            .flat_map(|item| {
                item.project_paths(cx)
                    .into_iter()
                    .map(|path| path.path.display(PathStyle::local()).into_owned())
            })
            .collect()
    }

    pub fn init_test(cx: &mut TestAppContext) {
        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            cx.set_global(db::AppDatabase::test_new());
            // `main.rs` installs this in production. Component placement writes
            // through it, so any test that moves a panel needs it as well.
            <dyn fs::Fs>::set_global(fs, cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
    }

    #[gpui::test]
    async fn test_toggle_theme_mode_persists_and_updates_active_theme(cx: &mut TestAppContext) {
        use settings::{ThemeName, ThemeSelection};
        use theme::SystemAppearance;
        use zed_actions::theme::ToggleMode;

        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        let settings_fs: Arc<dyn fs::Fs> = fs.clone();

        fs.insert_tree(path!("/root"), json!({ "file.rs": "fn main() {}\n" }))
            .await;

        // Build a test project and workspace view so the test can invoke
        // the workspace action handler the same way the UI would.
        let project = Project::test(fs.clone(), [path!("/root").as_ref()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        // Seed the settings file with a plain static light theme so the
        // first toggle always starts from a known persisted state.
        workspace.update_in(cx, |_workspace, _window, cx| {
            *SystemAppearance::global_mut(cx) = SystemAppearance(theme::Appearance::Light);
            settings::update_settings_file(settings_fs.clone(), cx, |settings, _cx| {
                settings.theme.theme = Some(ThemeSelection::Static(ThemeName("One Light".into())));
            });
        });
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();

        // Confirm the initial persisted settings contain the static theme
        // we just wrote before any toggling happens.
        let settings_text = SettingsStore::load_settings(&settings_fs).await.unwrap();
        assert!(settings_text.contains(r#""theme": "One Light""#));

        // Toggle once. This should migrate the persisted theme settings
        // into light/dark slots and enable system mode.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_theme_mode(&ToggleMode, window, cx);
        });
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();

        // 1. Static -> Dynamic
        // this assertion checks theme changed from static to dynamic.
        let settings_text = SettingsStore::load_settings(&settings_fs).await.unwrap();
        let parsed: serde_json::Value = settings::parse_json_with_comments(&settings_text).unwrap();
        assert_eq!(
            parsed["theme"],
            serde_json::json!({
                "mode": "system",
                "light": "One Light",
                "dark": "One Dark"
            })
        );

        // 2. Toggle again, suppose it will change the mode to light
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_theme_mode(&ToggleMode, window, cx);
        });
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();

        let settings_text = SettingsStore::load_settings(&settings_fs).await.unwrap();
        assert!(settings_text.contains(r#""mode": "light""#));
    }

    fn dirty_project_item(id: u64, path: &str, cx: &mut App) -> Entity<TestProjectItem> {
        let item = TestProjectItem::new(id, path, cx);
        item.update(cx, |item, _| {
            item.is_dirty = true;
        });
        item
    }

    fn new_test_project_item(
        id: u64,
        path: &str,
        worktree_id: WorktreeId,
        cx: &mut App,
    ) -> Entity<TestProjectItem> {
        let item = TestProjectItem::new(id, path, cx);
        item.update(cx, |item, _| {
            if let Some(ref mut project_path) = item.project_path {
                project_path.worktree_id = worktree_id;
            }
        });
        item
    }

    #[gpui::test]
    async fn test_zoomed_panel_without_pane_preserved_on_center_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            workspace
                .right_dock()
                .update(cx, |dock, cx| dock.set_open(true, window, cx));
            panel
        });

        let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        pane.update_in(cx, |pane, window, cx| {
            let item = cx.new(TestItem::new);
            pane.add_item(Box::new(item), true, true, None, window, cx);
        });

        // Transfer focus to the panel, then zoom it. Using toggle_panel_focus
        // mirrors the real-world flow and avoids side effects from directly
        // focusing the panel while the center pane is active.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });

        panel.update_in(cx, |panel, window, cx| {
            panel.set_zoomed(true, window, cx);
        });

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.right_dock().read(cx).is_open());
            assert!(panel.is_zoomed(window, cx));
            assert!(panel.read(cx).focus_handle(cx).contains_focused(window, cx));
        });

        // Simulate a spurious pane::Event::Focus on the center pane while the
        // panel still has focus. This mirrors what happens during macOS window
        // activation: the center pane fires a focus event even though actual
        // focus remains on the dock panel.
        pane.update_in(cx, |_, _, cx| {
            cx.emit(pane::Event::Focus);
        });

        // The dock must remain open because the panel had focus at the time the
        // event was processed. Before the fix, dock_to_preserve was None for
        // panels that don't implement pane(), causing the dock to close.
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(
                workspace.right_dock().read(cx).is_open(),
                "Dock should stay open when its zoomed panel (without pane()) still has focus"
            );
            assert!(panel.is_zoomed(window, cx));
        });
    }

    #[gpui::test]
    async fn test_panels_stay_open_after_position_change_and_settings_update(
        cx: &mut gpui::TestAppContext,
    ) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

        // Add two panels to the left dock and open it.
        let (panel_a, panel_b) = workspace.update_in(cx, |workspace, window, cx| {
            let panel_a = cx.new(|cx| TestPanel::new(DockPosition::Left, 100, cx));
            let panel_b = cx.new(|cx| TestPanel::new(DockPosition::Left, 101, cx));
            workspace.add_panel(panel_a.clone(), window, cx);
            workspace.add_panel(panel_b.clone(), window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.set_open(true, window, cx);
                dock.activate_panel(0, window, cx);
            });
            (panel_a, panel_b)
        });

        workspace.update_in(cx, |workspace, _, cx| {
            assert!(workspace.left_dock().read(cx).is_open());
        });

        // Simulate a feature flag changing default dock positions: both panels
        // move from Left to Right.
        workspace.update_in(cx, |_workspace, _window, cx| {
            panel_a.update(cx, |p, _cx| p.position = DockPosition::Right);
            panel_b.update(cx, |p, _cx| p.position = DockPosition::Right);
            cx.update_global::<SettingsStore, _>(|_, _| {});
        });

        // Both panels should now be in the right dock.
        workspace.update_in(cx, |workspace, _, cx| {
            let right_dock = workspace.right_dock().read(cx);
            assert_eq!(right_dock.panels_len(), 2);
        });

        // Open the right dock and activate panel_b (simulating the user
        // opening the panel after it moved).
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.right_dock().update(cx, |dock, cx| {
                dock.set_open(true, window, cx);
                dock.activate_panel(1, window, cx);
            });
        });

        // Now trigger another SettingsStore change
        workspace.update_in(cx, |_workspace, _window, cx| {
            cx.update_global::<SettingsStore, _>(|_, _| {});
        });

        workspace.update_in(cx, |workspace, _, cx| {
            assert!(
                workspace.right_dock().read(cx).is_open(),
                "Right dock should still be open after a settings change"
            );
            assert_eq!(
                workspace.right_dock().read(cx).panels_len(),
                2,
                "Both panels should still be in the right dock"
            );
        });
    }

    #[gpui::test]
    async fn test_most_recent_active_path_skips_read_only_paths(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/project"),
            json!({
                "src": { "main.py": "" },
                ".venv": { "lib": { "dep.py": "" } },
            }),
        )
        .await;

        let project = Project::test(fs.clone(), [path!("/project").as_ref()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
        let worktree_id = project.update(cx, |project, cx| {
            project.worktrees(cx).next().unwrap().read(cx).id()
        });

        // Configure .venv as read-only
        workspace.update_in(cx, |_workspace, _window, cx| {
            cx.update_global::<SettingsStore, _>(|store, cx| {
                store
                    .set_user_settings(r#"{"read_only_files": ["**/.venv/**"]}"#, cx)
                    .ok();
            });
        });

        let item_dep = cx.new(|cx| {
            TestItem::new(cx).with_project_items(&[TestProjectItem::new_in_worktree(
                1001,
                ".venv/lib/dep.py",
                worktree_id,
                cx,
            )])
        });

        // dep.py is active but matches read_only_files → should be skipped
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(item_dep.clone()), None, true, window, cx);
        });
        let path = workspace.read_with(cx, |workspace, cx| workspace.most_recent_active_path(cx));
        assert_eq!(path, None);
    }

    #[gpui::test]
    async fn test_open_url_or_file_routes_urls(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            "/root",
            json!({
                "file.txt": "content",
                "subdir": {
                    "nested.txt": "nested content"
                }
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/root").as_ref()], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        // Test opening an HTTPS URL - should go to open_url
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_url_or_file("https://example.com", None, window, cx);
        });
        assert_eq!(cx.opened_url(), Some("https://example.com".to_string()));

        // Test opening an HTTP URL - should go to open_url
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_url_or_file("http://example.com", None, window, cx);
        });
        assert_eq!(cx.opened_url(), Some("http://example.com".to_string()));

        // Test opening other URI schemes - should go to open_url
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_url_or_file("mailto:test@example.com", None, window, cx);
        });
        assert_eq!(cx.opened_url(), Some("mailto:test@example.com".to_string()));

        // Test opening a path that doesn't exist and doesn't match project - should fallback to open_url
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_url_or_file("nonexistent.txt", None, window, cx);
        });
        assert_eq!(cx.opened_url(), Some("nonexistent.txt".to_string()));
    }
}
