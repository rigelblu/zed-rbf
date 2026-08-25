use anyhow::{Context as _, Result};
use fs::Fs;

use gpui::{
    AnyView, App, AsyncApp, Context, DragMoveEvent, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, ManagedView, MouseButton, Pixels, Render, ScrollHandle, Subscription, Task, TaskExt,
    WeakEntity, Window, WindowId, actions, deferred, px,
};
pub use project::ProjectGroupKey;
use project::{DisableAiSettings, Project};
use remote::RemoteConnectionOptions;
use settings::Settings;
pub use settings::SidebarSide;
use std::cell::Cell;
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use ui::prelude::*;
use util::ResultExt;
use util::path_list::PathList;
use zed_actions::agents_sidebar::ToggleThreadSwitcher;

use agent_settings::AgentSettings;
use settings::SidebarDockPosition;
use ui::{ContextMenu, PopoverMenuHandle, right_click_menu};

const SIDEBAR_RESIZE_HANDLE_SIZE: Pixels = px(6.0);
const WORKSPACE_CONFIGURATION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

use crate::open_remote_project_with_existing_connection;
use crate::{
    CloseIntent, CloseWindow, DockPosition, Event as WorkspaceEvent, Item, ModalView, OpenMode,
    Panel, Workspace, WorkspaceId, client_side_decorations,
    persistence::{
        WorkspaceConfigurationMutation, WorkspaceConfigurationStore,
        model::{MultiWorkspaceState, WorkspaceConfigurationId, WorkspaceConfigurationMember},
    },
};

actions!(
    multi_workspace,
    [
        /// Toggles the workspace switcher sidebar.
        ToggleWorkspaceSidebar,
        /// Closes the workspace sidebar.
        CloseWorkspaceSidebar,
        /// Moves focus to or from the workspace sidebar without closing it.
        FocusWorkspaceSidebar,
        /// Activates the next workspace project.
        NextProject,
        /// Activates the previous workspace project.
        PreviousProject,
        /// Activates the next thread in sidebar order.
        NextThread,
        /// Activates the previous thread in sidebar order.
        PreviousThread,
        /// Creates a new thread in the current workspace.
        NewThread,
        /// Moves the active project to a new window.
        MoveProjectToNewWindow,
    ]
);

actions!(
    workspace,
    [
        /// Saves the current window's workspace configuration under a new name.
        SaveWorkspaceConfigurationAs,
        /// Opens the saved workspace configuration manager.
        ManageWorkspaceConfigurations,
        /// Selects the next saved workspace configuration in the manager.
        SelectNextWorkspaceConfiguration,
        /// Selects the previous saved workspace configuration in the manager.
        SelectPreviousWorkspaceConfiguration,
        /// Opens the saved workspace configuration switcher.
        SwitchWorkspaceConfiguration,
    ]
);

#[derive(Default)]
pub struct SidebarRenderState {
    pub open: bool,
    pub side: SidebarSide,
}

pub fn sidebar_side_context_menu(
    id: impl Into<ElementId>,
    cx: &App,
) -> ui::RightClickMenu<ContextMenu> {
    let current_position = AgentSettings::get_global(cx).sidebar_side;
    right_click_menu(id).menu(move |window, cx| {
        let fs = <dyn fs::Fs>::global(cx);
        ContextMenu::build(window, cx, move |mut menu, _, _cx| {
            let positions: [(SidebarDockPosition, &str); 2] = [
                (SidebarDockPosition::Left, "Left"),
                (SidebarDockPosition::Right, "Right"),
            ];
            for (position, label) in positions {
                let fs = fs.clone();
                menu = menu.toggleable_entry(
                    label,
                    position == current_position,
                    IconPosition::Start,
                    None,
                    move |_window, cx| {
                        let side = match position {
                            SidebarDockPosition::Left => "left",
                            SidebarDockPosition::Right => "right",
                        };
                        telemetry::event!("Sidebar Side Changed", side = side);
                        settings::update_settings_file(fs.clone(), cx, move |settings, _cx| {
                            settings
                                .agent
                                .get_or_insert_default()
                                .set_sidebar_side(position);
                        });
                    },
                );
            }
            menu
        })
    })
}

pub enum MultiWorkspaceEvent {
    ActiveWorkspaceChanged {
        source_workspace: Option<WeakEntity<Workspace>>,
    },
    WorkspaceAdded(Entity<Workspace>),
    WorkspaceRemoved(EntityId),
    ProjectGroupsChanged,
}

pub enum SidebarEvent {
    SerializeNeeded,
}

pub trait Sidebar: Focusable + Render + EventEmitter<SidebarEvent> + Sized {
    fn width(&self, cx: &App) -> Pixels;
    fn set_width(&mut self, width: Option<Pixels>, cx: &mut Context<Self>);
    fn has_notifications(&self, cx: &App) -> bool;
    fn side(&self, _cx: &App) -> SidebarSide;

    fn is_threads_list_view_active(&self) -> bool {
        true
    }
    /// Makes focus reset back to the search editor upon toggling the sidebar from outside
    fn prepare_for_focus(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
    /// Opens or cycles the thread switcher popup.
    fn toggle_thread_switcher(
        &mut self,
        _select_last: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// Activates the next or previous project.
    fn cycle_project(&mut self, _forward: bool, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Activates the next or previous thread in sidebar order.
    fn cycle_thread(&mut self, _forward: bool, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Return an opaque JSON blob of sidebar-specific state to persist.
    fn serialized_state(&self, _cx: &App) -> Option<String> {
        None
    }

    /// Restore sidebar state from a previously-serialized blob.
    fn restore_serialized_state(
        &mut self,
        _state: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }
}

pub trait SidebarHandle: 'static + Send + Sync {
    fn width(&self, cx: &App) -> Pixels;
    fn set_width(&self, width: Option<Pixels>, cx: &mut App);
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn focus(&self, window: &mut Window, cx: &mut App);
    fn prepare_for_focus(&self, window: &mut Window, cx: &mut App);
    fn has_notifications(&self, cx: &App) -> bool;
    fn to_any(&self) -> AnyView;
    fn entity_id(&self) -> EntityId;
    fn toggle_thread_switcher(&self, select_last: bool, window: &mut Window, cx: &mut App);
    fn cycle_project(&self, forward: bool, window: &mut Window, cx: &mut App);
    fn cycle_thread(&self, forward: bool, window: &mut Window, cx: &mut App);

    fn is_threads_list_view_active(&self, cx: &App) -> bool;

    fn side(&self, cx: &App) -> SidebarSide;
    fn serialized_state(&self, cx: &App) -> Option<String>;
    fn restore_serialized_state(&self, state: &str, window: &mut Window, cx: &mut App);
}

#[derive(Clone)]
pub struct DraggedSidebar;

impl Render for DraggedSidebar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl<T: Sidebar> SidebarHandle for Entity<T> {
    fn width(&self, cx: &App) -> Pixels {
        self.read(cx).width(cx)
    }

    fn set_width(&self, width: Option<Pixels>, cx: &mut App) {
        self.update(cx, |this, cx| this.set_width(width, cx))
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn prepare_for_focus(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.prepare_for_focus(window, cx));
    }

    fn has_notifications(&self, cx: &App) -> bool {
        self.read(cx).has_notifications(cx)
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn entity_id(&self) -> EntityId {
        Entity::entity_id(self)
    }

    fn toggle_thread_switcher(&self, select_last: bool, window: &mut Window, cx: &mut App) {
        let entity = self.clone();
        window.defer(cx, move |window, cx| {
            entity.update(cx, |this, cx| {
                this.toggle_thread_switcher(select_last, window, cx);
            });
        });
    }

    fn cycle_project(&self, forward: bool, window: &mut Window, cx: &mut App) {
        let entity = self.clone();
        window.defer(cx, move |window, cx| {
            entity.update(cx, |this, cx| {
                this.cycle_project(forward, window, cx);
            });
        });
    }

    fn cycle_thread(&self, forward: bool, window: &mut Window, cx: &mut App) {
        let entity = self.clone();
        window.defer(cx, move |window, cx| {
            entity.update(cx, |this, cx| {
                this.cycle_thread(forward, window, cx);
            });
        });
    }

    fn is_threads_list_view_active(&self, cx: &App) -> bool {
        self.read(cx).is_threads_list_view_active()
    }

    fn side(&self, cx: &App) -> SidebarSide {
        self.read(cx).side(cx)
    }

    fn serialized_state(&self, cx: &App) -> Option<String> {
        self.read(cx).serialized_state(cx)
    }

    fn restore_serialized_state(&self, state: &str, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| {
            this.restore_serialized_state(state, window, cx)
        })
    }
}

#[derive(Clone)]
pub struct ProjectGroup {
    pub key: ProjectGroupKey,
    pub workspaces: Vec<Entity<Workspace>>,
    pub expanded: bool,
}

pub struct SerializedProjectGroupState {
    pub key: ProjectGroupKey,
    pub expanded: bool,
}

#[derive(Clone)]
pub struct ProjectGroupState {
    pub key: ProjectGroupKey,
    pub expanded: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RemovalIntent {
    KeepProject,
    CloseProject,
}

/// One row per workspace held by this window. The displayed workspace is
/// always one of these rows. `pinned` records whether the workspace survives
/// being navigated away from; `activated_at` records when it was last
/// displayed.
struct HeldWorkspace {
    workspace: Entity<Workspace>,
    pinned: bool,
    activated_at: Option<u64>,
}

struct WorkspaceConfigurationSnapshot {
    generation: u64,
    workspaces: Vec<Entity<Workspace>>,
    members: Vec<WorkspaceConfigurationMember>,
    active_member: Option<WorkspaceId>,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceConfigurationSwitchTestStage {
    TargetStaged,
    BeforeCommit,
}

#[cfg(test)]
struct WorkspaceConfigurationSwitchTestPause {
    stage: WorkspaceConfigurationSwitchTestStage,
    reached: Option<futures::channel::oneshot::Sender<()>>,
    resume: Option<futures::channel::oneshot::Receiver<()>>,
}

#[cfg(test)]
struct WorkspaceConfigurationSaveTestPause {
    reached: Option<futures::channel::oneshot::Sender<()>>,
    resume: Option<futures::channel::oneshot::Receiver<()>>,
}

enum WorkspaceConfigurationTargetMember {
    Live(Entity<Workspace>),
    Serialized(crate::persistence::model::SerializedWorkspace),
}

#[derive(Debug)]
struct WorkspaceConfigurationSwitchCanceled;

impl std::fmt::Display for WorkspaceConfigurationSwitchCanceled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("workspace configuration switch canceled")
    }
}

impl std::error::Error for WorkspaceConfigurationSwitchCanceled {}

fn project_group_covers_workspace_key(
    group_key: &ProjectGroupKey,
    workspace_key: &ProjectGroupKey,
) -> bool {
    if group_key.matches(workspace_key) {
        return true;
    }
    if group_key.host() != workspace_key.host() {
        return false;
    }

    let group_paths = group_key.path_list().paths();
    let workspace_paths = workspace_key.path_list().paths();
    !workspace_paths.is_empty()
        && workspace_paths.iter().all(|workspace_path| {
            group_paths
                .iter()
                .any(|group_path| workspace_path.starts_with(group_path))
        })
}

pub struct MultiWorkspace {
    window_id: WindowId,
    held: Vec<HeldWorkspace>,
    project_groups: Vec<ProjectGroupState>,
    /// Source of truth for which workspace is presented in this window, shared
    /// with each member `Workspace` so they can tell whether they own the
    /// platform window's title and edited indicator. This only exists to prevent
    /// Workspaces from having to read their parent MultiWorkspace to check
    /// chrome ownership, as that might cause a double lease. Kept in sync with
    /// `active_workspace`.
    active_workspace_id: Rc<Cell<EntityId>>,
    sidebar: Option<Box<dyn SidebarHandle>>,
    sidebar_open: bool,
    sidebar_overlay: Option<AnyView>,
    pub(crate) workspace_tabs_scroll_handle: ScrollHandle,
    pub(crate) workspace_tabs_last_scrolled_workspace_id: Cell<Option<EntityId>>,
    pub(crate) workspace_tabs_last_scrolled_index: Cell<Option<usize>>,
    pub(crate) workspace_configuration_menu_handle: PopoverMenuHandle<ContextMenu>,
    pending_removal_tasks: Vec<Task<()>>,
    /// The saved configuration this window is currently following, if any.
    ///
    /// `None` means the current workspace set is unnamed: it is still fully usable, it
    /// just has no durable record, so nothing is checkpointed on its behalf.
    active_configuration_id: Option<WorkspaceConfigurationId>,
    configuration_checkpoint_error: Option<String>,
    workspace_configuration_generation: u64,
    configuration_checkpoint_queue_tail: Option<Task<()>>,
    configuration_switch_in_progress: bool,
    configuration_switch_gate: Option<String>,
    configuration_switch_gate_focus_handle: FocusHandle,
    #[cfg(test)]
    configuration_switch_test_pause: Option<WorkspaceConfigurationSwitchTestPause>,
    #[cfg(test)]
    configuration_save_test_pause: Option<WorkspaceConfigurationSaveTestPause>,
    _serialize_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    previous_focus_handle: Option<FocusHandle>,
}

impl EventEmitter<MultiWorkspaceEvent> for MultiWorkspace {}

impl MultiWorkspace {
    fn workspace_configuration_snapshot(&self, cx: &App) -> Result<WorkspaceConfigurationSnapshot> {
        let workspaces = self.ordered_workspaces(cx);
        let mut members = Vec::with_capacity(workspaces.len());
        for workspace in &workspaces {
            let workspace = workspace.read(cx);
            anyhow::ensure!(
                workspace.project().read(cx).is_local(),
                "Workspace configurations can only contain local workspaces."
            );
            let workspace_id = workspace
                .database_id()
                .context("a workspace has no durable persistence id")?;
            let identity_paths = workspace
                .project_group_key(cx)
                .path_list()
                .paths()
                .iter()
                .map(|path| path.to_path_buf())
                .collect();
            members.push(WorkspaceConfigurationMember {
                workspace_id,
                identity_paths,
            });
        }

        let active_member = self
            .workspace()
            .read(cx)
            .database_id()
            .context("the active workspace has no durable persistence id")?;
        anyhow::ensure!(
            members
                .iter()
                .any(|member| member.workspace_id == active_member),
            "the active workspace is not part of the window's workspace set"
        );

        Ok(WorkspaceConfigurationSnapshot {
            generation: self.workspace_configuration_generation,
            workspaces,
            members,
            active_member: Some(active_member),
        })
    }

    async fn exact_checkpoint_configuration_members(
        workspaces: &[Entity<Workspace>],
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let mut tasks = Vec::with_capacity(workspaces.len());
        for workspace in workspaces {
            tasks.push(workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx)));
        }

        let mut first_error = None;
        for result in futures::future::join_all(tasks).await {
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn publish_configuration_checkpoint_result(
        this: &WeakEntity<Self>,
        configuration_id: WorkspaceConfigurationId,
        generation: u64,
        result: &Result<()>,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let error = result.as_ref().err().map(|error| format!("{error:#}"));
        let serialization = this.update(cx, |this, cx| {
            if this.active_configuration_id != Some(configuration_id)
                || this.workspace_configuration_generation != generation
            {
                return None;
            }
            this.configuration_checkpoint_error = error;
            this.serialize(cx);
            cx.notify();
            Some(this.flush_serialization())
        })?;
        if let Some(serialization) = serialization {
            serialization.await;
        }
        Ok(())
    }

    async fn checkpoint_configuration_snapshot(
        this: &WeakEntity<Self>,
        configuration_id: WorkspaceConfigurationId,
        snapshot: WorkspaceConfigurationSnapshot,
        workspace_row_reservation_id: uuid::Uuid,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let result = async {
            Self::exact_checkpoint_configuration_members(&snapshot.workspaces, cx).await?;
            let still_current = this.read_with(cx, |this, _cx| {
                this.active_configuration_id == Some(configuration_id)
                    && this.workspace_configuration_generation == snapshot.generation
            })?;
            anyhow::ensure!(
                still_current,
                "the workspace set changed while its configuration was checkpointing"
            );

            cx.update(|cx| {
                WorkspaceConfigurationStore::mutate_global(
                    WorkspaceConfigurationMutation::Checkpoint {
                        id: configuration_id,
                        members: snapshot.members,
                        active_member: snapshot.active_member,
                    },
                    cx,
                )
            })
            .await?;
            Ok(())
        }
        .await;

        let publish_result = Self::publish_configuration_checkpoint_result(
            this,
            configuration_id,
            snapshot.generation,
            &result,
            cx,
        )
        .await;
        cx.update(|cx| {
            WorkspaceConfigurationStore::release_workspace_rows(workspace_row_reservation_id, cx)
        });
        publish_result?;
        result
    }

    fn enqueue_active_configuration_checkpoint(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(configuration_id) = self.active_configuration_id else {
            return Task::ready(Ok(()));
        };
        let generation = self.workspace_configuration_generation;
        let snapshot = self.workspace_configuration_snapshot(cx);
        let workspace_row_reservation_id = snapshot.as_ref().ok().map(|snapshot| {
            WorkspaceConfigurationStore::reserve_workspace_rows(
                snapshot.members.iter().map(|member| member.workspace_id),
                cx,
            )
        });
        let previous = self.configuration_checkpoint_queue_tail.take();
        let (send_result, receive_result) = futures::channel::oneshot::channel();

        let queued = cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let result = match snapshot {
                Ok(snapshot) => match workspace_row_reservation_id {
                    Some(workspace_row_reservation_id) => {
                        Self::checkpoint_configuration_snapshot(
                            &this,
                            configuration_id,
                            snapshot,
                            workspace_row_reservation_id,
                            cx,
                        )
                        .await
                    }
                    None => Err(anyhow::anyhow!(
                        "workspace configuration checkpoint lost its row reservation"
                    )),
                },
                Err(error) => {
                    let result = Err(error);
                    if let Err(error) = Self::publish_configuration_checkpoint_result(
                        &this,
                        configuration_id,
                        generation,
                        &result,
                        cx,
                    )
                    .await
                    {
                        log::error!(
                            "failed to publish workspace configuration checkpoint error: {error:#}"
                        );
                    }
                    result
                }
            };
            if send_result.send(result).is_err() {
                log::debug!("workspace configuration checkpoint caller was dropped");
            }
        });
        self.configuration_checkpoint_queue_tail = Some(queued);

        cx.background_spawn(async move {
            receive_result
                .await
                .context("workspace configuration checkpoint queue dropped the result")?
        })
    }

    fn workspace_configuration_changed(&mut self, cx: &mut Context<Self>) {
        self.workspace_configuration_generation += 1;
        if self.active_configuration_id.is_some() {
            self.enqueue_active_configuration_checkpoint(cx).detach();
        }
    }

    pub fn save_configuration_as(
        &mut self,
        name: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<WorkspaceConfigurationId>> {
        let snapshot = match self.workspace_configuration_snapshot(cx) {
            Ok(snapshot) => snapshot,
            Err(error) => return Task::ready(Err(error)),
        };
        let workspace_row_reservation_id = WorkspaceConfigurationStore::reserve_workspace_rows(
            snapshot.members.iter().map(|member| member.workspace_id),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = async {
                Self::exact_checkpoint_configuration_members(&snapshot.workspaces, cx).await?;
                let generation_is_current = this.read_with(cx, |this, _cx| {
                    this.workspace_configuration_generation == snapshot.generation
                })?;
                anyhow::ensure!(
                    generation_is_current,
                    "the workspace set changed while the configuration was being saved"
                );

                #[cfg(test)]
                Self::reach_configuration_save_test_pause(&this, cx).await?;

                let expected_generation = snapshot.generation;
                let validation_target = this.clone();
                let commit = cx
                    .update(|cx| {
                        WorkspaceConfigurationStore::mutate_conditionally_global(
                            move |cx| {
                                let generation_is_current =
                                    validation_target.read_with(cx, |this, _cx| {
                                        this.workspace_configuration_generation
                                            == expected_generation
                                    })?;
                                anyhow::ensure!(
                                    generation_is_current,
                                    "the workspace set changed while the configuration was being saved"
                                );
                                Ok(())
                            },
                            WorkspaceConfigurationMutation::Create {
                                name,
                                members: snapshot.members,
                                active_member: snapshot.active_member,
                            },
                            cx,
                        )
                    })
                    .await?;

                let serialization = this.update(cx, |this, cx| {
                    this.active_configuration_id = Some(commit.id);
                    this.configuration_checkpoint_error = None;
                    this.serialize(cx);
                    cx.notify();
                    this.flush_serialization()
                })?;
                serialization.await;
                Ok(commit.id)
            }
            .await;
            cx.update(|cx| {
                WorkspaceConfigurationStore::release_workspace_rows(
                    workspace_row_reservation_id,
                    cx,
                )
            });
            result
        })
    }

    #[cfg(test)]
    pub(crate) fn pause_configuration_save_for_test(
        &mut self,
    ) -> (
        futures::channel::oneshot::Receiver<()>,
        futures::channel::oneshot::Sender<()>,
    ) {
        let (reached_sender, reached_receiver) = futures::channel::oneshot::channel();
        let (resume_sender, resume_receiver) = futures::channel::oneshot::channel();
        self.configuration_save_test_pause = Some(WorkspaceConfigurationSaveTestPause {
            reached: Some(reached_sender),
            resume: Some(resume_receiver),
        });
        (reached_receiver, resume_sender)
    }

    #[cfg(test)]
    async fn reach_configuration_save_test_pause(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let resume = this.update(cx, |this, _cx| {
            let Some(pause) = this.configuration_save_test_pause.as_mut() else {
                return None;
            };
            if let Some(reached) = pause.reached.take() {
                reached.send(()).ok();
            }
            pause.resume.take()
        })?;
        if let Some(resume) = resume {
            resume
                .await
                .context("workspace configuration save test pause was dropped")?;
            this.update(cx, |this, _cx| {
                this.configuration_save_test_pause = None;
            })?;
        }
        Ok(())
    }

    pub fn retry_configuration_checkpoint(&mut self, cx: &mut Context<Self>) -> Task<Result<()>> {
        self.enqueue_active_configuration_checkpoint(cx)
    }

    pub fn active_configuration_id(&self) -> Option<WorkspaceConfigurationId> {
        self.active_configuration_id
    }

    pub fn configuration_checkpoint_error(&self) -> Option<&str> {
        self.configuration_checkpoint_error.as_deref()
    }

    pub(crate) fn configuration_switch_in_progress(&self) -> bool {
        self.configuration_switch_in_progress
    }

    pub(crate) fn workspace_configuration_switch_was_canceled(error: &anyhow::Error) -> bool {
        error
            .downcast_ref::<WorkspaceConfigurationSwitchCanceled>()
            .is_some()
    }

    #[cfg(test)]
    pub(crate) fn pause_configuration_switch_for_test(
        &mut self,
        stage: WorkspaceConfigurationSwitchTestStage,
    ) -> (
        futures::channel::oneshot::Receiver<()>,
        futures::channel::oneshot::Sender<()>,
    ) {
        let (reached_sender, reached_receiver) = futures::channel::oneshot::channel();
        let (resume_sender, resume_receiver) = futures::channel::oneshot::channel();
        self.configuration_switch_test_pause = Some(WorkspaceConfigurationSwitchTestPause {
            stage,
            reached: Some(reached_sender),
            resume: Some(resume_receiver),
        });
        (reached_receiver, resume_sender)
    }

    #[cfg(test)]
    async fn reach_configuration_switch_test_pause(
        this: &WeakEntity<Self>,
        stage: WorkspaceConfigurationSwitchTestStage,
        cx: &mut gpui::AsyncWindowContext,
    ) -> Result<()> {
        let resume = this.update(cx, |this, _cx| {
            let Some(pause) = this.configuration_switch_test_pause.as_mut() else {
                return None;
            };
            if pause.stage != stage {
                return None;
            }
            if let Some(reached) = pause.reached.take() {
                reached.send(()).ok();
            }
            pause.resume.take()
        })?;
        if let Some(resume) = resume {
            resume
                .await
                .context("workspace configuration switch test pause was dropped")?;
            this.update(cx, |this, _cx| {
                this.configuration_switch_test_pause = None;
            })?;
        }
        Ok(())
    }

    pub(crate) fn unnamed_configuration_is_pristine_scratch(
        &self,
        window: &Window,
        cx: &App,
    ) -> bool {
        if self.active_configuration_id.is_some() || self.held.len() != 1 {
            return false;
        }
        let workspace = self.workspace().read(cx);
        if !workspace.project().read(cx).is_local()
            || !workspace
                .project_group_key(cx)
                .path_list()
                .paths()
                .is_empty()
            || workspace.panes().len() != 1
            || workspace.panes()[0].read(cx).items_len() != 0
            || workspace.panes()[0].read(cx).has_restorable_items()
        {
            return false;
        }
        workspace.capture_dock_state(window, cx) == Default::default()
    }

    pub(crate) fn restore_configuration_association(
        &mut self,
        configuration_id: WorkspaceConfigurationId,
        cx: &mut Context<Self>,
    ) {
        let matches = self
            .workspace_configuration_snapshot(cx)
            .ok()
            .and_then(|snapshot| {
                let store = WorkspaceConfigurationStore::global(cx);
                store.configuration(configuration_id).map(|configuration| {
                    configuration
                        .members
                        .iter()
                        .map(|member| member.workspace_id)
                        .eq(snapshot.members.iter().map(|member| member.workspace_id))
                        && configuration.active_member == snapshot.active_member
                })
            })
            .unwrap_or(false);
        if matches {
            self.active_configuration_id = Some(configuration_id);
            self.configuration_checkpoint_error = None;
            cx.notify();
        }
    }

    fn resolve_workspace_configuration_target(
        configuration: &crate::persistence::model::WorkspaceConfiguration,
        source_window_id: WindowId,
        cx: &App,
    ) -> Result<Vec<WorkspaceConfigurationTargetMember>> {
        let db = crate::persistence::WorkspaceDb::global(cx);
        let mut target = Vec::with_capacity(configuration.members.len());
        for member in &configuration.members {
            let mut local_workspace = None;
            for window in cx
                .windows()
                .into_iter()
                .filter_map(|window| window.downcast::<MultiWorkspace>())
            {
                let multi_workspace = match window.read(cx) {
                    Ok(multi_workspace) => multi_workspace,
                    Err(_) => continue,
                };
                let matching_workspace = multi_workspace.workspaces().find(|workspace| {
                    workspace.read(cx).database_id() == Some(member.workspace_id)
                });
                let Some(matching_workspace) = matching_workspace else {
                    continue;
                };
                anyhow::ensure!(
                    multi_workspace.window_id == source_window_id,
                    "“{}” can’t be switched to because workspace “{}” is open in another window",
                    configuration.name,
                    Self::workspace_configuration_member_label(member)
                );
                local_workspace = Some(matching_workspace.clone());
            }

            if let Some(workspace) = local_workspace {
                target.push(WorkspaceConfigurationTargetMember::Live(workspace));
                continue;
            }

            let serialized_workspace = match db.workspace_for_id_checked(member.workspace_id)? {
                Some(workspace) => workspace,
                None => {
                    let mut workspace = db
                        .workspace_for_local_identity_paths_checked(&member.identity_paths)?
                        .with_context(|| {
                            format!(
                                "“{}” can’t be restored because workspace “{}” no longer has saved editor state",
                                configuration.name,
                                Self::workspace_configuration_member_label(member)
                            )
                        })?;
                    workspace.id = member.workspace_id;
                    workspace
                }
            };
            anyhow::ensure!(
                serialized_workspace.location
                    == crate::persistence::model::SerializedWorkspaceLocation::Local,
                "workspace configurations currently support local workspaces only"
            );
            target.push(WorkspaceConfigurationTargetMember::Serialized(
                serialized_workspace,
            ));
        }
        Ok(target)
    }

    fn target_is_unowned_elsewhere(
        configuration: &crate::persistence::model::WorkspaceConfiguration,
        target: &[Entity<Workspace>],
        source_window_id: WindowId,
        cx: &App,
    ) -> Result<()> {
        for window in cx
            .windows()
            .into_iter()
            .filter_map(|window| window.downcast::<MultiWorkspace>())
        {
            let multi_workspace = match window.read(cx) {
                Ok(multi_workspace) => multi_workspace,
                Err(_) => continue,
            };
            if multi_workspace.window_id == source_window_id {
                continue;
            }
            for (workspace, member) in target.iter().zip(&configuration.members) {
                let workspace = workspace.read(cx);
                let workspace_id = workspace.database_id();
                let workspace_identity = workspace.project_group_key(cx).path_list().clone();
                anyhow::ensure!(
                    !multi_workspace.workspaces().any(|candidate| {
                        let candidate = candidate.read(cx);
                        candidate.database_id() == workspace_id
                            || candidate.project_group_key(cx).path_list() == &workspace_identity
                    }),
                    "“{}” can’t be switched to because workspace “{}” is open in another window",
                    configuration.name,
                    Self::workspace_configuration_member_label(member)
                );
            }
        }
        Ok(())
    }

    fn workspace_configuration_member_label(member: &WorkspaceConfigurationMember) -> String {
        if member.identity_paths.is_empty() {
            return "Untitled".to_string();
        }

        member
            .identity_paths
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn commit_workspace_configuration(
        &mut self,
        configuration_id: WorkspaceConfigurationId,
        target: Vec<Entity<Workspace>>,
        active_member: WorkspaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let active_workspace = target
            .iter()
            .find(|workspace| workspace.read(cx).database_id() == Some(active_member))
            .cloned()
            .context("the target configuration has no active workspace")?;
        let previous = self.workspaces().cloned().collect::<Vec<_>>();

        for workspace in &target {
            if self.held_index(workspace).is_none() {
                self.register_workspace(workspace, window, cx);
            }
        }

        let mut project_groups = Vec::new();
        for workspace in &target {
            let key = workspace.read(cx).project_group_key(cx);
            if !key.path_list().paths().is_empty()
                && !project_groups
                    .iter()
                    .any(|group: &ProjectGroupState| group.key == key)
            {
                project_groups.push(ProjectGroupState {
                    key,
                    expanded: true,
                });
            }
        }

        self.held = target
            .iter()
            .map(|workspace| HeldWorkspace {
                workspace: workspace.clone(),
                pinned: true,
                activated_at: (workspace == &active_workspace).then_some(0),
            })
            .collect();
        self.project_groups = project_groups;
        self.active_workspace_id.set(active_workspace.entity_id());
        self.active_configuration_id = Some(configuration_id);
        self.configuration_checkpoint_error = None;
        self.workspace_configuration_generation += 1;

        for workspace in &previous {
            if !target.contains(workspace) {
                cx.emit(MultiWorkspaceEvent::WorkspaceRemoved(workspace.entity_id()));
                self.clear_workspace_session_binding(workspace, cx);
            }
        }
        for workspace in &target {
            if !previous.contains(workspace) {
                cx.emit(MultiWorkspaceEvent::WorkspaceAdded(workspace.clone()));
            }
        }
        cx.emit(MultiWorkspaceEvent::ProjectGroupsChanged);
        cx.emit(MultiWorkspaceEvent::ActiveWorkspaceChanged {
            source_workspace: None,
        });
        active_workspace.update(cx, |workspace, cx| {
            workspace.refresh_window_state(window, cx);
        });
        self.serialize(cx);
        self.focus_active_workspace(window, cx);
        cx.notify();
        Ok(())
    }

    pub fn switch_workspace_configuration(
        &mut self,
        configuration_id: WorkspaceConfigurationId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if self.active_configuration_id == Some(configuration_id)
            && self.configuration_checkpoint_error.is_none()
        {
            return Task::ready(Ok(()));
        }
        if self.configuration_switch_in_progress {
            return Task::ready(Err(anyhow::anyhow!(
                "another workspace configuration switch is already in progress"
            )));
        }
        let stale_checkpoint_retry = self
            .configuration_checkpoint_error
            .is_some()
            .then(|| self.enqueue_active_configuration_checkpoint(cx));
        let source_window_id = self.window_id;
        let source_window = match window.window_handle().downcast::<MultiWorkspace>() {
            Some(window) => window,
            None => {
                return Task::ready(Err(anyhow::anyhow!(
                    "workspace configuration switch requires a multi-workspace window"
                )));
            }
        };
        let app_state = self.workspace().read(cx).app_state().clone();
        self.configuration_switch_in_progress = true;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = async {
                if let Some(stale_checkpoint_retry) = stale_checkpoint_retry {
                    stale_checkpoint_retry
                        .await
                        .context("the active workspace configuration still has unsaved changes")?;
                    if this.read_with(cx, |this, _cx| {
                        this.active_configuration_id == Some(configuration_id)
                    })? {
                        return Ok(());
                    }
                }

                loop {
                    let (outgoing_configuration_id, outgoing_snapshot) =
                        this.read_with(cx, |this, cx| {
                            Ok::<_, anyhow::Error>((
                                this.active_configuration_id,
                                this.workspace_configuration_snapshot(cx)?,
                            ))
                        })??;
                    let target_configuration = cx.update(|_window, cx| {
                        WorkspaceConfigurationStore::global(cx)
                            .configuration(configuration_id)
                            .cloned()
                            .context("that workspace configuration no longer exists")
                    })??;
                    let target_members = cx.update(|_window, cx| {
                        Self::resolve_workspace_configuration_target(
                            &target_configuration,
                            source_window_id,
                            cx,
                        )
                    })??;

                    let mut target = Vec::with_capacity(target_members.len());
                    for member in target_members {
                        match member {
                            WorkspaceConfigurationTargetMember::Live(workspace) => {
                                target.push(workspace)
                            }
                            WorkspaceConfigurationTargetMember::Serialized(
                                serialized_workspace,
                            ) => {
                                let prepared = cx.update(|_window, cx| {
                                    Workspace::prepare_local_strict(
                                        serialized_workspace,
                                        app_state.clone(),
                                        source_window,
                                        cx,
                                    )
                                })?;
                                target.push(prepared.await?);
                            }
                        }
                    }

                    #[cfg(test)]
                    Self::reach_configuration_switch_test_pause(
                        &this,
                        WorkspaceConfigurationSwitchTestStage::TargetStaged,
                        cx,
                    )
                    .await?;

                    for workspace in &outgoing_snapshot.workspaces {
                        let should_continue = workspace
                            .update_in(cx, |workspace, window, cx| {
                                workspace.prompt_to_save_or_discard_dirty_items(window, cx)
                            })?
                            .await?;
                        if !should_continue {
                            return Err(WorkspaceConfigurationSwitchCanceled.into());
                        }
                    }

                    let outgoing_is_current = this.read_with(cx, |this, _cx| {
                        this.workspace_configuration_generation == outgoing_snapshot.generation
                    })?;
                    if !outgoing_is_current {
                        continue;
                    }

                    this.update_in(cx, |this, window, cx| {
                        this.configuration_switch_gate = Some(target_configuration.name.clone());
                        window.focus(&this.configuration_switch_gate_focus_handle, cx);
                        cx.notify();
                    })?;

                    let exact_checkpoints = cx.update(|_window, cx| {
                        outgoing_snapshot
                            .workspaces
                            .iter()
                            .map(|workspace| {
                                workspace.update(cx, |workspace, cx| workspace.checkpoint_exact(cx))
                            })
                            .collect::<Vec<_>>()
                    })?;
                    let mut first_error = None;
                    for result in futures::future::join_all(exact_checkpoints).await {
                        if let Err(error) = result
                            && first_error.is_none()
                        {
                            first_error = Some(error);
                        }
                    }
                    if let Some(error) = first_error {
                        return Err(error);
                    }

                    if let Some(outgoing_configuration_id) = outgoing_configuration_id {
                        let checkpoint = cx.update(|_window, cx| {
                            WorkspaceConfigurationStore::mutate_global(
                                WorkspaceConfigurationMutation::Checkpoint {
                                    id: outgoing_configuration_id,
                                    members: outgoing_snapshot.members.clone(),
                                    active_member: outgoing_snapshot.active_member,
                                },
                                cx,
                            )
                        })?;
                        match checkpoint.await {
                            Ok(_) => {}
                            Err(error) => {
                                let message = format!("{error:#}");
                                this.update(cx, |this, cx| {
                                    this.configuration_checkpoint_error = Some(message);
                                    this.serialize(cx);
                                    cx.notify();
                                })?;
                                return Err(error);
                            }
                        }
                    }

                    let active_member = target_configuration
                        .active_member
                        .context("the target configuration has no active workspace")?;
                    #[cfg(test)]
                    Self::reach_configuration_switch_test_pause(
                        &this,
                        WorkspaceConfigurationSwitchTestStage::BeforeCommit,
                        cx,
                    )
                    .await?;
                    this.update_in(cx, |this, window, cx| {
                        anyhow::ensure!(
                            this.workspace_configuration_generation == outgoing_snapshot.generation,
                            "the outgoing workspace set changed before switch commit"
                        );
                        let store = WorkspaceConfigurationStore::global(cx);
                        let current_target = store
                            .configuration(configuration_id)
                            .context("the target workspace configuration disappeared")?;
                        anyhow::ensure!(
                            current_target == &target_configuration,
                            "the target workspace configuration changed during restore"
                        );
                        Self::target_is_unowned_elsewhere(
                            &target_configuration,
                            &target,
                            source_window_id,
                            cx,
                        )?;
                        this.commit_workspace_configuration(
                            configuration_id,
                            target,
                            active_member,
                            window,
                            cx,
                        )
                    })??;
                    break Ok(());
                }
            }
            .await;

            let switch_failed = result.is_err();
            this.update_in(cx, |this, window, cx| {
                this.configuration_switch_in_progress = false;
                this.configuration_switch_gate = None;
                if switch_failed {
                    this.focus_active_workspace(window, cx);
                }
                cx.notify();
            })?;
            result
        })
    }

    pub fn sidebar_side(&self, cx: &App) -> SidebarSide {
        self.sidebar
            .as_ref()
            .map_or(SidebarSide::Left, |s| s.side(cx))
    }

    pub fn sidebar_render_state(&self, cx: &App) -> SidebarRenderState {
        SidebarRenderState {
            open: self.sidebar_open() && self.sidebar_ui_enabled(cx),
            side: self.sidebar_side(cx),
        }
    }

    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let release_subscription = cx.on_release(|this: &mut MultiWorkspace, _cx| {
            if let Some(task) = this._serialize_task.take() {
                task.detach();
            }
            for task in std::mem::take(&mut this.pending_removal_tasks) {
                task.detach();
            }
            if let Some(task) = this.configuration_checkpoint_queue_tail.take() {
                task.detach();
            }
        });
        let quit_subscription = cx.on_app_quit_with_timeout(
            WORKSPACE_CONFIGURATION_SHUTDOWN_TIMEOUT,
            Self::app_will_quit,
        );
        let settings_subscription = cx.observe_global_in::<settings::SettingsStore>(window, {
            let mut previous_retention_enabled = Self::retention_enabled_from_settings(cx);
            let mut previous_sidebar_ui_enabled = Self::sidebar_ui_enabled_from_settings(cx);
            move |this, window, cx| {
                let retention_enabled = this.retention_enabled(cx);
                if previous_retention_enabled && !retention_enabled {
                    this.collapse_to_single_workspace(window, cx);
                }
                previous_retention_enabled = retention_enabled;

                let sidebar_ui_enabled = this.sidebar_ui_enabled(cx);
                if previous_sidebar_ui_enabled && !sidebar_ui_enabled && this.sidebar_open() {
                    this.close_sidebar(window, cx);
                }
                previous_sidebar_ui_enabled = sidebar_ui_enabled;
            }
        });
        Self::subscribe_to_workspace(&workspace, window, cx);
        let weak_self = cx.weak_entity();
        let active_workspace_id = Rc::new(Cell::new(workspace.entity_id()));
        workspace.update(cx, |workspace, cx| {
            workspace.set_multi_workspace(weak_self, active_workspace_id.clone(), cx);
        });
        Self {
            window_id: window.window_handle().window_id(),
            held: vec![HeldWorkspace {
                workspace,
                pinned: false,
                activated_at: Some(0),
            }],
            project_groups: Vec::new(),
            active_workspace_id,
            sidebar: None,
            sidebar_open: false,
            sidebar_overlay: None,
            workspace_tabs_scroll_handle: ScrollHandle::new(),
            workspace_tabs_last_scrolled_workspace_id: Cell::new(None),
            workspace_tabs_last_scrolled_index: Cell::new(None),
            workspace_configuration_menu_handle: PopoverMenuHandle::default(),
            pending_removal_tasks: Vec::new(),
            active_configuration_id: None,
            configuration_checkpoint_error: None,
            workspace_configuration_generation: 0,
            configuration_checkpoint_queue_tail: None,
            configuration_switch_in_progress: false,
            configuration_switch_gate: None,
            configuration_switch_gate_focus_handle: cx.focus_handle(),
            #[cfg(test)]
            configuration_switch_test_pause: None,
            #[cfg(test)]
            configuration_save_test_pause: None,
            _serialize_task: None,
            _subscriptions: vec![
                release_subscription,
                quit_subscription,
                settings_subscription,
            ],
            previous_focus_handle: None,
        }
    }

    pub fn register_sidebar<T: Sidebar>(&mut self, sidebar: Entity<T>, cx: &mut Context<Self>) {
        self._subscriptions
            .push(cx.observe(&sidebar, |_this, _, cx| {
                cx.notify();
            }));
        self._subscriptions
            .push(cx.subscribe(&sidebar, |this, _, event, cx| match event {
                SidebarEvent::SerializeNeeded => {
                    this.serialize(cx);
                }
            }));
        self.sidebar = Some(Box::new(sidebar));
    }

    pub fn sidebar(&self) -> Option<&dyn SidebarHandle> {
        self.sidebar.as_deref()
    }

    pub fn set_sidebar_overlay(&mut self, overlay: Option<AnyView>, cx: &mut Context<Self>) {
        self.sidebar_overlay = overlay;
        cx.notify();
    }

    pub fn sidebar_open(&self) -> bool {
        self.sidebar_open
    }

    pub fn sidebar_has_notifications(&self, cx: &App) -> bool {
        self.sidebar
            .as_ref()
            .map_or(false, |s| s.has_notifications(cx))
    }

    pub fn is_threads_list_view_active(&self, cx: &App) -> bool {
        self.sidebar
            .as_ref()
            .map_or(false, |s| s.is_threads_list_view_active(cx))
    }

    pub fn retention_enabled_from_settings(_cx: &App) -> bool {
        true
    }

    pub fn sidebar_ui_enabled_from_settings(cx: &App) -> bool {
        !DisableAiSettings::get_global(cx).disable_ai && AgentSettings::get_global(cx).enabled
    }

    pub fn retention_enabled(&self, cx: &App) -> bool {
        Self::retention_enabled_from_settings(cx)
    }

    pub fn sidebar_ui_enabled(&self, cx: &App) -> bool {
        Self::sidebar_ui_enabled_from_settings(cx)
    }

    pub fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sidebar_ui_enabled(cx) {
            return;
        }

        if self.sidebar_open() {
            self.close_sidebar(window, cx);
        } else {
            self.previous_focus_handle = window.focused(cx);
            self.open_sidebar(cx);
            if let Some(sidebar) = &self.sidebar {
                sidebar.prepare_for_focus(window, cx);
                sidebar.focus(window, cx);
            }
        }
    }

    pub fn close_sidebar_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sidebar_ui_enabled(cx) {
            return;
        }

        if self.sidebar_open() {
            self.close_sidebar(window, cx);
        }
    }

    pub fn focus_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sidebar_ui_enabled(cx) {
            return;
        }

        if self.sidebar_open() {
            let sidebar_is_focused = self
                .sidebar
                .as_ref()
                .is_some_and(|s| s.focus_handle(cx).contains_focused(window, cx));

            if sidebar_is_focused {
                self.restore_previous_focus(false, window, cx);
            } else {
                self.previous_focus_handle = window.focused(cx);
                if let Some(sidebar) = &self.sidebar {
                    sidebar.prepare_for_focus(window, cx);
                    sidebar.focus(window, cx);
                }
            }
        } else {
            self.previous_focus_handle = window.focused(cx);
            self.open_sidebar(cx);
            if let Some(sidebar) = &self.sidebar {
                sidebar.prepare_for_focus(window, cx);
                sidebar.focus(window, cx);
            }
        }
    }

    pub fn open_sidebar(&mut self, cx: &mut Context<Self>) {
        let side = match self.sidebar_side(cx) {
            SidebarSide::Left => "left",
            SidebarSide::Right => "right",
        };
        telemetry::event!("Sidebar Toggled", action = "open", side = side);
        self.apply_open_sidebar(cx);
    }

    /// Restores the sidebar to open state from persisted session data without
    /// firing a telemetry event, since this is not a user-initiated action.
    pub(crate) fn restore_open_sidebar(&mut self, cx: &mut Context<Self>) {
        self.apply_open_sidebar(cx);
    }

    fn apply_open_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = true;
        self.retain_active_workspace(cx);
        let sidebar_focus_handle = self.sidebar.as_ref().map(|s| s.focus_handle(cx));
        for workspace in self.workspaces().cloned().collect::<Vec<_>>() {
            workspace.update(cx, |workspace, _cx| {
                workspace.set_sidebar_focus_handle(sidebar_focus_handle.clone());
            });
        }
        self.serialize(cx);
        cx.notify();
    }

    pub fn close_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let side = match self.sidebar_side(cx) {
            SidebarSide::Left => "left",
            SidebarSide::Right => "right",
        };
        telemetry::event!("Sidebar Toggled", action = "close", side = side);
        self.sidebar_open = false;
        for workspace in self.workspaces().cloned().collect::<Vec<_>>() {
            workspace.update(cx, |workspace, _cx| {
                workspace.set_sidebar_focus_handle(None);
            });
        }
        let sidebar_has_focus = self
            .sidebar
            .as_ref()
            .is_some_and(|s| s.focus_handle(cx).contains_focused(window, cx));
        if sidebar_has_focus {
            self.restore_previous_focus(true, window, cx);
        } else {
            self.previous_focus_handle.take();
        }
        self.serialize(cx);
        cx.notify();
    }

    fn restore_previous_focus(&mut self, clear: bool, window: &mut Window, cx: &mut Context<Self>) {
        let focus_handle = if clear {
            self.previous_focus_handle.take()
        } else {
            self.previous_focus_handle.clone()
        };

        if let Some(previous_focus) = focus_handle {
            previous_focus.focus(window, cx);
        } else {
            let pane = self.workspace().read(cx).active_pane().clone();
            window.focus(&pane.read(cx).focus_handle(cx), cx);
        }
    }

    pub fn close_window(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        if self.configuration_switch_in_progress {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let workspaces = this.update(cx, |multi_workspace, _cx| {
                multi_workspace.workspaces().cloned().collect::<Vec<_>>()
            })?;

            for workspace in workspaces {
                let should_continue = workspace
                    .update_in(cx, |workspace, window, cx| {
                        workspace.prepare_to_close(CloseIntent::CloseWindow, window, cx)
                    })?
                    .await?;
                if !should_continue {
                    return anyhow::Ok(());
                }
            }

            cx.update(|window, _cx| {
                window.remove_window();
            })?;

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn subscribe_to_workspace(
        workspace: &Entity<Workspace>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let project = workspace.read(cx).project().clone();
        cx.subscribe_in(&project, window, {
            let workspace = workspace.downgrade();
            move |this, _project, event, _window, cx| match event {
                project::Event::WorktreePathsChanged { old_worktree_paths } => {
                    if let Some(workspace) = workspace.upgrade() {
                        let host = workspace
                            .read(cx)
                            .project()
                            .read(cx)
                            .remote_connection_options(cx);
                        let old_key =
                            ProjectGroupKey::from_worktree_paths(old_worktree_paths, host);
                        this.handle_project_group_key_change(&workspace, &old_key, cx);
                    }
                }
                _ => {}
            }
        })
        .detach();

        cx.subscribe_in(
            workspace,
            window,
            |this, workspace, event, window, cx| match event {
                WorkspaceEvent::Activate => {
                    this.activate(workspace.clone(), None, window, cx);
                }
                WorkspaceEvent::ItemAdded { .. }
                | WorkspaceEvent::ItemDirtyStateChanged
                | WorkspaceEvent::ItemRemoved { .. } => {
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
    }

    fn handle_project_group_key_change(
        &mut self,
        workspace: &Entity<Workspace>,
        old_key: &ProjectGroupKey,
        cx: &mut Context<Self>,
    ) {
        if !self.is_workspace_retained(workspace) {
            return;
        }

        let new_key = workspace.read(cx).project_group_key(cx);
        if new_key.path_list().paths().is_empty() {
            return;
        }

        // The Project already emitted WorktreePathsChanged which the
        // sidebar handles for thread migration.
        self.rekey_project_group(old_key, &new_key, cx);
        self.workspace_configuration_changed(cx);
        self.serialize(cx);
        cx.notify();
    }

    fn held_index(&self, workspace: &Entity<Workspace>) -> Option<usize> {
        self.held
            .iter()
            .position(|held| held.workspace == *workspace)
    }

    pub fn is_workspace_retained(&self, workspace: &Entity<Workspace>) -> bool {
        self.held
            .iter()
            .any(|held| held.pinned && held.workspace == *workspace)
    }

    pub fn active_workspace_is_retained(&self) -> bool {
        self.held[self.displayed_index()].pinned
    }

    /// The displayed workspace is the most recently activated row.
    fn displayed_index(&self) -> usize {
        self.held
            .iter()
            .enumerate()
            .max_by_key(|(_, held)| held.activated_at)
            .expect("a window always holds at least one workspace")
            .0
    }

    /// Ensures a project group exists for `key`, creating one if needed.
    fn ensure_project_group_state(&mut self, key: ProjectGroupKey) {
        if key.path_list().paths().is_empty() {
            return;
        }

        if self.project_groups.iter().any(|group| group.key == key) {
            return;
        }

        self.project_groups.insert(
            0,
            ProjectGroupState {
                key,
                expanded: true,
            },
        );
    }

    /// Transitions a project group from `old_key` to `new_key`.
    ///
    /// On collision (both keys have groups), the active workspace's
    /// Re-keys a project group from `old_key` to `new_key`, handling
    /// collisions. When two groups collide, the active workspace's
    /// group always wins. Otherwise the old key's state is preserved
    /// — it represents the group the user or system just acted on.
    /// The losing group is removed, and the winner is re-keyed in
    /// place to preserve sidebar order.
    fn rekey_project_group(
        &mut self,
        old_key: &ProjectGroupKey,
        new_key: &ProjectGroupKey,
        cx: &App,
    ) {
        if old_key == new_key {
            return;
        }

        if new_key.path_list().paths().is_empty() {
            return;
        }

        let old_key_exists = self.project_groups.iter().any(|g| g.key == *old_key);
        let new_key_exists = self.project_groups.iter().any(|g| g.key == *new_key);

        if !old_key_exists {
            self.ensure_project_group_state(new_key.clone());
            return;
        }

        if new_key_exists {
            let active_key = self.workspace().read(cx).project_group_key(cx);
            if active_key == *new_key {
                self.project_groups.retain(|g| g.key != *old_key);
            } else {
                self.project_groups.retain(|g| g.key != *new_key);
                if let Some(group) = self.project_groups.iter_mut().find(|g| g.key == *old_key) {
                    group.key = new_key.clone();
                }
            }
        } else {
            if let Some(group) = self.project_groups.iter_mut().find(|g| g.key == *old_key) {
                group.key = new_key.clone();
            }
        }

        // If another retained workspace still has the old key (e.g. a
        // linked worktree workspace), re-create the old group so it
        // remains reachable in the sidebar.
        let other_workspace_needs_old_key = self
            .held
            .iter()
            .any(|held| held.pinned && held.workspace.read(cx).project_group_key(cx) == *old_key);
        if other_workspace_needs_old_key {
            self.ensure_project_group_state(old_key.clone());
        }
    }

    /// Ensures `workspace` has a row in `held`, registering it on first
    /// insert, and returns the row's index.
    fn hold(
        &mut self,
        workspace: Entity<Workspace>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> usize {
        if let Some(index) = self.held_index(&workspace) {
            return index;
        }
        self.register_workspace(&workspace, window, cx);
        self.held.push(HeldWorkspace {
            workspace,
            pinned: false,
            activated_at: None,
        });
        self.held.len() - 1
    }

    /// Pins the row so the workspace survives navigating away, recording
    /// `group` as the project group it was pinned under. No-op if already
    /// pinned.
    fn pin(&mut self, index: usize, group: ProjectGroupKey, cx: &mut Context<Self>) {
        if self.held[index].pinned {
            return;
        }
        self.held[index].pinned = true;
        self.ensure_project_group_state(group);
        cx.emit(MultiWorkspaceEvent::WorkspaceAdded(
            self.held[index].workspace.clone(),
        ));
    }

    pub(crate) fn activate_provisional_workspace(
        &mut self,
        workspace: Entity<Workspace>,
        provisional_key: ProjectGroupKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.hold(workspace.clone(), window, cx);
        self.pin(index, provisional_key, cx);
        self.activate(workspace, None, window, cx);
    }

    fn register_workspace(
        &mut self,
        workspace: &Entity<Workspace>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        Self::subscribe_to_workspace(workspace, window, cx);
        let weak_self = cx.weak_entity();
        let active_workspace_id = self.active_workspace_id.clone();
        workspace.update(cx, |workspace, cx| {
            workspace.set_multi_workspace(weak_self, active_workspace_id, cx);
        });

        let entity = cx.entity();
        cx.defer({
            let workspace = workspace.clone();
            move |cx| {
                entity.update(cx, |this, cx| {
                    this.sync_sidebar_to_workspace(&workspace, cx);
                })
            }
        });
    }

    pub fn project_group_key_for_workspace(
        &self,
        workspace: &Entity<Workspace>,
        cx: &App,
    ) -> ProjectGroupKey {
        let workspace_key = workspace.read(cx).project_group_key(cx);
        self.project_group_key_covering_workspace_key(&workspace_key)
            .cloned()
            .unwrap_or(workspace_key)
    }

    pub fn restore_project_groups(
        &mut self,
        groups: Vec<SerializedProjectGroupState>,
        cx: &mut Context<Self>,
    ) {
        let retained_group_keys = self
            .workspaces()
            .map(|workspace| workspace.read(cx).project_group_key(cx))
            .collect::<Vec<_>>();
        let mut restored: Vec<ProjectGroupState> = Vec::new();
        for SerializedProjectGroupState { key, expanded } in groups {
            if key.path_list().paths().is_empty() {
                continue;
            }
            if restored.iter().any(|group| group.key.matches(&key)) {
                continue;
            }
            restored.push(ProjectGroupState { key, expanded });
        }
        for key in retained_group_keys {
            if key.path_list().paths().is_empty() {
                continue;
            }
            if restored
                .iter()
                .any(|group| project_group_covers_workspace_key(&group.key, &key))
            {
                continue;
            }
            restored.push(ProjectGroupState {
                key,
                expanded: true,
            });
        }
        self.project_groups = restored;
    }

    pub fn project_group_keys(&self) -> Vec<ProjectGroupKey> {
        self.project_groups
            .iter()
            .map(|group| group.key.clone())
            .collect()
    }

    fn project_group_index_for_key(&self, key: &ProjectGroupKey) -> Option<usize> {
        self.project_groups
            .iter()
            .position(|group| group.key == *key)
            .or_else(|| {
                self.project_groups
                    .iter()
                    .position(|group| project_group_covers_workspace_key(&group.key, key))
            })
    }

    fn project_group_key_covering_workspace_key(
        &self,
        workspace_key: &ProjectGroupKey,
    ) -> Option<&ProjectGroupKey> {
        self.project_groups
            .iter()
            .find(|group| group.key.matches(workspace_key))
            .or_else(|| {
                self.project_groups
                    .iter()
                    .find(|group| project_group_covers_workspace_key(&group.key, workspace_key))
            })
            .map(|group| &group.key)
    }

    pub fn project_groups(&self, cx: &App) -> Vec<ProjectGroup> {
        self.project_groups
            .iter()
            .map(|group| ProjectGroup {
                key: group.key.clone(),
                workspaces: self.workspaces_for_project_group(&group.key, cx),
                expanded: group.expanded,
            })
            .collect()
    }

    pub fn last_active_workspace_for_group(
        &self,
        key: &ProjectGroupKey,
        cx: &App,
    ) -> Option<Entity<Workspace>> {
        let resolved_key = self
            .project_group_index_for_key(key)
            .and_then(|index| self.project_groups.get(index))
            .map(|group| &group.key)
            .unwrap_or(key);
        self.held
            .iter()
            .filter(|held| {
                self.project_group_key_for_workspace(&held.workspace, cx) == *resolved_key
            })
            .filter_map(|held| Some((held.activated_at?, &held.workspace)))
            .max_by_key(|(activated_at, _)| *activated_at)
            .map(|(_, workspace)| workspace.clone())
    }

    pub fn group_state_by_key(&self, key: &ProjectGroupKey) -> Option<&ProjectGroupState> {
        self.project_group_index_for_key(key)
            .and_then(|index| self.project_groups.get(index))
    }

    pub fn group_state_by_key_mut(
        &mut self,
        key: &ProjectGroupKey,
    ) -> Option<&mut ProjectGroupState> {
        let index = self.project_group_index_for_key(key)?;
        self.project_groups.get_mut(index)
    }

    pub fn set_all_groups_expanded(&mut self, expanded: bool) {
        for group in &mut self.project_groups {
            group.expanded = expanded;
        }
    }

    pub fn move_project_group_up(&mut self, key: &ProjectGroupKey, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.project_group_index_for_key(key) else {
            return false;
        };
        if index == 0 {
            return false;
        }
        self.project_groups.swap(index - 1, index);
        cx.emit(MultiWorkspaceEvent::ProjectGroupsChanged);
        self.workspace_configuration_changed(cx);
        self.serialize(cx);
        cx.notify();
        true
    }

    pub fn move_project_group_down(
        &mut self,
        key: &ProjectGroupKey,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.project_group_index_for_key(key) else {
            return false;
        };
        if index + 1 >= self.project_groups.len() {
            return false;
        }
        self.project_groups.swap(index, index + 1);
        cx.emit(MultiWorkspaceEvent::ProjectGroupsChanged);
        self.workspace_configuration_changed(cx);
        self.serialize(cx);
        cx.notify();
        true
    }

    pub fn move_project_group_to_index(
        &mut self,
        key: &ProjectGroupKey,
        target_index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(current_index) = self.project_group_index_for_key(key) else {
            return false;
        };
        if current_index == target_index {
            return false;
        }

        let group = self.project_groups.remove(current_index);
        let target_index = target_index.min(self.project_groups.len());
        self.project_groups.insert(target_index, group);
        cx.emit(MultiWorkspaceEvent::ProjectGroupsChanged);
        self.workspace_configuration_changed(cx);
        self.serialize(cx);
        cx.notify();
        true
    }

    pub fn move_workspace_tab_to_index(
        &mut self,
        workspace: &Entity<Workspace>,
        target_index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut ordered_workspaces = self.ordered_workspaces(cx);
        let Some(current_index) = ordered_workspaces
            .iter()
            .position(|candidate| candidate == workspace)
        else {
            return false;
        };
        if current_index == target_index {
            return false;
        }

        let workspace = ordered_workspaces.remove(current_index);
        let target_index = target_index.min(ordered_workspaces.len());
        ordered_workspaces.insert(target_index, workspace);

        let mut desired_group_keys = Vec::new();
        for workspace in &ordered_workspaces {
            let key = self.project_group_key_for_workspace(workspace, cx);
            if !desired_group_keys.contains(&key) {
                desired_group_keys.push(key);
            }
        }

        let mut remaining_project_groups = std::mem::take(&mut self.project_groups);
        let mut reordered_project_groups = Vec::with_capacity(remaining_project_groups.len());
        for desired_key in desired_group_keys {
            if let Some(index) = remaining_project_groups
                .iter()
                .position(|group| group.key == desired_key)
            {
                reordered_project_groups.push(remaining_project_groups.remove(index));
            }
        }
        reordered_project_groups.extend(remaining_project_groups);
        self.project_groups = reordered_project_groups;

        // `held` replaced the fork's flat `retained_workspaces` list, and its
        // order decides `ordered_workspaces` within a project group. Move whole
        // entries so `pinned` and `activated_at` travel with each workspace;
        // `displayed_index` reads `activated_at`, not position, so which
        // workspace is on screen does not change.
        let mut old_held = std::mem::take(&mut self.held);
        let mut reordered_held = Vec::with_capacity(old_held.len());
        for workspace in &ordered_workspaces {
            if let Some(index) = old_held
                .iter()
                .position(|held| &held.workspace == workspace)
            {
                reordered_held.push(old_held.remove(index));
            }
        }
        reordered_held.extend(old_held);
        self.held = reordered_held;

        cx.emit(MultiWorkspaceEvent::ProjectGroupsChanged);
        self.workspace_configuration_changed(cx);
        self.serialize(cx);
        cx.notify();
        true
    }

    pub fn workspaces_for_project_group(
        &self,
        key: &ProjectGroupKey,
        cx: &App,
    ) -> Vec<Entity<Workspace>> {
        let resolved_key = self
            .project_group_index_for_key(key)
            .and_then(|index| self.project_groups.get(index))
            .map(|group| &group.key)
            .unwrap_or(key);
        self.held
            .iter()
            .filter(|held| held.pinned)
            .map(|held| &held.workspace)
            .filter(|workspace| {
                self.project_group_key_for_workspace(workspace, cx) == *resolved_key
            })
            .cloned()
            .collect()
    }

    pub fn remove_project_group(
        &mut self,
        group_key: &ProjectGroupKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool>> {
        // The active workspace can remain unpinned while the sidebar is
        // closed. Pin it first: this puts it in the removal set below, and
        // stops `activate` from pinning it while switching to the
        // replacement, which would recreate the project group row this
        // function just deleted.
        let active_workspace = self.workspace().clone();
        if active_workspace.read(cx).project_group_key(cx) == *group_key
            && !self.is_workspace_retained(&active_workspace)
        {
            let index = self.hold(active_workspace, window, cx);
            self.pin(index, group_key.clone(), cx);
        }

        let workspaces = self.workspaces_for_project_group(group_key, cx);

        let task = self.remove(workspaces, RemovalIntent::CloseProject, window, cx);

        self.project_groups.retain(|group| group.key != *group_key);
        cx.emit(MultiWorkspaceEvent::ProjectGroupsChanged);

        task
    }

    /// Returns the nearest retained workspace outside the project group at
    /// `group_index`.
    ///
    /// Searches project groups by increasing distance, preferring the following
    /// group over the preceding group at equal distances. Within each group,
    /// prefers its last active workspace before falling back to any retained
    /// workspace. Workspaces in `excluded_workspaces` are ignored by both
    /// lookups.
    ///
    /// The `group_index` must identify a project group that is still present in
    /// [`Self::project_groups`].
    /// The keys of the other project groups, nearest first, preferring the
    /// following group over the preceding group at equal distances. Without an
    /// index, every group key in display order.
    fn neighbor_group_keys(&self, group_index: Option<usize>) -> Vec<ProjectGroupKey> {
        let Some(index) = group_index else {
            return self
                .project_groups
                .iter()
                .map(|group| group.key.clone())
                .collect();
        };

        (1..self.project_groups.len())
            .flat_map(|distance| [index.checked_add(distance), index.checked_sub(distance)])
            .flatten()
            .filter_map(|index| self.project_groups.get(index))
            .map(|group| group.key.clone())
            .collect()
    }

    #[cfg(test)]
    pub(super) fn nearest_retained_workspace(
        &self,
        group_index: usize,
        excluded_workspaces: &[Entity<Workspace>],
        cx: &App,
    ) -> Option<Entity<Workspace>> {
        self.neighbor_group_keys(Some(group_index))
            .into_iter()
            .find_map(|key| self.live_member_for_group(&key, excluded_workspaces, cx))
    }

    /// Goes through sqlite: serialize -> close -> open new window
    /// This avoids issues with pending tasks having the wrong window
    pub fn open_project_group_in_new_window(
        &mut self,
        key: &ProjectGroupKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let paths: Vec<PathBuf> = key.path_list().ordered_paths().cloned().collect();
        if paths.is_empty() {
            return Task::ready(Ok(()));
        }

        let app_state = self.workspace().read(cx).app_state().clone();

        let workspaces: Vec<_> = self.workspaces_for_project_group(key, cx);
        let mut serialization_tasks = Vec::new();
        for workspace in &workspaces {
            serialization_tasks.push(workspace.update(cx, |workspace, inner_cx| {
                workspace.flush_serialization(window, inner_cx)
            }));
        }

        let remove_task = self.remove_project_group(key, window, cx);

        cx.spawn(async move |_this, cx| {
            futures::future::join_all(serialization_tasks).await;

            let removed = remove_task.await?;
            if !removed {
                return Ok(());
            }

            cx.update(|cx| {
                Workspace::new_local(paths, app_state, None, None, None, OpenMode::NewWindow, cx)
            })
            .await?;

            Ok(())
        })
    }

    /// Finds an existing workspace whose root paths and host exactly match.
    pub fn workspace_for_paths(
        &self,
        path_list: &PathList,
        host: Option<&RemoteConnectionOptions>,
        cx: &App,
    ) -> Option<Entity<Workspace>> {
        for workspace in self.workspaces() {
            let root_paths = PathList::new(&workspace.read(cx).root_paths(cx));
            let key = workspace.read(cx).project_group_key(cx);
            let host_matches = key.host().as_ref() == host;
            let paths_match = root_paths == *path_list;
            if host_matches && paths_match {
                return Some(workspace.clone());
            }
        }

        None
    }

    /// Finds an existing workspace whose paths match, or creates a new one.
    ///
    /// For local projects (`host` is `None`), this delegates to
    /// [`Self::find_or_create_local_workspace`]. For remote projects, it
    /// tries an exact path match and, if no existing workspace is found,
    /// calls `connect_remote` to establish a connection and creates a new
    /// remote workspace.
    ///
    /// The `connect_remote` closure is responsible for any user-facing
    /// connection UI (e.g. password prompts). It receives the connection
    /// options and should return a [`Task`] that resolves to the
    /// [`RemoteClient`] session, or `None` if the connection was
    /// cancelled.
    pub fn find_or_create_workspace(
        &mut self,
        paths: PathList,
        host: Option<RemoteConnectionOptions>,
        provisional_project_group_key: Option<ProjectGroupKey>,
        connect_remote: impl FnOnce(
            RemoteConnectionOptions,
            &mut Window,
            &mut Context<Self>,
        ) -> Task<Result<Option<Entity<remote::RemoteClient>>>>
        + 'static,
        init: Option<Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send>>,
        open_mode: OpenMode,
        source_workspace: Option<WeakEntity<Workspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Workspace>>> {
        if let Some(workspace) = self.workspace_for_paths(&paths, host.as_ref(), cx) {
            self.activate(workspace.clone(), source_workspace, window, cx);
            return Task::ready(Ok(workspace));
        }

        let Some(connection_options) = host else {
            return self.find_or_create_local_workspace(
                paths,
                provisional_project_group_key,
                init,
                open_mode,
                source_workspace,
                window,
                cx,
            );
        };

        let app_state = self.workspace().read(cx).app_state().clone();
        let window_handle = window.window_handle().downcast::<MultiWorkspace>();
        let connect_task = connect_remote(connection_options.clone(), window, cx);
        let paths_vec = paths.paths().to_vec();

        cx.spawn(async move |_this, cx| {
            let session = connect_task
                .await?
                .ok_or_else(|| anyhow::anyhow!("Remote connection was cancelled"))?;

            let new_project = cx.update(|cx| {
                Project::remote(
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

            let effective_paths_vec =
                if let Some(project_group) = provisional_project_group_key.as_ref() {
                    let resolve_tasks = cx.update(|cx| {
                        let project = new_project.read(cx);
                        paths_vec
                            .iter()
                            .map(|path| project.resolve_abs_path(&path.to_string_lossy(), cx))
                            .collect::<Vec<_>>()
                    });
                    let resolved = futures::future::join_all(resolve_tasks).await;
                    // `resolve_abs_path` returns `None` for both "definitely
                    // absent" and transport errors (it swallows the error via
                    // `log_err`). This is a weaker guarantee than the local
                    // `Ok(None)` check, but it matches how the rest of the
                    // codebase consumes this API.
                    let all_paths_missing =
                        !paths_vec.is_empty() && resolved.iter().all(|resolved| resolved.is_none());

                    if all_paths_missing {
                        project_group.path_list().paths().to_vec()
                    } else {
                        paths_vec
                    }
                } else {
                    paths_vec
                };

            let window_handle =
                window_handle.ok_or_else(|| anyhow::anyhow!("Window is not a MultiWorkspace"))?;

            let (workspace, _items) = open_remote_project_with_existing_connection(
                connection_options,
                new_project,
                effective_paths_vec,
                app_state,
                window_handle,
                provisional_project_group_key,
                source_workspace,
                cx,
            )
            .await?;

            window_handle.update(cx, |multi_workspace, window, cx| {
                multi_workspace.add(workspace.clone(), window, cx);
                workspace
            })
        })
    }

    /// Finds an existing workspace in this multi-workspace whose paths match,
    /// or creates a new one (deserializing its saved state from the database).
    /// Never searches other windows or matches workspaces with a superset of
    /// the requested paths.
    pub fn find_or_create_local_workspace(
        &mut self,
        path_list: PathList,
        project_group: Option<ProjectGroupKey>,
        init: Option<Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send>>,
        open_mode: OpenMode,
        source_workspace: Option<WeakEntity<Workspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Workspace>>> {
        if let Some(workspace) = self.workspace_for_paths(&path_list, None, cx) {
            self.activate(workspace.clone(), source_workspace, window, cx);
            return Task::ready(Ok(workspace));
        }

        let paths = path_list.paths().to_vec();
        let app_state = self.workspace().read(cx).app_state().clone();
        let requesting_window = window.window_handle().downcast::<MultiWorkspace>();
        let fs = <dyn Fs>::global(cx);

        cx.spawn(async move |_this, cx| {
            let effective_path_list = if let Some(project_group) = project_group {
                let metadata_tasks: Vec<_> = paths
                    .iter()
                    .map(|path| fs.metadata(path.as_path()))
                    .collect();
                let metadata_results = futures::future::join_all(metadata_tasks).await;
                // Only fall back when every path is definitely absent; real
                // filesystem errors should not be treated as "missing".
                let all_paths_missing = !paths.is_empty()
                    && metadata_results
                        .into_iter()
                        // Ok(None) means the path is definitely absent
                        .all(|result| matches!(result, Ok(None)));

                if all_paths_missing {
                    project_group.path_list().clone()
                } else {
                    PathList::new(&paths)
                }
            } else {
                PathList::new(&paths)
            };

            if let Some(requesting_window) = requesting_window
                && let Some(workspace) = requesting_window
                    .update(cx, |multi_workspace, window, cx| {
                        multi_workspace
                            .workspace_for_paths(&effective_path_list, None, cx)
                            .inspect(|workspace| {
                                multi_workspace.activate(
                                    workspace.clone(),
                                    source_workspace.clone(),
                                    window,
                                    cx,
                                );
                            })
                    })
                    .ok()
                    .flatten()
            {
                return Ok(workspace);
            }

            let result = cx
                .update(|cx| {
                    Workspace::new_local(
                        effective_path_list.paths().to_vec(),
                        app_state,
                        requesting_window,
                        None,
                        init,
                        open_mode,
                        cx,
                    )
                })
                .await?;
            Ok(result.workspace)
        })
    }

    pub fn workspace(&self) -> &Entity<Workspace> {
        &self.held[self.displayed_index()].workspace
    }

    pub fn workspaces(&self) -> impl Iterator<Item = &Entity<Workspace>> {
        self.held.iter().map(|held| &held.workspace)
    }

    pub(crate) fn ordered_workspaces(&self, cx: &App) -> Vec<Entity<Workspace>> {
        let workspaces = self.workspaces().cloned().collect::<Vec<_>>();
        let mut ordered = Vec::with_capacity(workspaces.len());

        for group in &self.project_groups {
            for workspace in &workspaces {
                if ordered.contains(workspace) {
                    continue;
                }

                let workspace_key = workspace.read(cx).project_group_key(cx);
                if project_group_covers_workspace_key(&group.key, &workspace_key) {
                    ordered.push(workspace.clone());
                }
            }
        }

        for workspace in workspaces {
            if !ordered.contains(&workspace) {
                ordered.push(workspace);
            }
        }

        ordered
    }

    /// Adds a workspace to this window as persistent without changing which
    /// workspace is active. Unlike `activate()`, this always inserts into the
    /// persistent list regardless of sidebar state — it's used for system-
    /// initiated additions like deserialization and worktree discovery.
    pub fn add(&mut self, workspace: Entity<Workspace>, window: &Window, cx: &mut Context<Self>) {
        if self.is_workspace_retained(&workspace) {
            return;
        }
        let key = workspace.read(cx).project_group_key(cx);
        let index = self.hold(workspace, window, cx);
        self.pin(index, key, cx);
        telemetry::event!(
            "Workspace Added",
            workspace_count = self.held.iter().filter(|held| held.pinned).count()
        );
        self.workspace_configuration_changed(cx);
        cx.notify();
    }

    /// Ensures the workspace is in the multiworkspace and makes it the active one.
    pub fn activate(
        &mut self,
        workspace: Entity<Workspace>,
        source_workspace: Option<WeakEntity<Workspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace() == &workspace {
            self.focus_active_workspace(window, cx);
            return;
        }

        let old_active_workspace = self.workspace().clone();
        let old_active_was_retained = self.active_workspace_is_retained();
        let should_retain_workspaces = self.retention_enabled(cx);

        if should_retain_workspaces && !old_active_was_retained {
            let key = old_active_workspace.read(cx).project_group_key(cx);
            let index = self.hold(old_active_workspace.clone(), window, cx);
            self.pin(index, key, cx);
        }

        let displayed = self.hold(workspace.clone(), window, cx);
        if should_retain_workspaces {
            let key = workspace.read(cx).project_group_key(cx);
            self.pin(displayed, key, cx);
        }

        // Publish the new active workspace before anyone reads the shared cell
        // to decide who owns the window chrome.
        self.active_workspace_id.set(workspace.entity_id());

        let stamp = self
            .held
            .iter()
            .filter_map(|held| held.activated_at)
            .max()
            .map_or(0, |max| max + 1);
        self.held[displayed].activated_at = Some(stamp);

        if !should_retain_workspaces && !old_active_was_retained {
            self.detach_workspace(&old_active_workspace, cx);
        }

        // The platform window is shared across all workspaces in this window.
        // The previously-active workspace left the title and edited indicator
        // reflecting its own state, so re-apply them from the newly-active
        // workspace (which is now the chrome owner per `owns_window_chrome`).
        workspace.update(cx, |workspace, cx| {
            workspace.refresh_window_state(window, cx);
        });

        cx.emit(MultiWorkspaceEvent::ActiveWorkspaceChanged { source_workspace });
        self.workspace_configuration_changed(cx);
        self.serialize(cx);
        self.focus_active_workspace(window, cx);
        cx.notify();
    }

    /// Promotes the currently active workspace to persistent if it is
    /// transient, so it is retained across workspace switches even when
    /// the sidebar is closed. No-op if the workspace is already persistent.
    pub fn retain_active_workspace(&mut self, cx: &mut Context<Self>) {
        let index = self.displayed_index();
        if self.held[index].pinned {
            return;
        }
        let key = self.held[index].workspace.read(cx).project_group_key(cx);
        self.pin(index, key, cx);
        self.workspace_configuration_changed(cx);
        self.serialize(cx);
        cx.notify();
    }

    /// Collapses to a single workspace, discarding all groups.
    /// Used when multi-workspace is disabled by settings.
    fn collapse_to_single_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_open {
            self.close_sidebar(window, cx);
        }

        let displayed_workspace = self.workspace().clone();
        for workspace in self.workspaces().cloned().collect::<Vec<_>>() {
            if workspace != displayed_workspace {
                self.detach_workspace(&workspace, cx);
            }
        }

        for held in &mut self.held {
            held.pinned = false;
        }
        self.project_groups.clear();
        self.workspace_configuration_changed(cx);
        cx.notify();
    }

    /// Detaches a workspace: clears session state, DB binding, cached
    /// group key, and emits `WorkspaceRemoved`. The DB row is preserved
    /// so the workspace still appears in the recent-projects list.
    fn detach_workspace(&mut self, workspace: &Entity<Workspace>, cx: &mut Context<Self>) {
        if let Some(index) = self.held_index(workspace) {
            assert_ne!(
                index,
                self.displayed_index(),
                "the displayed workspace must be re-pointed before it is detached"
            );
            self.held.remove(index);
        }
        cx.emit(MultiWorkspaceEvent::WorkspaceRemoved(workspace.entity_id()));
        self.workspace_configuration_changed(cx);
        self.clear_workspace_session_binding(workspace, cx);
    }

    fn clear_workspace_session_binding(
        &mut self,
        workspace: &Entity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        workspace.update(cx, |workspace, _cx| {
            workspace.session_id.take();
            workspace._schedule_serialize_workspace.take();
            workspace._serialize_workspace_task.take();
        });

        if let Some(workspace_id) = workspace.read(cx).database_id() {
            let db = crate::persistence::WorkspaceDb::global(cx);
            self.pending_removal_tasks.retain(|task| !task.is_ready());
            self.pending_removal_tasks
                .push(cx.background_spawn(async move {
                    db.set_session_binding(workspace_id, None, None)
                        .await
                        .log_err();
                }));
        }
    }

    fn sync_sidebar_to_workspace(&self, workspace: &Entity<Workspace>, cx: &mut Context<Self>) {
        if self.sidebar_open() {
            let sidebar_focus_handle = self.sidebar.as_ref().map(|s| s.focus_handle(cx));
            workspace.update(cx, |workspace, _| {
                workspace.set_sidebar_focus_handle(sidebar_focus_handle);
            });
        }
    }

    fn multi_workspace_state(&self, cx: &App) -> MultiWorkspaceState {
        MultiWorkspaceState {
            active_workspace_id: self.workspace().read(cx).database_id(),
            project_groups: self
                .project_groups
                .iter()
                .map(|group| {
                    crate::persistence::model::SerializedProjectGroup::from_group(
                        &group.key,
                        group.expanded,
                    )
                })
                .collect::<Vec<_>>(),
            sidebar_open: self.sidebar_open,
            sidebar_state: self.sidebar.as_ref().and_then(|s| s.serialized_state(cx)),
            active_configuration_id: self
                .configuration_checkpoint_error
                .is_none()
                .then_some(self.active_configuration_id)
                .flatten(),
        }
    }

    pub fn serialize(&mut self, cx: &mut Context<Self>) {
        let previous = self._serialize_task.take();
        self._serialize_task = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let Some((window_id, state)) = this
                .read_with(cx, |this, cx| {
                    (this.window_id, this.multi_workspace_state(cx))
                })
                .ok()
            else {
                return;
            };
            let kvp = cx.update(|cx| db::kvp::KeyValueStore::global(cx));
            crate::persistence::write_multi_workspace_state(&kvp, window_id, state).await;
        }));
    }

    /// Returns the in-flight serialization task (if any) so the caller can
    /// await it. Used by the quit handler to ensure pending DB writes
    /// complete before the process exits.
    pub fn flush_serialization(&mut self) -> Task<()> {
        self._serialize_task.take().unwrap_or(Task::ready(()))
    }

    /// `#zed-37` treats app quit as a first-class restore path: flush serialization for
    /// every retained workspace, capture a live snapshot for the active one, and rebind
    /// each retained row to this window's session so restart reopens them all. Upstream's
    /// version only drains pending tasks, which leaves stale rows split across window ids.
    fn app_will_quit(&mut self, cx: &mut Context<Self>) -> impl Future<Output = ()> + use<> {
        let mut tasks: Vec<Task<()>> = Vec::new();
        let mut configuration_checkpoint = None;
        let active_configuration_id = self.active_configuration_id;
        if let Some(configuration_id) = active_configuration_id {
            self.configuration_checkpoint_queue_tail.take();

            let checkpoint = match self.workspace_configuration_snapshot(cx) {
                Ok(snapshot) => {
                    let exact_checkpoints = cx.with_window(cx.entity_id(), |window, cx| {
                        snapshot
                            .workspaces
                            .iter()
                            .map(|workspace| {
                                workspace.update(cx, |workspace, cx| {
                                    workspace.checkpoint_exact_for_shutdown(window, cx)
                                })
                            })
                            .collect::<Vec<_>>()
                    });
                    match exact_checkpoints {
                        Some(exact_checkpoints) => {
                            let prerequisite = cx.background_spawn(async move {
                                let mut first_error = None;
                                for result in futures::future::join_all(exact_checkpoints).await {
                                    if let Err(error) = result
                                        && first_error.is_none()
                                    {
                                        first_error = Some(error);
                                    }
                                }
                                match first_error {
                                    Some(error) => Err(error),
                                    None => Ok(()),
                                }
                            });
                            WorkspaceConfigurationStore::checkpoint_after_for_shutdown(
                                prerequisite,
                                configuration_id,
                                snapshot.members,
                                snapshot.active_member,
                                cx,
                            )
                        }
                        None => Box::pin(async {
                            Err(anyhow::anyhow!(
                                "the workspace configuration window closed before shutdown checkpointing"
                            ))
                        }),
                    }
                }
                Err(error) => Box::pin(async move { Err(error) }),
            };

            let previous_state_write = self._serialize_task.take();
            let mut state = self.multi_workspace_state(cx);
            let window_id = self.window_id;
            let kvp = db::kvp::KeyValueStore::global(cx);
            configuration_checkpoint = Some(async move {
                if let Some(previous_state_write) = previous_state_write {
                    previous_state_write.await;
                }
                match checkpoint.await {
                    Ok(commit) => state.active_configuration_id = Some(commit.id),
                    Err(error) => {
                        state.active_configuration_id = None;
                        log::error!(
                            "failed to checkpoint workspace configuration on quit: {error:#}"
                        );
                    }
                }
                crate::persistence::write_multi_workspace_state(&kvp, window_id, state).await;
            });
        } else if let Some(task) = self._serialize_task.take() {
            tasks.push(task);
        }
        let session_id = self.workspace().read(cx).session_id();
        let window_id = self.window_id;
        let workspaces = self.workspaces().cloned().collect::<Vec<_>>();
        if active_configuration_id.is_none() {
            let active_workspace = self.workspace().clone();
            let active_workspace_id = active_workspace.entity_id();
            let active_workspace_snapshot = cx.with_window(cx.entity_id(), |window, cx| {
                active_workspace
                    .read(cx)
                    .shutdown_serialization_snapshot(window, cx)
            });

            for workspace in &workspaces {
                let snapshot = (workspace.entity_id() == active_workspace_id)
                    .then(|| active_workspace_snapshot.clone())
                    .flatten();
                tasks.push(workspace.update(cx, |workspace, cx| {
                    workspace.flush_session_serialization_for_shutdown(window_id, snapshot, cx)
                }));
            }
        }

        let window_id = window_id.as_u64();
        let db = crate::persistence::WorkspaceDb::global(cx);
        for workspace in workspaces {
            if let Some(database_id) = workspace.read(cx).database_id() {
                let db = db.clone();
                let session_id = session_id.clone();
                tasks.push(cx.background_spawn(async move {
                    db.set_session_binding(database_id, session_id, Some(window_id))
                        .await
                        .log_err();
                }));
            }
        }
        tasks.extend(std::mem::take(&mut self.pending_removal_tasks));

        async move {
            if let Some(configuration_checkpoint) = configuration_checkpoint {
                configuration_checkpoint.await;
            }
            futures::future::join_all(tasks).await;
        }
    }

    pub fn focus_active_workspace(&self, window: &mut Window, cx: &mut App) {
        // If a dock panel is zoomed, focus it instead of the center pane.
        // Otherwise, focusing the center pane triggers dismiss_zoomed_items_to_reveal
        // which closes the zoomed dock.
        let focus_handle = {
            let workspace = self.workspace().read(cx);
            let mut target = None;
            for dock in workspace.all_docks() {
                let dock = dock.read(cx);
                if dock.is_open() {
                    if let Some(panel) = dock.active_panel() {
                        if panel.is_zoomed(window, cx) {
                            target = Some(panel.activation_focus_handle(cx));
                            break;
                        }
                    }
                }
            }
            target.unwrap_or_else(|| {
                let pane = workspace.active_pane().clone();
                pane.read(cx).focus_handle(cx)
            })
        };
        window.focus(&focus_handle, cx);
    }

    pub fn panel<T: Panel>(&self, cx: &App) -> Option<Entity<T>> {
        self.workspace().read(cx).panel::<T>(cx)
    }

    pub fn active_modal<V: ManagedView + 'static>(&self, cx: &App) -> Option<Entity<V>> {
        self.workspace().read(cx).active_modal::<V>(cx)
    }

    pub fn add_panel<T: Panel>(
        &mut self,
        panel: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace().update(cx, |workspace, cx| {
            workspace.add_panel(panel, window, cx);
        });
    }

    pub fn focus_panel<T: Panel>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<T>> {
        self.workspace()
            .update(cx, |workspace, cx| workspace.focus_panel::<T>(window, cx))
    }

    // used in a test
    pub fn toggle_modal<V: ModalView, B>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: B,
    ) where
        B: FnOnce(&mut Window, &mut gpui::Context<V>) -> V,
    {
        self.workspace().update(cx, |workspace, cx| {
            workspace.toggle_modal(window, cx, build);
        });
    }

    pub fn toggle_dock(
        &mut self,
        dock_side: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace().update(cx, |workspace, cx| {
            workspace.toggle_dock(dock_side, window, cx);
        });
    }

    pub fn active_item_as<I: 'static>(&self, cx: &App) -> Option<Entity<I>> {
        self.workspace().read(cx).active_item_as::<I>(cx)
    }

    pub fn items_of_type<'a, T: Item>(
        &'a self,
        cx: &'a App,
    ) -> impl 'a + Iterator<Item = Entity<T>> {
        self.workspace().read(cx).items_of_type::<T>(cx)
    }

    pub fn database_id(&self, cx: &App) -> Option<WorkspaceId> {
        self.workspace().read(cx).database_id()
    }

    pub fn take_pending_removal_tasks(&mut self) -> Vec<Task<()>> {
        let tasks: Vec<Task<()>> = std::mem::take(&mut self.pending_removal_tasks)
            .into_iter()
            .filter(|task| !task.is_ready())
            .collect();
        tasks
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_expand_all_groups(&mut self) {
        self.set_all_groups_expanded(true);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn assert_project_group_key_integrity(&self, cx: &App) -> anyhow::Result<()> {
        let mut retained_ids: collections::HashSet<EntityId> = Default::default();
        for workspace in self
            .held
            .iter()
            .filter(|held| held.pinned)
            .map(|held| &held.workspace)
        {
            anyhow::ensure!(
                retained_ids.insert(workspace.entity_id()),
                "workspace {:?} is retained more than once",
                workspace.entity_id(),
            );

            let live_key = workspace.read(cx).project_group_key(cx);
            anyhow::ensure!(
                self.project_groups
                    .iter()
                    .any(|group| group.key == live_key),
                "workspace {:?} has live key {:?} but no project-group metadata",
                workspace.entity_id(),
                live_key,
            );
        }
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_random_database_id(&mut self, cx: &mut Context<Self>) {
        self.workspace().update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_window_id(&self) -> WindowId {
        self.window_id
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_new(project: Entity<Project>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = cx.new(|cx| Workspace::test_new(project, window, cx));
        Self::new(workspace, window, cx)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_add_workspace(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Workspace> {
        let workspace = cx.new(|cx| Workspace::test_new(project, window, cx));
        self.activate(workspace.clone(), None, window, cx);
        workspace
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_add_project_group(&mut self, group: ProjectGroup) {
        self.project_groups.push(ProjectGroupState {
            key: group.key,
            expanded: group.expanded,
        });
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn create_test_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let app_state = self.workspace().read(cx).app_state().clone();
        let project = Project::local(
            app_state.client.clone(),
            app_state.node_runtime.clone(),
            app_state.user_store.clone(),
            app_state.languages.clone(),
            app_state.fs.clone(),
            None,
            project::LocalProjectFlags::default(),
            cx,
        );
        let new_workspace = cx.new(|cx| Workspace::new(None, project, app_state, window, cx));
        self.activate(new_workspace.clone(), None, window, cx);

        let weak_workspace = new_workspace.downgrade();
        let db = crate::persistence::WorkspaceDb::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            let workspace_id = db.next_id().await.unwrap();
            let workspace = weak_workspace.upgrade().unwrap();
            let task: Task<()> = this
                .update_in(cx, |this, window, cx| {
                    let session_id = workspace.read(cx).session_id();
                    let window_id = window.window_handle().window_id().as_u64();
                    workspace.update(cx, |workspace, _cx| {
                        workspace.set_database_id(workspace_id);
                    });
                    this.serialize(cx);
                    let db = db.clone();
                    cx.background_spawn(async move {
                        db.set_session_binding(workspace_id, session_id, Some(window_id))
                            .await
                            .log_err();
                    })
                })
                .unwrap();
            task.await
        })
    }

    /// Assigns random database IDs to all retained workspaces, flushes
    /// workspace serialization (SQLite) and multi-workspace state (KVP),
    /// and writes session bindings so the serialized data can be read
    /// back by `last_session_workspace_locations` +
    /// `read_serialized_multi_workspaces`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn flush_all_serialization(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Task<()>> {
        for workspace in self.workspaces() {
            workspace.update(cx, |ws, _cx| {
                if ws.database_id().is_none() {
                    ws.set_random_database_id();
                }
            });
        }

        let session_id = self.workspace().read(cx).session_id();
        let window_id_u64 = window.window_handle().window_id().as_u64();

        let mut tasks: Vec<Task<()>> = Vec::new();
        for workspace in self.workspaces() {
            tasks.push(workspace.update(cx, |ws, cx| ws.flush_serialization(window, cx)));
            if let Some(db_id) = workspace.read(cx).database_id() {
                let db = crate::persistence::WorkspaceDb::global(cx);
                let session_id = session_id.clone();
                tasks.push(cx.background_spawn(async move {
                    db.set_session_binding(db_id, session_id, Some(window_id_u64))
                        .await
                        .log_err();
                }));
            }
        }
        self.serialize(cx);
        tasks
    }

    /// The best replacement candidate within one project group: its most
    /// recently displayed member, else its first pinned member, skipping
    /// `excluding` and disconnected projects.
    fn live_member_for_group(
        &self,
        key: &ProjectGroupKey,
        excluding: &[Entity<Workspace>],
        cx: &App,
    ) -> Option<Entity<Workspace>> {
        let available = |workspace: &Entity<Workspace>| {
            !excluding.contains(workspace)
                && !workspace.read(cx).project().read(cx).is_disconnected(cx)
        };
        self.last_active_workspace_for_group(key, cx)
            .filter(&available)
            .or_else(|| {
                self.held
                    .iter()
                    .filter(|held| held.pinned)
                    .map(|held| held.workspace.clone())
                    .filter(|workspace| workspace.read(cx).project_group_key(cx) == *key)
                    .find(available)
            })
    }

    /// Removes one or more workspaces from this multi-workspace.
    ///
    /// Every workspace is first asked for consent (save prompts); no state
    /// changes until all consent. The rows are then deleted and, when the
    /// displayed workspace was among them, a replacement is chosen from what
    /// remains: another workspace in the same project, then a workspace in the
    /// nearest neighboring project, then an empty workspace. When the intent is
    /// `KeepProject` and the project has no other workspace, its root worktrees
    /// are reopened afterwards; the same applies to the adjacent local project
    /// when nothing at all remains.
    ///
    /// Returns `true` if any workspaces were actually removed.
    pub fn close_workspace(
        &mut self,
        workspace: &Entity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool>> {
        self.remove([workspace.clone()], RemovalIntent::CloseProject, window, cx)
    }

    /// Returns `true` if any workspaces were actually removed.
    pub fn remove(
        &mut self,
        workspaces: impl IntoIterator<Item = Entity<Workspace>>,
        intent: RemovalIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool>> {
        let workspaces: Vec<_> = workspaces.into_iter().collect();

        if workspaces.is_empty() {
            return Task::ready(Ok(false));
        }

        let original_active = self.workspace().clone();
        let group_key = original_active.read(cx).project_group_key(cx);

        // Record the neighborhood of the project as the user sees it now:
        // callers like `remove_project_group` delete the project row itself
        // before the removal task runs.
        let group_index = self
            .project_groups
            .iter()
            .position(|group| group.key == group_key);
        let neighbor_keys = self.neighbor_group_keys(group_index);
        let adjacent_key = group_index.and_then(|index| {
            self.project_groups
                .get(index + 1)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|previous| self.project_groups.get(previous))
                })
                .map(|group| group.key.clone())
        });

        cx.spawn_in(window, async move |this, cx| {
            // Consent phase: run the standard close lifecycle for every
            // workspace being removed. Prompts only; no state changes.
            for workspace in &workspaces {
                let should_continue = workspace
                    .update_in(cx, |workspace, window, cx| {
                        workspace.prepare_to_close(CloseIntent::ReplaceWindow, window, cx)
                    })?
                    .await?;

                if !should_continue {
                    return Ok(false);
                }
            }

            // Edit phase: one synchronous update. Delete the rows, then pick
            // the replacement from the rows that actually remain.
            let (removed_any, reopen_key) = this.update_in(cx, |this, window, cx| {
                let mut removed_any = false;
                let displayed_workspace = this.workspace().clone();

                for workspace in &workspaces {
                    if *workspace == displayed_workspace {
                        continue;
                    }
                    if this.held_index(workspace).is_some() {
                        this.detach_workspace(workspace, cx);
                        removed_any = true;
                    }
                }

                let mut reopen_key = None;
                if workspaces.contains(&displayed_workspace) {
                    let doomed = std::slice::from_ref(&displayed_workspace);

                    let same_group = this
                        .held
                        .iter()
                        .filter(|held| held.pinned && held.workspace != displayed_workspace)
                        .map(|held| held.workspace.clone())
                        .find(|workspace| workspace.read(cx).project_group_key(cx) == group_key);
                    if intent == RemovalIntent::KeepProject
                        && same_group.is_none()
                        && group_key.host().is_none()
                        && !group_key.path_list().is_empty()
                    {
                        reopen_key = Some(group_key.clone());
                    }

                    let replacement = same_group
                        .or_else(|| {
                            neighbor_keys
                                .iter()
                                .find_map(|key| this.live_member_for_group(key, doomed, cx))
                        })
                        .unwrap_or_else(|| {
                            if reopen_key.is_none() {
                                reopen_key = adjacent_key.clone().filter(|key| {
                                    key.host().is_none() && !key.path_list().is_empty()
                                });
                            }
                            let app_state = displayed_workspace.read(cx).app_state().clone();
                            let project = Project::local(
                                app_state.client.clone(),
                                app_state.node_runtime.clone(),
                                app_state.user_store.clone(),
                                app_state.languages.clone(),
                                app_state.fs.clone(),
                                None,
                                project::LocalProjectFlags::default(),
                                cx,
                            );
                            cx.new(|cx| Workspace::new(None, project, app_state, window, cx))
                        });

                    this.activate(replacement, None, window, cx);
                    this.detach_workspace(&displayed_workspace, cx);
                    removed_any = true;
                } else if *this.workspace() != original_active
                    && !workspaces.contains(&original_active)
                {
                    // Prompting switched the display away from where the user
                    // was; go back.
                    this.activate(original_active.clone(), None, window, cx);
                }

                if removed_any {
                    this.serialize(cx);
                    cx.notify();
                }

                (removed_any, reopen_key)
            })?;

            if let Some(key) = reopen_key {
                this.update_in(cx, |this, window, cx| {
                    this.find_or_create_local_workspace(
                        key.path_list().clone(),
                        Some(key),
                        None,
                        OpenMode::Activate,
                        None,
                        window,
                        cx,
                    )
                })?
                .await?;
            }

            Ok(removed_any)
        })
    }

    pub fn open_project(
        &mut self,
        paths: Vec<PathBuf>,
        open_mode: OpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Workspace>>> {
        if self.retention_enabled(cx) {
            let empty_workspace = if self
                .workspace()
                .read(cx)
                .project()
                .read(cx)
                .visible_worktrees(cx)
                .next()
                .is_none()
            {
                Some(self.workspace().clone())
            } else {
                None
            };

            cx.spawn_in(window, async move |this, cx| {
                if let Some(empty_workspace) = empty_workspace.as_ref() {
                    let should_continue = empty_workspace
                        .update_in(cx, |workspace, window, cx| {
                            workspace.prepare_to_close(CloseIntent::ReplaceWindow, window, cx)
                        })?
                        .await?;
                    if !should_continue {
                        return Ok(empty_workspace.clone());
                    }
                }

                let create_task = this.update_in(cx, |this, window, cx| {
                    this.find_or_create_local_workspace(
                        PathList::new(&paths),
                        None,
                        None,
                        OpenMode::Activate,
                        None,
                        window,
                        cx,
                    )
                })?;
                let new_workspace = create_task.await?;

                if let Some(empty_workspace) = empty_workspace
                    && empty_workspace != new_workspace
                {
                    this.update(cx, |this, cx| {
                        if this.is_workspace_retained(&empty_workspace) {
                            this.detach_workspace(&empty_workspace, cx);
                        }
                    })?;
                }

                Ok(new_workspace)
            })
        } else {
            let workspace = self.workspace().clone();
            cx.spawn_in(window, async move |_this, cx| {
                let should_continue = workspace
                    .update_in(cx, |workspace, window, cx| {
                        workspace.prepare_to_close(crate::CloseIntent::ReplaceWindow, window, cx)
                    })?
                    .await?;
                if should_continue {
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            workspace.open_workspace_for_paths(open_mode, paths, window, cx)
                        })?
                        .await
                } else {
                    Ok(workspace)
                }
            })
        }
    }
}

impl Render for MultiWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar_ui_enabled = self.sidebar_ui_enabled(cx);
        let sidebar_side = self.sidebar_side(cx);
        let sidebar_on_right = sidebar_side == SidebarSide::Right;

        let sidebar: Option<AnyElement> = if sidebar_ui_enabled && self.sidebar_open() {
            self.sidebar.as_ref().map(|sidebar_handle| {
                let weak = cx.weak_entity();

                let sidebar_width = sidebar_handle.width(cx);
                let resize_handle = deferred(
                    div()
                        .id("sidebar-resize-handle")
                        .absolute()
                        .when(!sidebar_on_right, |el| {
                            el.right(-SIDEBAR_RESIZE_HANDLE_SIZE / 2.)
                        })
                        .when(sidebar_on_right, |el| {
                            el.left(-SIDEBAR_RESIZE_HANDLE_SIZE / 2.)
                        })
                        .top(px(0.))
                        .h_full()
                        .w(SIDEBAR_RESIZE_HANDLE_SIZE)
                        .cursor_col_resize()
                        .on_drag(DraggedSidebar, |dragged, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| dragged.clone())
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_mouse_up(MouseButton::Left, move |event, _, cx| {
                            if event.click_count == 2 {
                                weak.update(cx, |this, cx| {
                                    if let Some(sidebar) = this.sidebar.as_mut() {
                                        sidebar.set_width(None, cx);
                                    }
                                    this.serialize(cx);
                                })
                                .ok();
                                cx.stop_propagation();
                            } else {
                                weak.update(cx, |this, cx| {
                                    this.serialize(cx);
                                })
                                .ok();
                            }
                        })
                        .occlude(),
                );

                div()
                    .id("sidebar-container")
                    .relative()
                    .h_full()
                    .w(sidebar_width)
                    .flex_shrink_0()
                    .child(sidebar_handle.to_any())
                    .child(resize_handle)
                    .into_any_element()
            })
        } else {
            None
        };

        let (left_sidebar, right_sidebar) = if sidebar_on_right {
            (None, sidebar)
        } else {
            (sidebar, None)
        };

        let ui_font = theme_settings::setup_ui_font(window, cx);
        let text_color = cx.theme().colors().text;
        let workspace_tabs = self.render_workspace_tabs(window, cx);

        let workspace = self.workspace().clone();
        let workspace_key_context = workspace.update(cx, |workspace, cx| workspace.key_context(cx));
        let root = workspace.update(cx, |workspace, cx| workspace.actions(h_flex(), window, cx));

        client_side_decorations(
            root.key_context(workspace_key_context)
                .relative()
                .size_full()
                .font(ui_font)
                .text_color(text_color)
                .on_action(cx.listener(Self::close_window))
                // `#zed-37`: these cycle the workspace tab strip, not the AI sidebar's
                // project list, and they sit outside the `sidebar_ui_enabled` gate below
                // on purpose — tab cycling has to keep working with the agent disabled,
                // which is the case this feature exists to serve. The thread actions
                // inside the gate stay upstream's.
                .on_action(cx.listener(|this: &mut Self, _: &NextProject, window, cx| {
                    if !this.configuration_switch_in_progress {
                        this.cycle_workspace_tab(true, window, cx);
                    }
                }))
                .on_action(
                    cx.listener(|this: &mut Self, _: &PreviousProject, window, cx| {
                        if !this.configuration_switch_in_progress {
                            this.cycle_workspace_tab(false, window, cx);
                        }
                    }),
                )
                .on_action(cx.listener(
                    |this: &mut Self, _: &SaveWorkspaceConfigurationAs, window, cx| {
                        if !this.configuration_switch_in_progress {
                            this.show_save_workspace_configuration_as(window, cx);
                        }
                    },
                ))
                .on_action(cx.listener(
                    |this: &mut Self, _: &ManageWorkspaceConfigurations, window, cx| {
                        if !this.configuration_switch_in_progress {
                            this.show_manage_workspace_configurations(window, cx);
                        }
                    },
                ))
                .on_action(cx.listener(
                    |this: &mut Self, _: &SwitchWorkspaceConfiguration, window, cx| {
                        if !this.configuration_switch_in_progress {
                            let menu_handle = this.workspace_configuration_menu_handle.clone();
                            window.defer(cx, move |window, cx| menu_handle.show(window, cx));
                        }
                    },
                ))
                .when(self.sidebar_ui_enabled(cx), |this| {
                    this.on_action(cx.listener(
                        |this: &mut Self, _: &ToggleWorkspaceSidebar, window, cx| {
                            this.toggle_sidebar(window, cx);
                        },
                    ))
                    .on_action(cx.listener(
                        |this: &mut Self, _: &CloseWorkspaceSidebar, window, cx| {
                            this.close_sidebar_action(window, cx);
                        },
                    ))
                    .on_action(cx.listener(
                        |this: &mut Self, _: &FocusWorkspaceSidebar, window, cx| {
                            this.focus_sidebar(window, cx);
                        },
                    ))
                    .on_action(cx.listener(
                        |this: &mut Self, action: &ToggleThreadSwitcher, window, cx| {
                            if let Some(sidebar) = &this.sidebar {
                                sidebar.toggle_thread_switcher(action.select_last, window, cx);
                            }
                        },
                    ))
                    .on_action(cx.listener(|this: &mut Self, _: &NextThread, window, cx| {
                        if let Some(sidebar) = &this.sidebar {
                            sidebar.cycle_thread(true, window, cx);
                        }
                    }))
                    .on_action(
                        cx.listener(|this: &mut Self, _: &PreviousThread, window, cx| {
                            if let Some(sidebar) = &this.sidebar {
                                sidebar.cycle_thread(false, window, cx);
                            }
                        }),
                    )
                    .when(self.project_group_keys().len() >= 2, |el| {
                        el.on_action(cx.listener(
                            |this: &mut Self, _: &MoveProjectToNewWindow, window, cx| {
                                let key =
                                    this.project_group_key_for_workspace(this.workspace(), cx);
                                this.open_project_group_in_new_window(&key, window, cx)
                                    .detach_and_log_err(cx);
                            },
                        ))
                    })
                })
                .when(self.sidebar_open() && self.sidebar_ui_enabled(cx), |this| {
                    this.on_drag_move(cx.listener(
                        move |this: &mut Self, e: &DragMoveEvent<DraggedSidebar>, window, cx| {
                            if let Some(sidebar) = &this.sidebar {
                                let new_width = if sidebar_on_right {
                                    window.bounds().size.width - e.event.position.x
                                } else {
                                    e.event.position.x
                                };
                                sidebar.set_width(Some(new_width), cx);
                            }
                        },
                    ))
                })
                // `#zed-37`: the tab strip sits above the sidebars and the workspace,
                // so it spans the window and can extend the titlebar background.
                .children(workspace_tabs)
                .children(left_sidebar)
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .size_full()
                        .overflow_hidden()
                        .child(self.workspace().clone()),
                )
                .children(right_sidebar)
                .child(self.workspace().read(cx).modal_layer.clone())
                .children(
                    self.configuration_switch_gate
                        .as_ref()
                        .map(|configuration_name| {
                            let configuration_name = configuration_name.clone();
                            let focus_handle = self.configuration_switch_gate_focus_handle.clone();
                            deferred(
                                div()
                                    .debug_selector(|| {
                                        "SWITCHING-WORKSPACE-CONFIGURATION-GATE".to_string()
                                    })
                                    .absolute()
                                    .size_full()
                                    .inset_0()
                                    .occlude()
                                    .track_focus(&focus_handle)
                                    .bg(cx.theme().colors().panel_background.opacity(0.8))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .child(
                                        v_flex()
                                            .p_4()
                                            .gap_1()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(cx.theme().colors().border)
                                            .bg(cx.theme().colors().elevated_surface_background)
                                            .child(Label::new("Switching Workspace Configuration…"))
                                            .child(
                                                Label::new(configuration_name)
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted),
                                            ),
                                    ),
                            )
                            .with_priority(3)
                        }),
                )
                .children(self.sidebar_overlay.as_ref().map(|view| {
                    deferred(div().absolute().size_full().inset_0().occlude().child(
                        v_flex().h(px(0.0)).top_20().items_center().child(
                            h_flex().occlude().child(view.clone()).on_mouse_down(
                                MouseButton::Left,
                                |_, _, cx| {
                                    cx.stop_propagation();
                                },
                            ),
                        ),
                    ))
                    .with_priority(2)
                })),
            window,
            cx,
        )
    }
}
