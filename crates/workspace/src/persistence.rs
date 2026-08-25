pub mod model;

use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

#[cfg(test)]
use std::sync::LazyLock;

use chrono::{DateTime, NaiveDateTime, Utc};
use fs::Fs;

use anyhow::{Context as _, Result, bail};
use collections::{HashMap, HashSet, IndexSet};
use db::{
    kvp::KeyValueStore,
    query,
    sqlez::{connection::Connection, domain::Domain},
    sqlez_macros::sql,
};
use gpui::{
    AppContext as _, AsyncApp, Axis, BorrowAppContext as _, Bounds, Task, WindowBounds, WindowId,
    point, size,
};
use project::{
    ProjectGroupKey,
    bookmark_store::SerializedBookmark,
    debugger::breakpoint_store::{BreakpointState, SourceBreakpoint},
    trusted_worktrees::{DbTrustedPaths, RemoteHostLocation},
};

use language::{LanguageName, Toolchain, ToolchainScope};
use remote::{
    DockerConnectionOptions, RemoteConnectionIdentity, RemoteConnectionOptions,
    SshConnectionOptions, WslConnectionOptions, remote_connection_identity,
};
use serde::{Deserialize, Serialize};
use sqlez::{
    bindable::{Bind, Column, StaticColumnCount},
    statement::Statement,
    thread_safe_connection::ThreadSafeConnection,
};

use ui::{App, SharedString, px};
use util::{ResultExt, maybe, rel_path::RelPath};
use uuid::Uuid;

use crate::{
    WorkspaceId,
    path_list::{PathList, SerializedPathList},
    persistence::model::RemoteConnectionKind,
};

use model::{
    GroupId, ItemId, PaneId, RemoteConnectionId, SerializedItem, SerializedPane,
    SerializedPaneGroup, SerializedWorkspace,
};

use self::model::{DockStructure, SerializedWorkspaceLocation, SessionWorkspace};

// https://www.sqlite.org/limits.html
// > <..> the maximum value of a host parameter number is SQLITE_MAX_VARIABLE_NUMBER,
// > which defaults to <..> 32766 for SQLite versions after 3.32.0.
const MAX_QUERY_PLACEHOLDERS: usize = 32000;

#[cfg(test)]
static FAIL_NEXT_WORKSPACE_WRITE: LazyLock<parking_lot::Mutex<HashSet<WorkspaceId>>> =
    LazyLock::new(Default::default);

fn parse_timestamp(text: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
        .map(|naive| naive.and_utc())
        .unwrap_or_else(|_| Utc::now())
}

fn contains_wsl_path(paths: &PathList) -> bool {
    cfg!(windows)
        && paths
            .paths()
            .iter()
            .any(|path| util::paths::WslPath::from_path(path).is_some())
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SerializedAxis(pub(crate) gpui::Axis);
impl sqlez::bindable::StaticColumnCount for SerializedAxis {}
impl sqlez::bindable::Bind for SerializedAxis {
    fn bind(
        &self,
        statement: &sqlez::statement::Statement,
        start_index: i32,
    ) -> anyhow::Result<i32> {
        match self.0 {
            gpui::Axis::Horizontal => "Horizontal",
            gpui::Axis::Vertical => "Vertical",
        }
        .bind(statement, start_index)
    }
}

impl sqlez::bindable::Column for SerializedAxis {
    fn column(
        statement: &mut sqlez::statement::Statement,
        start_index: i32,
    ) -> anyhow::Result<(Self, i32)> {
        String::column(statement, start_index).and_then(|(axis_text, next_index)| {
            Ok((
                match axis_text.as_str() {
                    "Horizontal" => Self(Axis::Horizontal),
                    "Vertical" => Self(Axis::Vertical),
                    _ => anyhow::bail!("Stored serialized item kind is incorrect"),
                },
                next_index,
            ))
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub(crate) struct SerializedWindowBounds(pub(crate) WindowBounds);

impl StaticColumnCount for SerializedWindowBounds {
    fn column_count() -> usize {
        5
    }
}

impl Bind for SerializedWindowBounds {
    fn bind(&self, statement: &Statement, start_index: i32) -> Result<i32> {
        match self.0 {
            WindowBounds::Windowed(bounds) => {
                let next_index = statement.bind(&"Windowed", start_index)?;
                statement.bind(
                    &(
                        SerializedPixels(bounds.origin.x),
                        SerializedPixels(bounds.origin.y),
                        SerializedPixels(bounds.size.width),
                        SerializedPixels(bounds.size.height),
                    ),
                    next_index,
                )
            }
            WindowBounds::Maximized(bounds) => {
                let next_index = statement.bind(&"Maximized", start_index)?;
                statement.bind(
                    &(
                        SerializedPixels(bounds.origin.x),
                        SerializedPixels(bounds.origin.y),
                        SerializedPixels(bounds.size.width),
                        SerializedPixels(bounds.size.height),
                    ),
                    next_index,
                )
            }
            WindowBounds::Fullscreen(bounds) => {
                let next_index = statement.bind(&"FullScreen", start_index)?;
                statement.bind(
                    &(
                        SerializedPixels(bounds.origin.x),
                        SerializedPixels(bounds.origin.y),
                        SerializedPixels(bounds.size.width),
                        SerializedPixels(bounds.size.height),
                    ),
                    next_index,
                )
            }
        }
    }
}

impl Column for SerializedWindowBounds {
    fn column(statement: &mut Statement, start_index: i32) -> Result<(Self, i32)> {
        let (window_state, next_index) = String::column(statement, start_index)?;
        let ((x, y, width, height), _): ((i32, i32, i32, i32), _) =
            Column::column(statement, next_index)?;
        let bounds = Bounds {
            origin: point(px(x as f32), px(y as f32)),
            size: size(px(width as f32), px(height as f32)),
        };

        let status = match window_state.as_str() {
            "Windowed" | "Fixed" => SerializedWindowBounds(WindowBounds::Windowed(bounds)),
            "Maximized" => SerializedWindowBounds(WindowBounds::Maximized(bounds)),
            "FullScreen" => SerializedWindowBounds(WindowBounds::Fullscreen(bounds)),
            _ => bail!("Window State did not have a valid string"),
        };

        Ok((status, next_index + 4))
    }
}

const DEFAULT_WINDOW_BOUNDS_KEY: &str = "default_window_bounds";

pub fn read_default_window_bounds(kvp: &KeyValueStore) -> Option<(Uuid, WindowBounds)> {
    let json_str = kvp
        .read_kvp(DEFAULT_WINDOW_BOUNDS_KEY)
        .log_err()
        .flatten()?;

    let (display_uuid, persisted) =
        serde_json::from_str::<(Uuid, WindowBoundsJson)>(&json_str).ok()?;
    Some((display_uuid, persisted.into()))
}

pub async fn write_default_window_bounds(
    kvp: &KeyValueStore,
    bounds: WindowBounds,
    display_uuid: Uuid,
) -> anyhow::Result<()> {
    let persisted = WindowBoundsJson::from(bounds);
    let json_str = serde_json::to_string(&(display_uuid, persisted))?;
    kvp.write_kvp(DEFAULT_WINDOW_BOUNDS_KEY.to_string(), json_str)
        .await?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub enum WindowBoundsJson {
    Windowed {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Maximized {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Fullscreen {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

impl From<WindowBounds> for WindowBoundsJson {
    fn from(b: WindowBounds) -> Self {
        match b {
            WindowBounds::Windowed(bounds) => {
                let origin = bounds.origin;
                let size = bounds.size;
                WindowBoundsJson::Windowed {
                    x: f32::from(origin.x).round() as i32,
                    y: f32::from(origin.y).round() as i32,
                    width: f32::from(size.width).round() as i32,
                    height: f32::from(size.height).round() as i32,
                }
            }
            WindowBounds::Maximized(bounds) => {
                let origin = bounds.origin;
                let size = bounds.size;
                WindowBoundsJson::Maximized {
                    x: f32::from(origin.x).round() as i32,
                    y: f32::from(origin.y).round() as i32,
                    width: f32::from(size.width).round() as i32,
                    height: f32::from(size.height).round() as i32,
                }
            }
            WindowBounds::Fullscreen(bounds) => {
                let origin = bounds.origin;
                let size = bounds.size;
                WindowBoundsJson::Fullscreen {
                    x: f32::from(origin.x).round() as i32,
                    y: f32::from(origin.y).round() as i32,
                    width: f32::from(size.width).round() as i32,
                    height: f32::from(size.height).round() as i32,
                }
            }
        }
    }
}

impl From<WindowBoundsJson> for WindowBounds {
    fn from(n: WindowBoundsJson) -> Self {
        match n {
            WindowBoundsJson::Windowed {
                x,
                y,
                width,
                height,
            } => WindowBounds::Windowed(Bounds {
                origin: point(px(x as f32), px(y as f32)),
                size: size(px(width as f32), px(height as f32)),
            }),
            WindowBoundsJson::Maximized {
                x,
                y,
                width,
                height,
            } => WindowBounds::Maximized(Bounds {
                origin: point(px(x as f32), px(y as f32)),
                size: size(px(width as f32), px(height as f32)),
            }),
            WindowBoundsJson::Fullscreen {
                x,
                y,
                width,
                height,
            } => WindowBounds::Fullscreen(Bounds {
                origin: point(px(x as f32), px(y as f32)),
                size: size(px(width as f32), px(height as f32)),
            }),
        }
    }
}

fn read_multi_workspace_state(window_id: WindowId, cx: &App) -> model::MultiWorkspaceState {
    let kvp = KeyValueStore::global(cx);
    kvp.scoped("multi_workspace_state")
        .read(&window_id.as_u64().to_string())
        .log_err()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub async fn write_multi_workspace_state(
    kvp: &KeyValueStore,
    window_id: WindowId,
    state: model::MultiWorkspaceState,
) {
    if let Ok(json_str) = serde_json::to_string(&state) {
        kvp.scoped("multi_workspace_state")
            .write(window_id.as_u64().to_string(), json_str)
            .await
            .log_err();
    }
}

/// The scoped-KVP namespace and key holding every saved workspace configuration.
///
/// One key, one value: a whole-collection replace is therefore atomic, so no write can
/// leave two configurations disagreeing with each other.
const WORKSPACE_CONFIGURATIONS_NAMESPACE: &str = "workspace_configurations";
const WORKSPACE_CONFIGURATIONS_KEY: &str = "collection";

/// What a read found in the configuration value.
///
/// The store distinguishes these because they call for different handling: an absent
/// value can be replaced, while a value that could not be read or was written by a newer
/// build must not be silently overwritten by this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredConfigurations {
    /// A value this build wrote and understands, or no value at all.
    Readable(model::WorkspaceConfigurationCollection),
    /// A value whose `schema_version` is newer than this build writes.
    TooNew { schema_version: u32 },
    /// A value present but not parseable as a configuration collection.
    Corrupt,
    /// The scoped KVP read itself failed, so absence cannot be established safely.
    ReadFailed,
}

fn stored_configurations_from_read_result(
    read_result: Result<Option<String>>,
) -> StoredConfigurations {
    let json = match read_result {
        Ok(Some(json)) => json,
        Ok(None) => return StoredConfigurations::Readable(Default::default()),
        Err(error) => {
            log::error!("failed to read workspace configurations: {error:#}");
            return StoredConfigurations::ReadFailed;
        }
    };

    // Read the version before the body. A newer build is the case most likely to have
    // changed the shape of everything around it, so parsing the whole value first would
    // report its data as corrupt rather than as merely newer — and send the user chasing
    // a log instead of an update.
    #[derive(Deserialize)]
    struct SchemaVersionProbe {
        schema_version: u32,
    }

    if let Ok(SchemaVersionProbe { schema_version }) =
        serde_json::from_str::<SchemaVersionProbe>(&json)
        && schema_version > model::WORKSPACE_CONFIGURATION_SCHEMA_VERSION
    {
        return StoredConfigurations::TooNew { schema_version };
    }

    match serde_json::from_str::<model::WorkspaceConfigurationCollection>(&json) {
        Ok(collection) => StoredConfigurations::Readable(collection),
        Err(error) => {
            log::error!("workspace configurations are unreadable: {error}");
            StoredConfigurations::Corrupt
        }
    }
}

fn read_workspace_configurations(cx: &App) -> StoredConfigurations {
    let kvp = KeyValueStore::global(cx);
    stored_configurations_from_read_result(
        kvp.scoped(WORKSPACE_CONFIGURATIONS_NAMESPACE)
            .read(WORKSPACE_CONFIGURATIONS_KEY),
    )
}

/// Why the store will not accept mutations.
///
/// Both variants mean the same thing to a caller — no configuration can be written — but
/// they carry different copy, because only one of them is the user's fault to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreBlock {
    /// A newer build wrote the stored value. Refusing every mutation is what keeps that
    /// build's configurations intact across a rollback, which is the whole point of this
    /// feature; overwriting them would be the exact loss it exists to prevent.
    WrittenByNewerBuild { schema_version: u32 },
    /// The stored value is present but unparseable, so applying a mutation to it would
    /// mean guessing at what it used to hold.
    Corrupt,
    /// The stored value could not be read, so treating it as absent could erase it on the
    /// next successful write.
    ReadFailed,
}

impl StoreBlock {
    pub fn user_facing_message(&self) -> String {
        match self {
            StoreBlock::WrittenByNewerBuild { .. } => {
                "These workspace configurations were saved by a newer version of Zed RBF. \
                 Update Zed RBF to use them again. They have not been changed."
                    .to_string()
            }
            StoreBlock::Corrupt => "Saved workspace configurations couldn't be read. \
                 They have not been changed. See the Zed log for details."
                .to_string(),
            StoreBlock::ReadFailed => "Saved workspace configurations couldn't be loaded. \
                 They have not been changed. Retry after restarting Zed RBF."
                .to_string(),
        }
    }
}

/// A typed change to one configuration.
///
/// Callers submit these rather than a whole replacement collection. That is what makes
/// two windows checkpointing different configurations safe: each mutation is applied to
/// the store's latest committed value at the moment it runs, so neither can carry a stale
/// copy of the other's configuration back over it.
#[derive(Debug, Clone)]
pub enum WorkspaceConfigurationMutation {
    Create {
        name: String,
        members: Vec<model::WorkspaceConfigurationMember>,
        active_member: Option<WorkspaceId>,
    },
    Checkpoint {
        id: model::WorkspaceConfigurationId,
        members: Vec<model::WorkspaceConfigurationMember>,
        active_member: Option<WorkspaceId>,
    },
}

/// What a successful mutation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreCommit {
    pub id: model::WorkspaceConfigurationId,
    /// The store generation after this commit. A switch captures the generation before it
    /// stages anything and rechecks it at commit time, so a configuration edited during a
    /// long stage aborts rather than committing against what it read.
    pub generation: u64,
}

/// The one owner of every saved workspace configuration in this process.
///
/// All Zed windows share one `App`, so a single global here genuinely serializes every
/// writer: there is no second process to race with.
pub struct WorkspaceConfigurationStore {
    /// The latest value this process committed, mirrored in memory so the menu can render
    /// names without touching the database.
    committed: model::WorkspaceConfigurationCollection,
    generation: u64,
    /// Set when the stored value must not be overwritten. Every mutation is refused while
    /// it holds a value.
    blocked: Option<StoreBlock>,
    workspace_row_reservations: HashMap<Uuid, HashSet<WorkspaceId>>,
    shutdown_committed: Arc<parking_lot::Mutex<model::WorkspaceConfigurationCollection>>,
    shutdown_queue: Arc<futures::lock::Mutex<()>>,
    /// Tail of the mutation queue.
    ///
    /// Each new mutation moves this into its own future and awaits it, rather than
    /// replacing it — dropping a `Task` in GPUI cancels it, so replacing the tail would
    /// silently discard a checkpoint the moment a second one arrived.
    queue_tail: Option<Task<()>>,
    #[cfg(test)]
    fail_writes_for_tests: bool,
}

impl gpui::Global for WorkspaceConfigurationStore {}

impl WorkspaceConfigurationStore {
    pub fn init(cx: &mut App) {
        if cx.try_global::<Self>().is_some() {
            return;
        }
        cx.set_global(Self::load(cx));
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn try_global(cx: &App) -> Option<&Self> {
        cx.try_global::<Self>()
    }

    fn load(cx: &App) -> Self {
        let (committed, blocked) = Self::load_state(cx);
        Self::new(committed, blocked)
    }

    fn load_state(cx: &App) -> (model::WorkspaceConfigurationCollection, Option<StoreBlock>) {
        let (committed, blocked) = match read_workspace_configurations(cx) {
            StoredConfigurations::Readable(collection) => (collection, None),
            StoredConfigurations::TooNew { schema_version } => (
                Default::default(),
                Some(StoreBlock::WrittenByNewerBuild { schema_version }),
            ),
            StoredConfigurations::Corrupt => (Default::default(), Some(StoreBlock::Corrupt)),
            StoredConfigurations::ReadFailed => (Default::default(), Some(StoreBlock::ReadFailed)),
        };

        (committed, blocked)
    }

    fn new(
        committed: model::WorkspaceConfigurationCollection,
        blocked: Option<StoreBlock>,
    ) -> Self {
        let shutdown_committed = Arc::new(parking_lot::Mutex::new(committed.clone()));
        Self {
            committed,
            generation: 0,
            blocked,
            workspace_row_reservations: HashMap::default(),
            shutdown_committed,
            shutdown_queue: Arc::new(futures::lock::Mutex::new(())),
            queue_tail: None,
            #[cfg(test)]
            fail_writes_for_tests: false,
        }
    }

    /// Every saved configuration, from memory. Never touches the filesystem.
    pub fn configurations(&self) -> &[model::WorkspaceConfiguration] {
        &self.committed.configurations
    }

    pub fn configuration(
        &self,
        id: model::WorkspaceConfigurationId,
    ) -> Option<&model::WorkspaceConfiguration> {
        self.committed.find(id)
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Why mutations are refused, when they are.
    pub fn blocked(&self) -> Option<&StoreBlock> {
        self.blocked.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_write_failure_for_tests(fail: bool, cx: &mut App) {
        cx.global_mut::<Self>().fail_writes_for_tests = fail;
    }

    #[cfg(test)]
    pub(crate) fn reload_for_tests(cx: &mut App) {
        cx.set_global(Self::load(cx));
    }

    /// Every workspace row a configuration still refers to.
    ///
    /// Recent cleanup consults this so it cannot delete the exact state a saved
    /// configuration depends on, leaving a name that restores only folders.
    pub fn referenced_workspace_ids(&self) -> Option<HashSet<WorkspaceId>> {
        self.blocked.is_none().then(|| {
            let mut referenced = self.committed.referenced_workspace_ids();
            referenced.extend(self.workspace_row_reservations.values().flatten().copied());
            referenced
        })
    }

    pub(crate) fn reserve_workspace_rows(
        workspace_ids: impl IntoIterator<Item = WorkspaceId>,
        cx: &mut App,
    ) -> Uuid {
        let this = cx.global_mut::<Self>();
        let reservation_id = Uuid::new_v4();
        this.workspace_row_reservations
            .insert(reservation_id, workspace_ids.into_iter().collect());
        reservation_id
    }

    pub(crate) fn release_workspace_rows(reservation_id: Uuid, cx: &mut App) {
        cx.global_mut::<Self>()
            .workspace_row_reservations
            .remove(&reservation_id);
    }

    pub fn delete_unreferenced_workspace_rows_global(
        db: WorkspaceDb,
        workspace_ids: Vec<WorkspaceId>,
        cx: &mut App,
    ) -> Task<Result<Vec<WorkspaceId>>> {
        let (send_result, receive_result) = futures::channel::oneshot::channel();
        let previous = cx.global_mut::<Self>().queue_tail.take();

        let queued = cx.spawn(async move |cx| {
            if let Some(previous) = previous {
                previous.await;
            }

            let result = async {
                let workspace_ids = cx.update(|cx| {
                    let protected_workspace_ids = Self::global(cx)
                        .referenced_workspace_ids()
                        .context("saved workspace configurations are unavailable")?;
                    Ok::<_, anyhow::Error>(
                        workspace_ids
                            .into_iter()
                            .filter(|id| !protected_workspace_ids.contains(id))
                            .collect::<Vec<_>>(),
                    )
                })?;

                for workspace_id in &workspace_ids {
                    db.delete_workspace_by_id(*workspace_id).await?;
                }
                Ok(workspace_ids)
            }
            .await;

            if send_result.send(result).is_err() {
                log::debug!("workspace row deletion caller was dropped");
            }
        });

        cx.global_mut::<Self>().queue_tail = Some(queued);
        cx.background_spawn(async move {
            receive_result
                .await
                .context("workspace configuration store dropped the row deletion")?
        })
    }

    /// Queue one typed change.
    ///
    /// The returned task resolves after this mutation has been durably written, and every
    /// mutation queued before it has finished. A caller that drops the returned task does
    /// not cancel the write — the work lives on the store's own queue.
    pub fn mutate_global(
        mutation: WorkspaceConfigurationMutation,
        cx: &mut App,
    ) -> Task<Result<StoreCommit>> {
        Self::mutate_after_global(Task::ready(Ok(())), None, mutation, cx)
    }

    pub(crate) fn mutate_conditionally_global(
        validation: impl FnOnce(&mut App) -> Result<()> + 'static,
        mutation: WorkspaceConfigurationMutation,
        cx: &mut App,
    ) -> Task<Result<StoreCommit>> {
        Self::mutate_after_global(
            Task::ready(Ok(())),
            Some(Box::new(validation)),
            mutation,
            cx,
        )
    }

    fn mutate_after_global(
        prerequisite: Task<Result<()>>,
        validation: Option<Box<dyn FnOnce(&mut App) -> Result<()>>>,
        mutation: WorkspaceConfigurationMutation,
        cx: &mut App,
    ) -> Task<Result<StoreCommit>> {
        let (send_result, receive_result) = futures::channel::oneshot::channel();
        let previous = cx.global_mut::<Self>().queue_tail.take();

        let queued = cx.spawn(async move |cx| {
            if let Some(previous) = previous {
                previous.await;
            }

            let result = async {
                prerequisite.await?;
                if let Some(validation) = validation {
                    cx.update(validation)?;
                }
                Self::apply(mutation, cx).await
            }
            .await;
            if send_result.send(result).is_err() {
                log::debug!("workspace configuration mutation caller was dropped");
            }
        });

        cx.global_mut::<Self>().queue_tail = Some(queued);

        cx.background_spawn(async move {
            receive_result
                .await
                .context("workspace configuration store dropped the mutation")?
        })
    }

    pub(crate) fn checkpoint_after_for_shutdown(
        prerequisite: Task<Result<()>>,
        id: model::WorkspaceConfigurationId,
        members: Vec<model::WorkspaceConfigurationMember>,
        active_member: Option<WorkspaceId>,
        cx: &mut App,
    ) -> futures::future::LocalBoxFuture<'static, Result<StoreCommit>> {
        let kvp = KeyValueStore::global(cx);
        let (previous, shutdown_committed, shutdown_queue, blocked, generation) = {
            let this = cx.global_mut::<Self>();
            (
                this.queue_tail.take(),
                this.shutdown_committed.clone(),
                this.shutdown_queue.clone(),
                this.blocked.clone(),
                this.generation,
            )
        };
        Box::pin(async move {
            let _queue_guard = shutdown_queue.lock().await;
            if let Some(previous) = previous {
                previous.await;
            }
            prerequisite.await?;
            if let Some(blocked) = blocked {
                anyhow::bail!("{}", blocked.user_facing_message());
            }
            let mut collection = shutdown_committed.lock().clone();
            let id = Self::apply_to_collection(
                &mut collection,
                WorkspaceConfigurationMutation::Checkpoint {
                    id,
                    members,
                    active_member,
                },
            )?;
            let json = serde_json::to_string(&collection)
                .context("serializing the workspace configuration collection")?;
            kvp.scoped(WORKSPACE_CONFIGURATIONS_NAMESPACE)
                .write(WORKSPACE_CONFIGURATIONS_KEY.to_string(), json)
                .await
                .context("writing the workspace configuration collection")?;
            *shutdown_committed.lock() = collection;
            Ok(StoreCommit {
                id,
                generation: generation + 1,
            })
        })
    }

    async fn apply(
        mutation: WorkspaceConfigurationMutation,
        cx: &mut AsyncApp,
    ) -> Result<StoreCommit> {
        let (mut collection, kvp) = cx.update(|cx| {
            let this = Self::global(cx);
            if let Some(blocked) = this.blocked.clone() {
                anyhow::bail!("{}", blocked.user_facing_message());
            }
            #[cfg(test)]
            anyhow::ensure!(
                !this.fail_writes_for_tests,
                "forced workspace configuration write failure"
            );
            Ok((this.committed.clone(), KeyValueStore::global(cx)))
        })?;

        let id = Self::apply_to_collection(&mut collection, mutation)?;

        let json = serde_json::to_string(&collection)
            .context("serializing the workspace configuration collection")?;

        // The in-memory value advances only after the write lands, so a failed write
        // leaves both the stored value and this process's view of it untouched.
        kvp.scoped(WORKSPACE_CONFIGURATIONS_NAMESPACE)
            .write(WORKSPACE_CONFIGURATIONS_KEY.to_string(), json)
            .await
            .context("writing the workspace configuration collection")?;
        Ok(cx.update(|cx| {
            cx.update_global::<Self, _>(|this, cx| {
                this.committed = collection.clone();
                *this.shutdown_committed.lock() = collection;
                this.generation += 1;
                cx.refresh_windows();
                StoreCommit {
                    id,
                    generation: this.generation,
                }
            })
        }))
    }

    fn apply_to_collection(
        collection: &mut model::WorkspaceConfigurationCollection,
        mutation: WorkspaceConfigurationMutation,
    ) -> Result<model::WorkspaceConfigurationId> {
        let id = match mutation {
            WorkspaceConfigurationMutation::Create {
                name,
                members,
                active_member,
            } => {
                let name = name.trim().to_string();
                anyhow::ensure!(!name.is_empty(), "Enter a configuration name.");
                anyhow::ensure!(
                    collection.find_by_name(&name).is_none(),
                    "A workspace configuration named \u{201c}{name}\u{201d} already exists."
                );

                let id = model::WorkspaceConfigurationId::new();
                collection
                    .configurations
                    .push(model::WorkspaceConfiguration {
                        id,
                        name,
                        members,
                        active_member,
                    });
                id
            }
            WorkspaceConfigurationMutation::Checkpoint {
                id,
                members,
                active_member,
            } => {
                let configuration = collection
                    .configurations
                    .iter_mut()
                    .find(|configuration| configuration.id == id)
                    .context("that workspace configuration no longer exists")?;
                configuration.members = members;
                configuration.active_member = active_member;
                id
            }
        };

        Ok(id)
    }
}

pub fn workspace_configuration_referenced_workspace_ids(cx: &App) -> Option<HashSet<WorkspaceId>> {
    WorkspaceConfigurationStore::try_global(cx)
        .and_then(WorkspaceConfigurationStore::referenced_workspace_ids)
}

pub fn read_serialized_multi_workspaces(
    session_workspaces: Vec<model::SessionWorkspace>,
    cx: &App,
) -> Vec<model::SerializedMultiWorkspace> {
    let mut window_groups: Vec<Vec<model::SessionWorkspace>> = Vec::new();
    let mut window_id_to_group: HashMap<WindowId, usize> = HashMap::default();

    for session_workspace in session_workspaces {
        match session_workspace.window_id {
            Some(window_id) => {
                let group_index = *window_id_to_group.entry(window_id).or_insert_with(|| {
                    window_groups.push(Vec::new());
                    window_groups.len() - 1
                });
                window_groups[group_index].push(session_workspace);
            }
            None => {
                window_groups.push(vec![session_workspace]);
            }
        }
    }

    window_groups
        .into_iter()
        .filter_map(|group| {
            let window_id = group.first().and_then(|sw| sw.window_id);
            let state = window_id
                .map(|wid| read_multi_workspace_state(wid, cx))
                .unwrap_or_default();
            let active_workspace_index = state
                .active_workspace_id
                .and_then(|id| group.iter().position(|ws| ws.workspace_id == id))
                // If the persisted active workspace can't be matched (e.g. its
                // pointer was lost or its row was pruned), fall back to the
                // first workspace that actually has paths rather than blindly
                // taking index 0, so a stray scratch/empty workspace isn't
                // restored as the focused window. Only if none have paths do we
                // fall back to the first entry.
                .or_else(|| group.iter().position(|ws| !ws.paths.is_empty()))
                .unwrap_or(0);
            let active_workspace = group.get(active_workspace_index)?.clone();
            Some(model::SerializedMultiWorkspace {
                active_workspace,
                workspaces: group,
                state,
            })
        })
        .collect()
}

const DEFAULT_DOCK_STATE_KEY: &str = "default_dock_state";

pub fn read_default_dock_state(kvp: &KeyValueStore) -> Option<DockStructure> {
    let json_str = kvp.read_kvp(DEFAULT_DOCK_STATE_KEY).log_err().flatten()?;

    serde_json::from_str::<DockStructure>(&json_str).ok()
}

pub async fn write_default_dock_state(
    kvp: &KeyValueStore,
    docks: DockStructure,
) -> anyhow::Result<()> {
    let json_str = serde_json::to_string(&docks)?;
    kvp.write_kvp(DEFAULT_DOCK_STATE_KEY.to_string(), json_str)
        .await?;
    Ok(())
}

#[derive(Debug)]
pub struct Bookmark {
    pub row: u32,
    pub label: String,
}

impl sqlez::bindable::StaticColumnCount for Bookmark {
    fn column_count() -> usize {
        // row, label
        2
    }
}

impl sqlez::bindable::Bind for Bookmark {
    fn bind(
        &self,
        statement: &sqlez::statement::Statement,
        start_index: i32,
    ) -> anyhow::Result<i32> {
        let next_index = statement.bind(&self.row, start_index)?;
        statement.bind(&self.label, next_index)
    }
}

impl Column for Bookmark {
    fn column(statement: &mut Statement, start_index: i32) -> Result<(Self, i32)> {
        let row = statement
            .column_int(start_index)
            .with_context(|| format!("Failed to read bookmark at index {start_index}"))?
            as u32;

        let (label, next_index) = String::column(statement, start_index + 1)?;

        Ok((Bookmark { row, label }, next_index))
    }
}

#[derive(Debug)]
pub struct Breakpoint {
    pub position: u32,
    pub message: Option<Arc<str>>,
    pub condition: Option<Arc<str>>,
    pub hit_condition: Option<Arc<str>>,
    pub state: BreakpointState,
}

/// Wrapper for DB type of a breakpoint
struct BreakpointStateWrapper<'a>(Cow<'a, BreakpointState>);

impl From<BreakpointState> for BreakpointStateWrapper<'static> {
    fn from(kind: BreakpointState) -> Self {
        BreakpointStateWrapper(Cow::Owned(kind))
    }
}

impl StaticColumnCount for BreakpointStateWrapper<'_> {
    fn column_count() -> usize {
        1
    }
}

impl Bind for BreakpointStateWrapper<'_> {
    fn bind(&self, statement: &Statement, start_index: i32) -> anyhow::Result<i32> {
        statement.bind(&self.0.to_int(), start_index)
    }
}

impl Column for BreakpointStateWrapper<'_> {
    fn column(statement: &mut Statement, start_index: i32) -> anyhow::Result<(Self, i32)> {
        let state = statement.column_int(start_index)?;

        match state {
            0 => Ok((BreakpointState::Enabled.into(), start_index + 1)),
            1 => Ok((BreakpointState::Disabled.into(), start_index + 1)),
            _ => anyhow::bail!("Invalid BreakpointState discriminant {state}"),
        }
    }
}

impl sqlez::bindable::StaticColumnCount for Breakpoint {
    fn column_count() -> usize {
        // Position, log message, condition message, and hit condition message
        4 + BreakpointStateWrapper::column_count()
    }
}

impl sqlez::bindable::Bind for Breakpoint {
    fn bind(
        &self,
        statement: &sqlez::statement::Statement,
        start_index: i32,
    ) -> anyhow::Result<i32> {
        let next_index = statement.bind(&self.position, start_index)?;
        let next_index = statement.bind(&self.message, next_index)?;
        let next_index = statement.bind(&self.condition, next_index)?;
        let next_index = statement.bind(&self.hit_condition, next_index)?;
        statement.bind(
            &BreakpointStateWrapper(Cow::Borrowed(&self.state)),
            next_index,
        )
    }
}

impl Column for Breakpoint {
    fn column(statement: &mut Statement, start_index: i32) -> Result<(Self, i32)> {
        let position = statement
            .column_int(start_index)
            .with_context(|| format!("Failed to read BreakPoint at index {start_index}"))?
            as u32;
        let (message, next_index) = Option::<String>::column(statement, start_index + 1)?;
        let (condition, next_index) = Option::<String>::column(statement, next_index)?;
        let (hit_condition, next_index) = Option::<String>::column(statement, next_index)?;
        let (state, next_index) = BreakpointStateWrapper::column(statement, next_index)?;

        Ok((
            Breakpoint {
                position,
                message: message.map(Arc::from),
                condition: condition.map(Arc::from),
                hit_condition: hit_condition.map(Arc::from),
                state: state.0.into_owned(),
            },
            next_index,
        ))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct SerializedPixels(gpui::Pixels);
impl sqlez::bindable::StaticColumnCount for SerializedPixels {}

impl sqlez::bindable::Bind for SerializedPixels {
    fn bind(
        &self,
        statement: &sqlez::statement::Statement,
        start_index: i32,
    ) -> anyhow::Result<i32> {
        let this: i32 = u32::from(self.0) as _;
        this.bind(statement, start_index)
    }
}

pub struct WorkspaceDb(ThreadSafeConnection);

impl Domain for WorkspaceDb {
    const NAME: &str = stringify!(WorkspaceDb);

    const MIGRATIONS: &[&str] = &[
        sql!(
            CREATE TABLE workspaces(
                workspace_id INTEGER PRIMARY KEY,
                workspace_location BLOB UNIQUE,
                dock_visible INTEGER, // Deprecated. Preserving so users can downgrade Zed.
                dock_anchor TEXT, // Deprecated. Preserving so users can downgrade Zed.
                dock_pane INTEGER, // Deprecated.  Preserving so users can downgrade Zed.
                left_sidebar_open INTEGER, // Boolean
                timestamp TEXT DEFAULT CURRENT_TIMESTAMP NOT NULL,
                FOREIGN KEY(dock_pane) REFERENCES panes(pane_id)
            ) STRICT;

            CREATE TABLE pane_groups(
                group_id INTEGER PRIMARY KEY,
                workspace_id INTEGER NOT NULL,
                parent_group_id INTEGER, // NULL indicates that this is a root node
                position INTEGER, // NULL indicates that this is a root node
                axis TEXT NOT NULL, // Enum: 'Vertical' / 'Horizontal'
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                ON DELETE CASCADE
                ON UPDATE CASCADE,
                FOREIGN KEY(parent_group_id) REFERENCES pane_groups(group_id) ON DELETE CASCADE
            ) STRICT;

            CREATE TABLE panes(
                pane_id INTEGER PRIMARY KEY,
                workspace_id INTEGER NOT NULL,
                active INTEGER NOT NULL, // Boolean
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                ON DELETE CASCADE
                ON UPDATE CASCADE
            ) STRICT;

            CREATE TABLE center_panes(
                pane_id INTEGER PRIMARY KEY,
                parent_group_id INTEGER, // NULL means that this is a root pane
                position INTEGER, // NULL means that this is a root pane
                FOREIGN KEY(pane_id) REFERENCES panes(pane_id)
                ON DELETE CASCADE,
                FOREIGN KEY(parent_group_id) REFERENCES pane_groups(group_id) ON DELETE CASCADE
            ) STRICT;

            CREATE TABLE items(
                item_id INTEGER NOT NULL, // This is the item's view id, so this is not unique
                workspace_id INTEGER NOT NULL,
                pane_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                position INTEGER NOT NULL,
                active INTEGER NOT NULL,
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                ON DELETE CASCADE
                ON UPDATE CASCADE,
                FOREIGN KEY(pane_id) REFERENCES panes(pane_id)
                ON DELETE CASCADE,
                PRIMARY KEY(item_id, workspace_id)
            ) STRICT;
        ),
        sql!(
            ALTER TABLE workspaces ADD COLUMN window_state TEXT;
            ALTER TABLE workspaces ADD COLUMN window_x REAL;
            ALTER TABLE workspaces ADD COLUMN window_y REAL;
            ALTER TABLE workspaces ADD COLUMN window_width REAL;
            ALTER TABLE workspaces ADD COLUMN window_height REAL;
            ALTER TABLE workspaces ADD COLUMN display BLOB;
        ),
        // Drop foreign key constraint from workspaces.dock_pane to panes table.
        sql!(
            CREATE TABLE workspaces_2(
                workspace_id INTEGER PRIMARY KEY,
                workspace_location BLOB UNIQUE,
                dock_visible INTEGER, // Deprecated. Preserving so users can downgrade Zed.
                dock_anchor TEXT, // Deprecated. Preserving so users can downgrade Zed.
                dock_pane INTEGER, // Deprecated.  Preserving so users can downgrade Zed.
                left_sidebar_open INTEGER, // Boolean
                timestamp TEXT DEFAULT CURRENT_TIMESTAMP NOT NULL,
                window_state TEXT,
                window_x REAL,
                window_y REAL,
                window_width REAL,
                window_height REAL,
                display BLOB
            ) STRICT;
            INSERT INTO workspaces_2 SELECT * FROM workspaces;
            DROP TABLE workspaces;
            ALTER TABLE workspaces_2 RENAME TO workspaces;
        ),
        // Add panels related information
        sql!(
            ALTER TABLE workspaces ADD COLUMN left_dock_visible INTEGER; //bool
            ALTER TABLE workspaces ADD COLUMN left_dock_active_panel TEXT;
            ALTER TABLE workspaces ADD COLUMN right_dock_visible INTEGER; //bool
            ALTER TABLE workspaces ADD COLUMN right_dock_active_panel TEXT;
            ALTER TABLE workspaces ADD COLUMN bottom_dock_visible INTEGER; //bool
            ALTER TABLE workspaces ADD COLUMN bottom_dock_active_panel TEXT;
        ),
        // Add panel zoom persistence
        sql!(
            ALTER TABLE workspaces ADD COLUMN left_dock_zoom INTEGER; //bool
            ALTER TABLE workspaces ADD COLUMN right_dock_zoom INTEGER; //bool
            ALTER TABLE workspaces ADD COLUMN bottom_dock_zoom INTEGER; //bool
        ),
        // Add pane group flex data
        sql!(
            ALTER TABLE pane_groups ADD COLUMN flexes TEXT;
        ),
        // Add fullscreen field to workspace
        // Deprecated, `WindowBounds` holds the fullscreen state now.
        // Preserving so users can downgrade Zed.
        sql!(
            ALTER TABLE workspaces ADD COLUMN fullscreen INTEGER; //bool
        ),
        // Add preview field to items
        sql!(
            ALTER TABLE items ADD COLUMN preview INTEGER; //bool
        ),
        // Add centered_layout field to workspace
        sql!(
            ALTER TABLE workspaces ADD COLUMN centered_layout INTEGER; //bool
        ),
        sql!(
            CREATE TABLE remote_projects (
                remote_project_id INTEGER NOT NULL UNIQUE,
                path TEXT,
                dev_server_name TEXT
            );
            ALTER TABLE workspaces ADD COLUMN remote_project_id INTEGER;
            ALTER TABLE workspaces RENAME COLUMN workspace_location TO local_paths;
        ),
        sql!(
            DROP TABLE remote_projects;
            CREATE TABLE dev_server_projects (
                id INTEGER NOT NULL UNIQUE,
                path TEXT,
                dev_server_name TEXT
            );
            ALTER TABLE workspaces DROP COLUMN remote_project_id;
            ALTER TABLE workspaces ADD COLUMN dev_server_project_id INTEGER;
        ),
        sql!(
            ALTER TABLE workspaces ADD COLUMN local_paths_order BLOB;
        ),
        sql!(
            ALTER TABLE workspaces ADD COLUMN session_id TEXT DEFAULT NULL;
        ),
        sql!(
            ALTER TABLE workspaces ADD COLUMN window_id INTEGER DEFAULT NULL;
        ),
        sql!(
            ALTER TABLE panes ADD COLUMN pinned_count INTEGER DEFAULT 0;
        ),
        sql!(
            CREATE TABLE ssh_projects (
                id INTEGER PRIMARY KEY,
                host TEXT NOT NULL,
                port INTEGER,
                path TEXT NOT NULL,
                user TEXT
            );
            ALTER TABLE workspaces ADD COLUMN ssh_project_id INTEGER REFERENCES ssh_projects(id) ON DELETE CASCADE;
        ),
        sql!(
            ALTER TABLE ssh_projects RENAME COLUMN path TO paths;
        ),
        sql!(
            CREATE TABLE toolchains (
                workspace_id INTEGER,
                worktree_id INTEGER,
                language_name TEXT NOT NULL,
                name TEXT NOT NULL,
                path TEXT NOT NULL,
                PRIMARY KEY (workspace_id, worktree_id, language_name)
            );
        ),
        sql!(
            ALTER TABLE toolchains ADD COLUMN raw_json TEXT DEFAULT "{}";
        ),
        sql!(
            CREATE TABLE breakpoints (
                workspace_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                breakpoint_location INTEGER NOT NULL,
                kind INTEGER NOT NULL,
                log_message TEXT,
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                ON DELETE CASCADE
                ON UPDATE CASCADE
            );
        ),
        sql!(
            ALTER TABLE workspaces ADD COLUMN local_paths_array TEXT;
            CREATE UNIQUE INDEX local_paths_array_uq ON workspaces(local_paths_array);
            ALTER TABLE workspaces ADD COLUMN local_paths_order_array TEXT;
        ),
        sql!(
            ALTER TABLE breakpoints ADD COLUMN state INTEGER DEFAULT(0) NOT NULL
        ),
        sql!(
            ALTER TABLE breakpoints DROP COLUMN kind
        ),
        sql!(ALTER TABLE toolchains ADD COLUMN relative_worktree_path TEXT DEFAULT "" NOT NULL),
        sql!(
            ALTER TABLE breakpoints ADD COLUMN condition TEXT;
            ALTER TABLE breakpoints ADD COLUMN hit_condition TEXT;
        ),
        sql!(CREATE TABLE toolchains2 (
            workspace_id INTEGER,
            worktree_id INTEGER,
            language_name TEXT NOT NULL,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            raw_json TEXT NOT NULL,
            relative_worktree_path TEXT NOT NULL,
            PRIMARY KEY (workspace_id, worktree_id, language_name, relative_worktree_path)) STRICT;
            INSERT INTO toolchains2
                SELECT * FROM toolchains;
            DROP TABLE toolchains;
            ALTER TABLE toolchains2 RENAME TO toolchains;
        ),
        sql!(
            CREATE TABLE ssh_connections (
                id INTEGER PRIMARY KEY,
                host TEXT NOT NULL,
                port INTEGER,
                user TEXT
            );

            INSERT INTO ssh_connections (host, port, user)
            SELECT DISTINCT host, port, user
            FROM ssh_projects;

            CREATE TABLE workspaces_2(
                workspace_id INTEGER PRIMARY KEY,
                paths TEXT,
                paths_order TEXT,
                ssh_connection_id INTEGER REFERENCES ssh_connections(id),
                timestamp TEXT DEFAULT CURRENT_TIMESTAMP NOT NULL,
                window_state TEXT,
                window_x REAL,
                window_y REAL,
                window_width REAL,
                window_height REAL,
                display BLOB,
                left_dock_visible INTEGER,
                left_dock_active_panel TEXT,
                right_dock_visible INTEGER,
                right_dock_active_panel TEXT,
                bottom_dock_visible INTEGER,
                bottom_dock_active_panel TEXT,
                left_dock_zoom INTEGER,
                right_dock_zoom INTEGER,
                bottom_dock_zoom INTEGER,
                fullscreen INTEGER,
                centered_layout INTEGER,
                session_id TEXT,
                window_id INTEGER
            ) STRICT;

            INSERT
            INTO workspaces_2
            SELECT
                workspaces.workspace_id,
                CASE
                    WHEN ssh_projects.id IS NOT NULL THEN ssh_projects.paths
                    ELSE
                        CASE
                            WHEN workspaces.local_paths_array IS NULL OR workspaces.local_paths_array = "" THEN
                                NULL
                            ELSE
                                replace(workspaces.local_paths_array, ',', CHAR(10))
                        END
                END as paths,

                CASE
                    WHEN ssh_projects.id IS NOT NULL THEN ""
                    ELSE workspaces.local_paths_order_array
                END as paths_order,

                CASE
                    WHEN ssh_projects.id IS NOT NULL THEN (
                        SELECT ssh_connections.id
                        FROM ssh_connections
                        WHERE
                            ssh_connections.host IS ssh_projects.host AND
                            ssh_connections.port IS ssh_projects.port AND
                            ssh_connections.user IS ssh_projects.user
                    )
                    ELSE NULL
                END as ssh_connection_id,

                workspaces.timestamp,
                workspaces.window_state,
                workspaces.window_x,
                workspaces.window_y,
                workspaces.window_width,
                workspaces.window_height,
                workspaces.display,
                workspaces.left_dock_visible,
                workspaces.left_dock_active_panel,
                workspaces.right_dock_visible,
                workspaces.right_dock_active_panel,
                workspaces.bottom_dock_visible,
                workspaces.bottom_dock_active_panel,
                workspaces.left_dock_zoom,
                workspaces.right_dock_zoom,
                workspaces.bottom_dock_zoom,
                workspaces.fullscreen,
                workspaces.centered_layout,
                workspaces.session_id,
                workspaces.window_id
            FROM
                workspaces LEFT JOIN
                ssh_projects ON
                workspaces.ssh_project_id = ssh_projects.id;

            DELETE FROM workspaces_2
            WHERE workspace_id NOT IN (
                SELECT MAX(workspace_id)
                FROM workspaces_2
                GROUP BY ssh_connection_id, paths
            );

            DROP TABLE ssh_projects;
            DROP TABLE workspaces;
            ALTER TABLE workspaces_2 RENAME TO workspaces;

            CREATE UNIQUE INDEX ix_workspaces_location ON workspaces(ssh_connection_id, paths);
        ),
        // Fix any data from when workspaces.paths were briefly encoded as JSON arrays
        sql!(
            UPDATE workspaces
            SET paths = CASE
                WHEN substr(paths, 1, 2) = '[' || '"' AND substr(paths, -2, 2) = '"' || ']' THEN
                    replace(
                        substr(paths, 3, length(paths) - 4),
                        '"' || ',' || '"',
                        CHAR(10)
                    )
                ELSE
                    replace(paths, ',', CHAR(10))
            END
            WHERE paths IS NOT NULL
        ),
        sql!(
            CREATE TABLE remote_connections(
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                host TEXT,
                port INTEGER,
                user TEXT,
                distro TEXT
            );

            CREATE TABLE workspaces_2(
                workspace_id INTEGER PRIMARY KEY,
                paths TEXT,
                paths_order TEXT,
                remote_connection_id INTEGER REFERENCES remote_connections(id),
                timestamp TEXT DEFAULT CURRENT_TIMESTAMP NOT NULL,
                window_state TEXT,
                window_x REAL,
                window_y REAL,
                window_width REAL,
                window_height REAL,
                display BLOB,
                left_dock_visible INTEGER,
                left_dock_active_panel TEXT,
                right_dock_visible INTEGER,
                right_dock_active_panel TEXT,
                bottom_dock_visible INTEGER,
                bottom_dock_active_panel TEXT,
                left_dock_zoom INTEGER,
                right_dock_zoom INTEGER,
                bottom_dock_zoom INTEGER,
                fullscreen INTEGER,
                centered_layout INTEGER,
                session_id TEXT,
                window_id INTEGER
            ) STRICT;

            INSERT INTO remote_connections
            SELECT
                id,
                "ssh" as kind,
                host,
                port,
                user,
                NULL as distro
            FROM ssh_connections;

            INSERT
            INTO workspaces_2
            SELECT
                workspace_id,
                paths,
                paths_order,
                ssh_connection_id as remote_connection_id,
                timestamp,
                window_state,
                window_x,
                window_y,
                window_width,
                window_height,
                display,
                left_dock_visible,
                left_dock_active_panel,
                right_dock_visible,
                right_dock_active_panel,
                bottom_dock_visible,
                bottom_dock_active_panel,
                left_dock_zoom,
                right_dock_zoom,
                bottom_dock_zoom,
                fullscreen,
                centered_layout,
                session_id,
                window_id
            FROM
                workspaces;

            DROP TABLE workspaces;
            ALTER TABLE workspaces_2 RENAME TO workspaces;

            CREATE UNIQUE INDEX ix_workspaces_location ON workspaces(remote_connection_id, paths);
        ),
        sql!(CREATE TABLE user_toolchains (
            remote_connection_id INTEGER,
            workspace_id INTEGER NOT NULL,
            worktree_id INTEGER NOT NULL,
            relative_worktree_path TEXT NOT NULL,
            language_name TEXT NOT NULL,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            raw_json TEXT NOT NULL,

            PRIMARY KEY (workspace_id, worktree_id, relative_worktree_path, language_name, name, path, raw_json)
        ) STRICT;),
        sql!(
            DROP TABLE ssh_connections;
        ),
        sql!(
            ALTER TABLE remote_connections ADD COLUMN name TEXT;
            ALTER TABLE remote_connections ADD COLUMN container_id TEXT;
        ),
        sql!(
            CREATE TABLE IF NOT EXISTS trusted_worktrees (
                trust_id INTEGER PRIMARY KEY AUTOINCREMENT,
                absolute_path TEXT,
                user_name TEXT,
                host_name TEXT
            ) STRICT;
        ),
        sql!(CREATE TABLE toolchains2 (
            workspace_id INTEGER,
            worktree_root_path TEXT NOT NULL,
            language_name TEXT NOT NULL,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            raw_json TEXT NOT NULL,
            relative_worktree_path TEXT NOT NULL,
            PRIMARY KEY (workspace_id, worktree_root_path, language_name, relative_worktree_path)) STRICT;
            INSERT OR REPLACE INTO toolchains2
                // The `instr(paths, '\n') = 0` part allows us to find all
                // workspaces that have a single worktree, as `\n` is used as a
                // separator when serializing the workspace paths, so if no `\n` is
                // found, we know we have a single worktree.
                SELECT toolchains.workspace_id, paths, language_name, name, path, raw_json, relative_worktree_path FROM toolchains INNER JOIN workspaces ON toolchains.workspace_id = workspaces.workspace_id AND instr(paths, '\n') = 0;
            DROP TABLE toolchains;
            ALTER TABLE toolchains2 RENAME TO toolchains;
        ),
        sql!(CREATE TABLE user_toolchains2 (
            remote_connection_id INTEGER,
            workspace_id INTEGER NOT NULL,
            worktree_root_path TEXT NOT NULL,
            relative_worktree_path TEXT NOT NULL,
            language_name TEXT NOT NULL,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            raw_json TEXT NOT NULL,

            PRIMARY KEY (workspace_id, worktree_root_path, relative_worktree_path, language_name, name, path, raw_json)) STRICT;
            INSERT OR REPLACE INTO user_toolchains2
                // The `instr(paths, '\n') = 0` part allows us to find all
                // workspaces that have a single worktree, as `\n` is used as a
                // separator when serializing the workspace paths, so if no `\n` is
                // found, we know we have a single worktree.
                SELECT user_toolchains.remote_connection_id, user_toolchains.workspace_id, paths, relative_worktree_path, language_name, name, path, raw_json  FROM user_toolchains INNER JOIN workspaces ON user_toolchains.workspace_id = workspaces.workspace_id AND instr(paths, '\n') = 0;
            DROP TABLE user_toolchains;
            ALTER TABLE user_toolchains2 RENAME TO user_toolchains;
        ),
        sql!(
            ALTER TABLE remote_connections ADD COLUMN use_podman BOOLEAN;
        ),
        sql!(
            ALTER TABLE remote_connections ADD COLUMN remote_env TEXT;
        ),
        sql!(
            CREATE TABLE bookmarks (
                workspace_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                row INTEGER NOT NULL,
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                ON DELETE CASCADE
                ON UPDATE CASCADE
            );
        ),
        sql!(
            ALTER TABLE workspaces ADD COLUMN identity_paths TEXT;
            ALTER TABLE workspaces ADD COLUMN identity_paths_order TEXT;
        ),
        sql!(
            ALTER TABLE bookmarks ADD COLUMN label TEXT NOT NULL DEFAULT "";
        ),
    ];

    // Allow recovering from bad migration that was initially shipped to nightly
    // when introducing the ssh_connections table.
    fn should_allow_migration_change(_index: usize, old: &str, new: &str) -> bool {
        old.starts_with("CREATE TABLE ssh_connections")
            && new.starts_with("CREATE TABLE ssh_connections")
    }
}

db::static_connection!(WorkspaceDb, []);

impl WorkspaceDb {
    /// Returns a serialized workspace for the given worktree_roots. If the passed array
    /// is empty, the most recent workspace is returned instead. If no workspace for the
    /// passed roots is stored, returns none.
    pub(crate) fn workspace_for_roots<P: AsRef<Path>>(
        &self,
        worktree_roots: &[P],
    ) -> Option<SerializedWorkspace> {
        self.workspace_for_roots_internal(worktree_roots, None)
    }

    pub(crate) fn remote_workspace_for_roots<P: AsRef<Path>>(
        &self,
        worktree_roots: &[P],
        remote_project_id: RemoteConnectionId,
    ) -> Option<SerializedWorkspace> {
        self.workspace_for_roots_internal(worktree_roots, Some(remote_project_id))
    }

    pub(crate) fn workspace_for_roots_internal<P: AsRef<Path>>(
        &self,
        worktree_roots: &[P],
        remote_connection_id: Option<RemoteConnectionId>,
    ) -> Option<SerializedWorkspace> {
        // paths are sorted before db interactions to ensure that the order of the paths
        // doesn't affect the workspace selection for existing workspaces
        let root_paths = PathList::new(worktree_roots);

        // Empty workspaces cannot be matched by paths (all empty workspaces have paths = "").
        // They should only be restored via workspace_for_id during session restoration.
        if root_paths.is_empty() && remote_connection_id.is_none() {
            return None;
        }

        // Note that we re-assign the workspace_id here in case it's empty
        // and we've grabbed the most recent workspace
        let (
            workspace_id,
            paths,
            paths_order,
            identity_paths,
            identity_paths_order,
            window_bounds,
            display,
            centered_layout,
            docks,
            window_id,
        ): (
            WorkspaceId,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<SerializedWindowBounds>,
            Option<Uuid>,
            Option<bool>,
            DockStructure,
            Option<u64>,
        ) = self
            .select_row_bound(sql! {
                SELECT
                    workspace_id,
                    paths,
                    paths_order,
                    identity_paths,
                    identity_paths_order,
                    window_state,
                    window_x,
                    window_y,
                    window_width,
                    window_height,
                    display,
                    centered_layout,
                    left_dock_visible,
                    left_dock_active_panel,
                    left_dock_zoom,
                    right_dock_visible,
                    right_dock_active_panel,
                    right_dock_zoom,
                    bottom_dock_visible,
                    bottom_dock_active_panel,
                    bottom_dock_zoom,
                    window_id
                FROM workspaces
                WHERE
                    paths IS ? AND
                    remote_connection_id IS ?
                LIMIT 1
            })
            .and_then(|mut prepared_statement| {
                (prepared_statement)((
                    root_paths.serialize().paths,
                    remote_connection_id.map(|id| id.0 as i32),
                ))
            })
            .context("No workspaces found")
            .warn_on_err()
            .flatten()?;

        let paths = PathList::deserialize(&SerializedPathList {
            paths,
            order: paths_order,
        });
        let identity_paths = identity_paths.map(|paths| {
            PathList::deserialize(&SerializedPathList {
                paths,
                order: identity_paths_order.unwrap_or_default(),
            })
        });

        let remote_connection_options = if let Some(remote_connection_id) = remote_connection_id {
            self.remote_connection(remote_connection_id)
                .context("Get remote connection")
                .log_err()
        } else {
            None
        };

        Some(SerializedWorkspace {
            id: workspace_id,
            location: match remote_connection_options {
                Some(options) => SerializedWorkspaceLocation::Remote(options),
                None => SerializedWorkspaceLocation::Local,
            },
            paths,
            identity_paths,
            center_group: self
                .get_center_pane_group(workspace_id)
                .context("Getting center group")
                .log_err()?,
            window_bounds,
            centered_layout: centered_layout.unwrap_or(false),
            display,
            docks,
            session_id: None,
            bookmarks: self.bookmarks(workspace_id).log_err().unwrap_or_default(),
            breakpoints: self.breakpoints(workspace_id).log_err().unwrap_or_default(),
            window_id,
            user_toolchains: self
                .user_toolchains(workspace_id, remote_connection_id)
                .log_err()
                .unwrap_or_default(),
        })
    }

    /// Returns the workspace with the given ID, loading all associated data.
    pub(crate) fn workspace_for_id(
        &self,
        workspace_id: WorkspaceId,
    ) -> Option<SerializedWorkspace> {
        self.workspace_for_id_checked(workspace_id)
            .log_err()
            .flatten()
    }

    pub(crate) fn workspace_for_id_checked(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Option<SerializedWorkspace>> {
        let row: Option<(
            String,
            String,
            Option<String>,
            Option<String>,
            Option<SerializedWindowBounds>,
            Option<Uuid>,
            Option<bool>,
            DockStructure,
            Option<u64>,
            Option<i32>,
        )> = self
            .select_row_bound(sql! {
                SELECT
                    paths,
                    paths_order,
                    identity_paths,
                    identity_paths_order,
                    window_state,
                    window_x,
                    window_y,
                    window_width,
                    window_height,
                    display,
                    centered_layout,
                    left_dock_visible,
                    left_dock_active_panel,
                    left_dock_zoom,
                    right_dock_visible,
                    right_dock_active_panel,
                    right_dock_zoom,
                    bottom_dock_visible,
                    bottom_dock_active_panel,
                    bottom_dock_zoom,
                    window_id,
                    remote_connection_id
                FROM workspaces
                WHERE workspace_id = ?
            })
            .and_then(|mut prepared_statement| (prepared_statement)(workspace_id))?;
        let Some((
            paths,
            paths_order,
            identity_paths,
            identity_paths_order,
            window_bounds,
            display,
            centered_layout,
            docks,
            window_id,
            remote_connection_id,
        )) = row
        else {
            return Ok(None);
        };

        let paths = PathList::deserialize(&SerializedPathList {
            paths,
            order: paths_order,
        });
        let identity_paths = identity_paths.map(|paths| {
            PathList::deserialize(&SerializedPathList {
                paths,
                order: identity_paths_order.unwrap_or_default(),
            })
        });

        let remote_connection_id = remote_connection_id.map(|id| RemoteConnectionId(id as u64));
        let remote_connection_options = if let Some(remote_connection_id) = remote_connection_id {
            Some(
                self.remote_connection(remote_connection_id)
                    .context("getting remote connection for exact workspace restore")?,
            )
        } else {
            None
        };

        Ok(Some(SerializedWorkspace {
            id: workspace_id,
            location: match remote_connection_options {
                Some(options) => SerializedWorkspaceLocation::Remote(options),
                None => SerializedWorkspaceLocation::Local,
            },
            paths,
            identity_paths,
            center_group: self
                .get_center_pane_group(workspace_id)
                .context("getting center group for exact workspace restore")?,
            window_bounds,
            centered_layout: centered_layout.unwrap_or(false),
            display,
            docks,
            session_id: None,
            bookmarks: self.bookmarks(workspace_id)?,
            breakpoints: self.breakpoints(workspace_id)?,
            window_id,
            user_toolchains: self.user_toolchains(workspace_id, remote_connection_id)?,
        }))
    }

    pub(crate) fn workspace_for_local_identity_paths(
        &self,
        identity_paths: &[PathBuf],
    ) -> Option<SerializedWorkspace> {
        self.workspace_for_local_identity_paths_checked(identity_paths)
            .log_err()
            .flatten()
    }

    pub(crate) fn workspace_for_local_identity_paths_checked(
        &self,
        identity_paths: &[PathBuf],
    ) -> Result<Option<SerializedWorkspace>> {
        if identity_paths.is_empty() {
            return Ok(None);
        }
        for (workspace_id, paths, stored_identity_paths, remote_connection_id, _, _) in
            self.recent_workspaces()?
        {
            if remote_connection_id.is_none()
                && stored_identity_paths.unwrap_or(paths).paths() == identity_paths
            {
                return self.workspace_for_id_checked(workspace_id);
            }
        }
        Ok(None)
    }

    fn bookmarks(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<BTreeMap<Arc<Path>, Vec<SerializedBookmark>>> {
        let bookmarks: Result<Vec<(PathBuf, Bookmark)>> = self
            .select_bound(sql! {
                SELECT path, row, label
                FROM bookmarks
                WHERE workspace_id = ?
                ORDER BY path, row
            })
            .and_then(|mut prepared_statement| (prepared_statement)(workspace_id));

        let bookmarks = bookmarks?;
        if bookmarks.is_empty() {
            log::debug!("Bookmarks are empty after querying database for them");
        }

        let mut map: BTreeMap<_, Vec<_>> = BTreeMap::default();

        for (path, bookmark) in bookmarks {
            let path: Arc<Path> = path.into();
            map.entry(path.clone())
                .or_default()
                .push(SerializedBookmark {
                    row: bookmark.row,
                    label: bookmark.label,
                })
        }

        Ok(map)
    }

    fn breakpoints(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<BTreeMap<Arc<Path>, Vec<SourceBreakpoint>>> {
        let breakpoints: Result<Vec<(PathBuf, Breakpoint)>> = self
            .select_bound(sql! {
                SELECT path, breakpoint_location, log_message, condition, hit_condition, state
                FROM breakpoints
                WHERE workspace_id = ?
            })
            .and_then(|mut prepared_statement| (prepared_statement)(workspace_id));

        let breakpoints = breakpoints?;
        if breakpoints.is_empty() {
            log::debug!("Breakpoints are empty after querying database for them");
        }

        let mut map: BTreeMap<Arc<Path>, Vec<SourceBreakpoint>> = Default::default();

        for (path, breakpoint) in breakpoints {
            let path: Arc<Path> = path.into();
            map.entry(path.clone()).or_default().push(SourceBreakpoint {
                row: breakpoint.position,
                path,
                message: breakpoint.message,
                condition: breakpoint.condition,
                hit_condition: breakpoint.hit_condition,
                state: breakpoint.state,
            });
        }

        for (path, breakpoints) in map.iter() {
            log::info!(
                "Got {} breakpoints from database at path: {}",
                breakpoints.len(),
                path.to_string_lossy()
            );
        }

        Ok(map)
    }

    fn user_toolchains(
        &self,
        workspace_id: WorkspaceId,
        remote_connection_id: Option<RemoteConnectionId>,
    ) -> Result<BTreeMap<ToolchainScope, IndexSet<Toolchain>>> {
        type RowKind = (WorkspaceId, String, String, String, String, String, String);

        let toolchains: Vec<RowKind> = self
            .select_bound(sql! {
                SELECT workspace_id, worktree_root_path, relative_worktree_path,
                language_name, name, path, raw_json
                FROM user_toolchains WHERE remote_connection_id IS ?1 AND (
                      workspace_id IN (0, ?2)
                )
            })
            .and_then(|mut statement| {
                (statement)((remote_connection_id.map(|id| id.0), workspace_id))
            })?;
        let mut ret = BTreeMap::<_, IndexSet<_>>::default();

        for (
            _workspace_id,
            worktree_root_path,
            relative_worktree_path,
            language_name,
            name,
            path,
            raw_json,
        ) in toolchains
        {
            // INTEGER's that are primary keys (like workspace ids, remote connection ids and such) start at 1, so we're safe to
            let scope = if _workspace_id == WorkspaceId(0) {
                debug_assert_eq!(worktree_root_path, String::default());
                debug_assert_eq!(relative_worktree_path, String::default());
                ToolchainScope::Global
            } else {
                debug_assert_eq!(workspace_id, _workspace_id);
                debug_assert_eq!(
                    worktree_root_path == String::default(),
                    relative_worktree_path == String::default()
                );

                let Some(relative_path) = RelPath::from_unix_str(&relative_worktree_path).log_err()
                else {
                    continue;
                };
                if worktree_root_path != String::default()
                    && relative_worktree_path != String::default()
                {
                    ToolchainScope::Subproject(
                        Arc::from(worktree_root_path.as_ref()),
                        relative_path.into(),
                    )
                } else {
                    ToolchainScope::Project
                }
            };
            let Ok(as_json) = serde_json::from_str(&raw_json) else {
                continue;
            };
            let toolchain = Toolchain {
                name: SharedString::from(name),
                path: SharedString::from(path),
                language_name: LanguageName::from_proto(language_name),
                as_json,
            };
            ret.entry(scope).or_default().insert(toolchain);
        }

        Ok(ret)
    }

    pub(crate) async fn save_workspace_checked(
        &self,
        workspace: SerializedWorkspace,
    ) -> Result<()> {
        self.write(move |connection| Self::save_workspace_on_connection(connection, workspace))
            .await
    }

    #[cfg(test)]
    pub(crate) fn fail_next_workspace_write_for_tests(workspace_id: WorkspaceId) {
        FAIL_NEXT_WORKSPACE_WRITE.lock().insert(workspace_id);
    }

    pub(crate) async fn begin_exact_checkpoint_transaction(
        &self,
    ) -> Result<sqlez::thread_safe_connection::WriteTransaction> {
        self.0.begin_write_transaction().await
    }

    pub(crate) fn queue_exact_checkpoint_transaction(
        &self,
    ) -> (
        sqlez::thread_safe_connection::WriteTransaction,
        impl std::future::Future<Output = Result<()>> + use<>,
    ) {
        self.0.queue_write_transaction()
    }

    pub(crate) async fn save_workspace_in_transaction(
        transaction: &sqlez::thread_safe_connection::WriteTransaction,
        workspace: SerializedWorkspace,
    ) -> Result<()> {
        transaction
            .write(move |connection| Self::save_workspace_on_connection(connection, workspace))
            .await?
    }

    fn save_workspace_on_connection(
        conn: &Connection,
        workspace: SerializedWorkspace,
    ) -> Result<()> {
        #[cfg(test)]
        if FAIL_NEXT_WORKSPACE_WRITE.lock().remove(&workspace.id) {
            bail!("forced workspace row write failure");
        }

        let paths = workspace.paths.serialize();
        let identity_paths = workspace.identity_paths.map(|paths| paths.serialize());
        log::debug!("Saving workspace at location: {:?}", workspace.location);
        conn.with_savepoint("update_worktrees", || {
                let remote_connection_id = match workspace.location.clone() {
                    SerializedWorkspaceLocation::Local => None,
                    SerializedWorkspaceLocation::Remote(connection_options) => {
                        Some(Self::get_or_create_remote_connection_internal(
                            conn,
                            connection_options
                        )?.0)
                    }
                };

                // Clear out panes and pane_groups
                conn.exec_bound(sql!(
                    DELETE FROM pane_groups WHERE workspace_id = ?1;
                    DELETE FROM panes WHERE workspace_id = ?1;))?(workspace.id)
                    .context("Clearing old panes")?;

                conn.exec_bound(
                    sql!(
                        DELETE FROM bookmarks WHERE workspace_id = ?1;
                    )
                )?(workspace.id).context("Clearing old bookmarks")?;

                for (path, bookmarks) in workspace.bookmarks {
                    for bookmark in bookmarks {
                        conn.exec_bound(sql!(
                            INSERT INTO bookmarks (workspace_id, path, row, label)
                            VALUES (?1, ?2, ?3, ?4);
                        ))?((workspace.id, path.as_ref(), bookmark.row, bookmark.label)).context("Inserting bookmark")?;
                    }
                }

                conn.exec_bound(
                    sql!(
                        DELETE FROM breakpoints WHERE workspace_id = ?1;
                    )
                )?(workspace.id).context("Clearing old breakpoints")?;

                for (path, breakpoints) in workspace.breakpoints {
                    for bp in breakpoints {
                        let state = BreakpointStateWrapper::from(bp.state);
                        conn.exec_bound(sql!(
                            INSERT INTO breakpoints (workspace_id, path, breakpoint_location,  log_message, condition, hit_condition, state)
                            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);))?((
                            workspace.id,
                            path.as_ref(),
                            bp.row,
                            bp.message,
                            bp.condition,
                            bp.hit_condition,
                            state,
                        ))
                        .with_context(|| {
                            format!(
                                "storing breakpoint at row {} in {}",
                                bp.row,
                                path.to_string_lossy()
                            )
                        })?;
                    }
                }

                conn.exec_bound(
                    sql!(
                        DELETE FROM user_toolchains WHERE workspace_id = ?1;
                    )
                )?(workspace.id).context("Clearing old user toolchains")?;

                for (scope, toolchains) in workspace.user_toolchains {
                    for toolchain in toolchains {
                        let query = sql!(INSERT OR REPLACE INTO user_toolchains(remote_connection_id, workspace_id, worktree_root_path, relative_worktree_path, language_name, name, path, raw_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8));
                        let (workspace_id, worktree_root_path, relative_worktree_path) = match scope {
                            ToolchainScope::Subproject(ref worktree_root_path, ref path) => (Some(workspace.id), Some(worktree_root_path.to_string_lossy().into_owned()), Some(path.as_unix_str().to_owned())),
                            ToolchainScope::Project => (Some(workspace.id), None, None),
                            ToolchainScope::Global => (None, None, None),
                        };
                        let args = (remote_connection_id, workspace_id.unwrap_or(WorkspaceId(0)), worktree_root_path.unwrap_or_default(), relative_worktree_path.unwrap_or_default(),
                        toolchain.language_name.as_ref().to_owned(), toolchain.name.to_string(), toolchain.path.to_string(), toolchain.as_json.to_string());
                        conn.exec_bound(query)?(args)
                            .context("storing workspace user toolchain")?;
                    }
                }

                // Clear out old workspaces with the same paths.
                // Skip this for empty workspaces - they are identified by workspace_id, not paths.
                // Multiple empty workspaces with different content should coexist.
                if !paths.paths.is_empty() {
                    conn.exec_bound(sql!(
                        DELETE
                        FROM workspaces
                        WHERE
                            workspace_id != ?1 AND
                            paths IS ?2 AND
                            remote_connection_id IS ?3
                    ))?((
                        workspace.id,
                        paths.paths.clone(),
                        remote_connection_id,
                    ))
                    .context("clearing out old locations")?;
                }

                // Upsert
                let query = sql!(
                    INSERT INTO workspaces(
                        workspace_id,
                        paths,
                        paths_order,
                        identity_paths,
                        identity_paths_order,
                        remote_connection_id,
                        left_dock_visible,
                        left_dock_active_panel,
                        left_dock_zoom,
                        right_dock_visible,
                        right_dock_active_panel,
                        right_dock_zoom,
                        bottom_dock_visible,
                        bottom_dock_active_panel,
                        bottom_dock_zoom,
                        session_id,
                        window_id,
                        window_state,
                        window_x,
                        window_y,
                        window_width,
                        window_height,
                        display,
                        centered_layout,
                        timestamp
                    )
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, CURRENT_TIMESTAMP)
                    ON CONFLICT DO
                    UPDATE SET
                        paths = ?2,
                        paths_order = ?3,
                        identity_paths = ?4,
                        identity_paths_order = ?5,
                        remote_connection_id = ?6,
                        left_dock_visible = ?7,
                        left_dock_active_panel = ?8,
                        left_dock_zoom = ?9,
                        right_dock_visible = ?10,
                        right_dock_active_panel = ?11,
                        right_dock_zoom = ?12,
                        bottom_dock_visible = ?13,
                        bottom_dock_active_panel = ?14,
                        bottom_dock_zoom = ?15,
                        session_id = ?16,
                        window_id = ?17,
                        window_state = ?18,
                        window_x = ?19,
                        window_y = ?20,
                        window_width = ?21,
                        window_height = ?22,
                        display = ?23,
                        centered_layout = ?24,
                        timestamp = CURRENT_TIMESTAMP
                );
                let mut prepared_query = conn.exec_bound(query)?;
                let args = (
                    workspace.id,
                    paths.paths.clone(),
                    paths.order.clone(),
                    identity_paths.as_ref().map(|paths| paths.paths.clone()),
                    identity_paths.as_ref().map(|paths| paths.order.clone()),
                    remote_connection_id,
                    workspace.docks,
                    workspace.session_id,
                    workspace.window_id,
                    (
                        workspace.window_bounds,
                        workspace.display,
                        workspace.centered_layout,
                    ),
                );

                prepared_query(args).context("Updating workspace")?;

                // Save center pane group
                Self::save_pane_group(conn, workspace.id, &workspace.center_group, None)
                    .context("save pane group in save workspace")?;

            Ok(())
        })
    }

    pub(crate) async fn save_workspace(&self, workspace: SerializedWorkspace) {
        self.save_workspace_checked(workspace).await.log_err();
    }

    #[cfg(test)]
    pub(crate) async fn set_query_only_for_tests(&self, query_only: bool) -> Result<()> {
        let query = if query_only {
            "PRAGMA query_only = ON"
        } else {
            "PRAGMA query_only = OFF"
        };
        self.write(move |connection| connection.exec(query).and_then(|mut statement| statement()))
            .await
    }

    pub(crate) async fn get_or_create_remote_connection(
        &self,
        options: RemoteConnectionOptions,
    ) -> Result<RemoteConnectionId> {
        self.write(move |conn| Self::get_or_create_remote_connection_internal(conn, options))
            .await
    }

    fn get_or_create_remote_connection_internal(
        this: &Connection,
        options: RemoteConnectionOptions,
    ) -> Result<RemoteConnectionId> {
        let identity = remote_connection_identity(&options);
        let kind;
        let user: Option<String>;
        let mut host = None;
        let mut port = None;
        let mut distro = None;
        let mut name = None;
        let mut container_id = None;
        let mut use_podman = None;
        let mut remote_env = None;

        match identity {
            RemoteConnectionIdentity::Ssh {
                host: identity_host,
                username,
                port: identity_port,
            } => {
                kind = RemoteConnectionKind::Ssh;
                host = Some(identity_host);
                port = identity_port;
                user = username;
            }
            RemoteConnectionIdentity::Wsl {
                distro_name,
                user: identity_user,
            } => {
                kind = RemoteConnectionKind::Wsl;
                distro = Some(distro_name);
                user = identity_user;
            }
            RemoteConnectionIdentity::Docker {
                container_id: identity_container_id,
                name: identity_name,
                remote_user,
            } => {
                kind = RemoteConnectionKind::Docker;
                container_id = Some(identity_container_id);
                name = Some(identity_name);
                user = Some(remote_user);
            }
            #[cfg(any(test, feature = "test-support"))]
            RemoteConnectionIdentity::Mock { id } => {
                kind = RemoteConnectionKind::Ssh;
                host = Some(format!("mock-{}", id));
                user = Some(format!("mock-user-{}", id));
            }
        }

        if let RemoteConnectionOptions::Docker(options) = options {
            use_podman = Some(options.use_podman);
            remote_env = serde_json::to_string(&options.remote_env).ok();
        }

        Self::get_or_create_remote_connection_query(
            this,
            kind,
            host,
            port,
            user,
            distro,
            name,
            container_id,
            use_podman,
            remote_env,
        )
    }

    fn get_or_create_remote_connection_query(
        this: &Connection,
        kind: RemoteConnectionKind,
        host: Option<String>,
        port: Option<u16>,
        user: Option<String>,
        distro: Option<String>,
        name: Option<String>,
        container_id: Option<String>,
        use_podman: Option<bool>,
        remote_env: Option<String>,
    ) -> Result<RemoteConnectionId> {
        if let Some(id) = this.select_row_bound(sql!(
            SELECT id
            FROM remote_connections
            WHERE
                kind IS ? AND
                host IS ? AND
                port IS ? AND
                user IS ? AND
                distro IS ? AND
                name IS ? AND
                container_id IS ?
            LIMIT 1
        ))?((
            kind.serialize(),
            host.clone(),
            port,
            user.clone(),
            distro.clone(),
            name.clone(),
            container_id.clone(),
        ))? {
            Ok(RemoteConnectionId(id))
        } else {
            let id = this.select_row_bound(sql!(
                INSERT INTO remote_connections (
                    kind,
                    host,
                    port,
                    user,
                    distro,
                    name,
                    container_id,
                    use_podman,
                    remote_env
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                RETURNING id
            ))?((
                kind.serialize(),
                host,
                port,
                user,
                distro,
                name,
                container_id,
                use_podman,
                remote_env,
            ))?
            .context("failed to insert remote project")?;
            Ok(RemoteConnectionId(id))
        }
    }

    query! {
        pub async fn next_id() -> Result<WorkspaceId> {
            INSERT INTO workspaces DEFAULT VALUES RETURNING workspace_id
        }
    }

    fn recent_workspaces(
        &self,
    ) -> Result<
        Vec<(
            WorkspaceId,
            PathList,
            Option<PathList>,
            Option<RemoteConnectionId>,
            Option<String>,
            DateTime<Utc>,
        )>,
    > {
        Ok(self
            .recent_workspaces_query()?
            .into_iter()
            .map(
                |(
                    id,
                    paths,
                    order,
                    identity_paths,
                    identity_paths_order,
                    remote_connection_id,
                    session_id,
                    timestamp,
                )| {
                    (
                        id,
                        PathList::deserialize(&SerializedPathList { paths, order }),
                        identity_paths.map(|paths| {
                            PathList::deserialize(&SerializedPathList {
                                paths,
                                order: identity_paths_order.unwrap_or_default(),
                            })
                        }),
                        remote_connection_id.map(RemoteConnectionId),
                        session_id,
                        parse_timestamp(&timestamp),
                    )
                },
            )
            .collect())
    }

    query! {
        fn recent_workspaces_query() -> Result<Vec<(WorkspaceId, String, String, Option<String>, Option<String>, Option<u64>, Option<String>, String)>> {
            SELECT workspace_id, paths, paths_order, identity_paths, identity_paths_order, remote_connection_id, session_id, timestamp
            FROM workspaces
            WHERE
                paths IS NOT NULL OR
                remote_connection_id IS NOT NULL
            ORDER BY timestamp DESC
        }
    }

    fn session_workspaces(
        &self,
        session_id: String,
    ) -> Result<
        Vec<(
            WorkspaceId,
            PathList,
            Option<u64>,
            Option<RemoteConnectionId>,
        )>,
    > {
        Ok(self
            .session_workspaces_query(session_id)?
            .into_iter()
            .map(
                |(workspace_id, paths, order, window_id, remote_connection_id)| {
                    (
                        WorkspaceId(workspace_id),
                        PathList::deserialize(&SerializedPathList { paths, order }),
                        window_id,
                        remote_connection_id.map(RemoteConnectionId),
                    )
                },
            )
            .collect())
    }

    query! {
        fn session_workspaces_query(session_id: String) -> Result<Vec<(i64, String, String, Option<u64>, Option<u64>)>> {
            SELECT workspace_id, paths, paths_order, window_id, remote_connection_id
            FROM workspaces
            WHERE session_id = ?1
            ORDER BY timestamp DESC
        }
    }

    query! {
        pub fn breakpoints_for_file(workspace_id: WorkspaceId, file_path: &Path) -> Result<Vec<Breakpoint>> {
            SELECT breakpoint_location
            FROM breakpoints
            WHERE  workspace_id= ?1 AND path = ?2
        }
    }

    query! {
        pub fn clear_breakpoints(file_path: &Path) -> Result<()> {
            DELETE FROM breakpoints
            WHERE file_path = ?2
        }
    }

    fn remote_connections(&self) -> Result<HashMap<RemoteConnectionId, RemoteConnectionOptions>> {
        Ok(self.select(sql!(
            SELECT
                id, kind, host, port, user, distro, container_id, name, use_podman, remote_env
            FROM
                remote_connections
        ))?()?
        .into_iter()
        .filter_map(
            |(id, kind, host, port, user, distro, container_id, name, use_podman, remote_env)| {
                Some((
                    RemoteConnectionId(id),
                    Self::remote_connection_from_row(
                        kind,
                        host,
                        port,
                        user,
                        distro,
                        container_id,
                        name,
                        use_podman,
                        remote_env,
                    )?,
                ))
            },
        )
        .collect())
    }

    pub(crate) fn remote_connection(
        &self,
        id: RemoteConnectionId,
    ) -> Result<RemoteConnectionOptions> {
        let (kind, host, port, user, distro, container_id, name, use_podman, remote_env) =
            self.select_row_bound(sql!(
                SELECT kind, host, port, user, distro, container_id, name, use_podman, remote_env
                FROM remote_connections
                WHERE id = ?
            ))?(id.0)?
            .context("no such remote connection")?;
        Self::remote_connection_from_row(
            kind,
            host,
            port,
            user,
            distro,
            container_id,
            name,
            use_podman,
            remote_env,
        )
        .context("invalid remote_connection row")
    }

    fn remote_connection_from_row(
        kind: String,
        host: Option<String>,
        port: Option<u16>,
        user: Option<String>,
        distro: Option<String>,
        container_id: Option<String>,
        name: Option<String>,
        use_podman: Option<bool>,
        remote_env: Option<String>,
    ) -> Option<RemoteConnectionOptions> {
        match RemoteConnectionKind::deserialize(&kind)? {
            RemoteConnectionKind::Wsl => Some(RemoteConnectionOptions::Wsl(WslConnectionOptions {
                distro_name: distro?,
                user: user,
            })),
            RemoteConnectionKind::Ssh => Some(RemoteConnectionOptions::Ssh(SshConnectionOptions {
                host: host?.into(),
                port,
                username: user,
                ..Default::default()
            })),
            RemoteConnectionKind::Docker => {
                let remote_env: BTreeMap<String, String> =
                    serde_json::from_str(&remote_env?).ok()?;
                Some(RemoteConnectionOptions::Docker(DockerConnectionOptions {
                    container_id: container_id?,
                    name: name?,
                    remote_user: user?,
                    upload_binary_over_docker_exec: false,
                    use_podman: use_podman?,
                    remote_env,
                }))
            }
        }
    }

    query! {
        async fn delete_workspace_by_id(id: WorkspaceId) -> Result<()> {
            DELETE FROM workspaces
            WHERE workspace_id IS ?
        }
    }

    async fn all_paths_exist_with_a_directory(paths: &[PathBuf], fs: &dyn Fs) -> bool {
        let mut any_dir = false;
        for path in paths {
            match fs.metadata(path).await.ok().flatten() {
                None => return false,
                Some(meta) => {
                    if meta.is_dir {
                        any_dir = true;
                    }
                }
            }
        }
        any_dir
    }

    // Returns the raw recent workspace history. Scratch workspaces (no paths) are filtered
    // out because they are restored separately by `last_session_workspace_locations`.
    pub async fn recent_project_workspaces_ungrouped(
        &self,
        fs: &dyn Fs,
    ) -> Result<Vec<RecentWorkspace>> {
        let remote_connections = self.remote_connections()?;
        let mut result = Vec::new();
        for (id, paths, identity_paths_hint, remote_connection_id, _session_id, timestamp) in
            self.recent_workspaces()?
        {
            if let Some(remote_connection_id) = remote_connection_id {
                if let Some(connection_options) = remote_connections.get(&remote_connection_id) {
                    result.push(RecentWorkspace {
                        workspace_id: id,
                        location: SerializedWorkspaceLocation::Remote(connection_options.clone()),
                        paths: paths.clone(),
                        identity_paths: identity_paths_hint.unwrap_or(paths),
                        timestamp,
                    });
                }
                continue;
            }

            if paths.paths().is_empty() || contains_wsl_path(&paths) {
                continue;
            }

            if Self::all_paths_exist_with_a_directory(paths.paths(), fs).await {
                let identity_paths = resolve_local_workspace_identity(fs, &paths)
                    .await
                    .or(identity_paths_hint)
                    .unwrap_or_else(|| paths.clone());
                result.push(RecentWorkspace {
                    workspace_id: id,
                    location: SerializedWorkspaceLocation::Local,
                    paths,
                    identity_paths,
                    timestamp,
                });
            }
        }

        Ok(result)
    }

    // Returns the recent project workspaces suitable for recent-project UIs.
    // Entries are deduplicated by git worktree identity, but preserve the original
    // serialized paths for reopening.
    pub async fn recent_project_workspaces(&self, fs: &dyn Fs) -> Result<Vec<RecentWorkspace>> {
        Ok(dedupe_recent_workspaces(
            self.recent_project_workspaces_ungrouped(fs).await?,
        ))
    }

    pub async fn recent_workspace_group_ids(
        &self,
        target: &RecentWorkspace,
    ) -> Result<Vec<WorkspaceId>> {
        let target_paths = &target.identity_paths;
        let target_remote_connection = match &target.location {
            SerializedWorkspaceLocation::Local => None,
            SerializedWorkspaceLocation::Remote(connection) => {
                Some(remote_connection_identity(connection))
            }
        };

        let remote_connections = self.remote_connections()?;

        let mut workspace_ids = Vec::new();
        for (workspace_id, paths, identity_paths, remote_connection_id, _, _) in
            self.recent_workspaces()?
        {
            let remote_connection = if let Some(id) = remote_connection_id {
                let Some(connection_options) = remote_connections.get(&id) else {
                    continue;
                };
                Some(remote_connection_identity(connection_options))
            } else {
                None
            };
            if remote_connection == target_remote_connection
                && &identity_paths.unwrap_or(paths) == target_paths
            {
                workspace_ids.push(workspace_id);
            }
        }
        Ok(workspace_ids)
    }

    #[cfg(test)]
    async fn delete_recent_workspace_group(
        &self,
        target: &RecentWorkspace,
        protected_workspace_ids: &HashSet<WorkspaceId>,
    ) -> Result<Vec<WorkspaceId>> {
        let workspace_ids = self
            .recent_workspace_group_ids(target)
            .await?
            .into_iter()
            .filter(|id| !protected_workspace_ids.contains(id))
            .collect::<Vec<_>>();
        for workspace_id in &workspace_ids {
            self.delete_workspace_by_id(*workspace_id).await?;
        }
        Ok(workspace_ids)
    }

    // Deletes workspace rows that can no longer be restored from. Remote workspaces whose
    // connection was removed, and (on Windows) workspaces pointing at WSL paths, are cleaned
    // up immediately. Local workspaces with no valid paths on disk are kept for seven days
    // after going stale. Workspaces belonging to the current session or the last session are
    // always preserved so that an in-progress restore can rehydrate them.
    pub async fn garbage_collect_workspace_candidates(
        &self,
        fs: &dyn Fs,
        current_session_id: &str,
        last_session_id: Option<&str>,
        protected_workspace_ids: &HashSet<WorkspaceId>,
    ) -> Result<Vec<WorkspaceId>> {
        let remote_connections = self.remote_connections()?;
        let now = Utc::now();
        let mut workspaces_to_delete = Vec::new();
        for (id, paths, _identity_paths_hint, remote_connection_id, session_id, timestamp) in
            self.recent_workspaces()?
        {
            if protected_workspace_ids.contains(&id) {
                continue;
            }
            if let Some(session_id) = session_id.as_deref() {
                if session_id == current_session_id || Some(session_id) == last_session_id {
                    continue;
                }
            }

            if let Some(remote_connection_id) = remote_connection_id {
                if !remote_connections.contains_key(&remote_connection_id) {
                    workspaces_to_delete.push(id);
                }
                continue;
            }

            // Delete the workspace if any of the paths are WSL paths. If a
            // local workspace points to WSL, attempting to read its metadata
            // will wait for the WSL VM and file server to boot up. This can
            // block for many seconds. Supported scenarios use remote
            // workspaces.
            if contains_wsl_path(&paths) {
                workspaces_to_delete.push(id);
                continue;
            }

            if !Self::all_paths_exist_with_a_directory(paths.paths(), fs).await
                && now - timestamp >= chrono::Duration::days(7)
            {
                workspaces_to_delete.push(id);
            }
        }

        Ok(workspaces_to_delete)
    }

    #[cfg(test)]
    async fn garbage_collect_workspaces(
        &self,
        fs: &dyn Fs,
        current_session_id: &str,
        last_session_id: Option<&str>,
        protected_workspace_ids: &HashSet<WorkspaceId>,
    ) -> Result<()> {
        let workspace_ids = self
            .garbage_collect_workspace_candidates(
                fs,
                current_session_id,
                last_session_id,
                protected_workspace_ids,
            )
            .await?;
        for workspace_id in workspace_ids {
            self.delete_workspace_by_id(workspace_id).await?;
        }
        Ok(())
    }

    pub async fn last_workspace(&self, fs: &dyn Fs) -> Result<Option<RecentWorkspace>> {
        Ok(self.recent_project_workspaces(fs).await?.into_iter().next())
    }

    // Returns the locations of the workspaces that were still opened when the last
    // session was closed (i.e. when Zed was quit).
    // If `last_session_window_order` is provided, the returned locations are ordered
    // according to that.
    pub async fn last_session_workspace_locations(
        &self,
        last_session_id: &str,
        last_session_window_stack: Option<Vec<WindowId>>,
        fs: &dyn Fs,
    ) -> Result<Vec<SessionWorkspace>> {
        let mut workspaces = Vec::new();

        for (workspace_id, paths, window_id, remote_connection_id) in
            self.session_workspaces(last_session_id.to_owned())?
        {
            let window_id = window_id.map(WindowId::from);

            if let Some(remote_connection_id) = remote_connection_id {
                workspaces.push(SessionWorkspace {
                    workspace_id,
                    location: SerializedWorkspaceLocation::Remote(
                        self.remote_connection(remote_connection_id)?,
                    ),
                    paths,
                    window_id,
                });
                continue;
            }

            if paths.is_empty() || Self::all_paths_exist_with_a_directory(paths.paths(), fs).await {
                workspaces.push(SessionWorkspace {
                    workspace_id,
                    location: SerializedWorkspaceLocation::Local,
                    paths,
                    window_id,
                });
            }
        }

        if let Some(stack) = last_session_window_stack {
            workspaces.sort_by_key(|workspace| {
                workspace
                    .window_id
                    .and_then(|id| stack.iter().position(|&order_id| order_id == id))
                    .unwrap_or(usize::MAX)
            });
        }

        Ok(workspaces)
    }

    fn get_center_pane_group(&self, workspace_id: WorkspaceId) -> Result<SerializedPaneGroup> {
        Ok(self
            .get_pane_group(workspace_id, None)?
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                SerializedPaneGroup::Pane(SerializedPane {
                    active: true,
                    children: vec![],
                    pinned_count: 0,
                })
            }))
    }

    fn get_pane_group(
        &self,
        workspace_id: WorkspaceId,
        group_id: Option<GroupId>,
    ) -> Result<Vec<SerializedPaneGroup>> {
        type GroupKey = (Option<GroupId>, WorkspaceId);
        type GroupOrPane = (
            Option<GroupId>,
            Option<SerializedAxis>,
            Option<PaneId>,
            Option<bool>,
            Option<usize>,
            Option<String>,
        );
        self.select_bound::<GroupKey, GroupOrPane>(sql!(
            SELECT group_id, axis, pane_id, active, pinned_count, flexes
                FROM (SELECT
                        group_id,
                        axis,
                        NULL as pane_id,
                        NULL as active,
                        NULL as pinned_count,
                        position,
                        parent_group_id,
                        workspace_id,
                        flexes
                      FROM pane_groups
                    UNION
                      SELECT
                        NULL,
                        NULL,
                        center_panes.pane_id,
                        panes.active as active,
                        pinned_count,
                        position,
                        parent_group_id,
                        panes.workspace_id as workspace_id,
                        NULL
                      FROM center_panes
                      JOIN panes ON center_panes.pane_id = panes.pane_id)
                WHERE parent_group_id IS ? AND workspace_id = ?
                ORDER BY position
        ))?((group_id, workspace_id))?
        .into_iter()
        .map(|(group_id, axis, pane_id, active, pinned_count, flexes)| {
            let maybe_pane = maybe!({ Some((pane_id?, active?, pinned_count?)) });
            if let Some((group_id, axis)) = group_id.zip(axis) {
                let flexes = flexes
                    .map(|flexes: String| serde_json::from_str::<Vec<f32>>(&flexes))
                    .transpose()?;

                Ok(SerializedPaneGroup::Group {
                    axis,
                    children: self.get_pane_group(workspace_id, Some(group_id))?,
                    flexes,
                })
            } else if let Some((pane_id, active, pinned_count)) = maybe_pane {
                Ok(SerializedPaneGroup::Pane(SerializedPane::new(
                    self.get_items(pane_id)?,
                    active,
                    pinned_count,
                )))
            } else {
                bail!("Pane Group Child was neither a pane group or a pane");
            }
        })
        // Filter out panes and pane groups which don't have any children or items
        .filter(|pane_group| match pane_group {
            Ok(SerializedPaneGroup::Group { children, .. }) => !children.is_empty(),
            Ok(SerializedPaneGroup::Pane(pane)) => !pane.children.is_empty(),
            _ => true,
        })
        .collect::<Result<_>>()
    }

    fn save_pane_group(
        conn: &Connection,
        workspace_id: WorkspaceId,
        pane_group: &SerializedPaneGroup,
        parent: Option<(GroupId, usize)>,
    ) -> Result<()> {
        if parent.is_none() {
            log::debug!("Saving a pane group for workspace {workspace_id:?}");
        }
        match pane_group {
            SerializedPaneGroup::Group {
                axis,
                children,
                flexes,
            } => {
                let (parent_id, position) = parent.unzip();

                let flex_string = flexes
                    .as_ref()
                    .map(|flexes| serde_json::json!(flexes).to_string());

                let group_id = conn.select_row_bound::<_, i64>(sql!(
                    INSERT INTO pane_groups(
                        workspace_id,
                        parent_group_id,
                        position,
                        axis,
                        flexes
                    )
                    VALUES (?, ?, ?, ?, ?)
                    RETURNING group_id
                ))?((
                    workspace_id,
                    parent_id,
                    position,
                    *axis,
                    flex_string,
                ))?
                .context("Couldn't retrieve group_id from inserted pane_group")?;

                for (position, group) in children.iter().enumerate() {
                    Self::save_pane_group(conn, workspace_id, group, Some((group_id, position)))?
                }

                Ok(())
            }
            SerializedPaneGroup::Pane(pane) => {
                Self::save_pane(conn, workspace_id, pane, parent)?;
                Ok(())
            }
        }
    }

    fn save_pane(
        conn: &Connection,
        workspace_id: WorkspaceId,
        pane: &SerializedPane,
        parent: Option<(GroupId, usize)>,
    ) -> Result<PaneId> {
        let pane_id = conn.select_row_bound::<_, i64>(sql!(
            INSERT INTO panes(workspace_id, active, pinned_count)
            VALUES (?, ?, ?)
            RETURNING pane_id
        ))?((workspace_id, pane.active, pane.pinned_count))?
        .context("Could not retrieve inserted pane_id")?;

        let (parent_id, order) = parent.unzip();
        conn.exec_bound(sql!(
            INSERT INTO center_panes(pane_id, parent_group_id, position)
            VALUES (?, ?, ?)
        ))?((pane_id, parent_id, order))?;

        Self::save_items(conn, workspace_id, pane_id, &pane.children).context("Saving items")?;

        Ok(pane_id)
    }

    fn get_items(&self, pane_id: PaneId) -> Result<Vec<SerializedItem>> {
        self.select_bound(sql!(
            SELECT kind, item_id, active, preview FROM items
            WHERE pane_id = ?
                ORDER BY position
        ))?(pane_id)
    }

    fn save_items(
        conn: &Connection,
        workspace_id: WorkspaceId,
        pane_id: PaneId,
        items: &[SerializedItem],
    ) -> Result<()> {
        let mut insert = conn.exec_bound(sql!(
            INSERT INTO items(workspace_id, pane_id, position, kind, item_id, active, preview) VALUES (?, ?, ?, ?, ?, ?, ?)
        )).context("Preparing insertion")?;
        for (position, item) in items.iter().enumerate() {
            insert((workspace_id, pane_id, position, item))?;
        }

        Ok(())
    }

    query! {
        pub async fn update_timestamp(workspace_id: WorkspaceId) -> Result<()> {
            UPDATE workspaces
            SET timestamp = CURRENT_TIMESTAMP
            WHERE workspace_id = ?
        }
    }

    #[cfg(test)]
    query! {
        pub(crate) async fn set_timestamp_for_tests(workspace_id: WorkspaceId, timestamp: String) -> Result<()> {
            UPDATE workspaces
            SET timestamp = ?2
            WHERE workspace_id = ?1
        }
    }

    query! {
        pub(crate) async fn set_window_open_status(workspace_id: WorkspaceId, bounds: SerializedWindowBounds, display: Uuid) -> Result<()> {
            UPDATE workspaces
            SET window_state = ?2,
                window_x = ?3,
                window_y = ?4,
                window_width = ?5,
                window_height = ?6,
                display = ?7
            WHERE workspace_id = ?1
        }
    }

    query! {
        pub(crate) async fn set_centered_layout(workspace_id: WorkspaceId, centered_layout: bool) -> Result<()> {
            UPDATE workspaces
            SET centered_layout = ?2
            WHERE workspace_id = ?1
        }
    }

    query! {
        pub(crate) async fn set_session_id(workspace_id: WorkspaceId, session_id: Option<String>) -> Result<()> {
            UPDATE workspaces
            SET session_id = ?2
            WHERE workspace_id = ?1
        }
    }

    query! {
        pub(crate) async fn set_session_binding(workspace_id: WorkspaceId, session_id: Option<String>, window_id: Option<u64>) -> Result<()> {
            UPDATE workspaces
            SET session_id = ?2, window_id = ?3
            WHERE workspace_id = ?1
        }
    }

    pub(crate) async fn toolchains(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<(Toolchain, Arc<Path>, Arc<RelPath>)>> {
        self.write(move |this| {
            let mut select = this
                .select_bound(sql!(
                    SELECT
                        name, path, worktree_root_path, relative_worktree_path, language_name, raw_json
                    FROM toolchains
                    WHERE workspace_id = ?
                ))
                .context("select toolchains")?;

            let toolchain: Vec<(String, String, String, String, String, String)> =
                select(workspace_id)?;

            Ok(toolchain
                .into_iter()
                .filter_map(
                    |(name, path, worktree_root_path, relative_worktree_path, language, json)| {
                        Some((
                            Toolchain {
                                name: name.into(),
                                path: path.into(),
                                language_name: LanguageName::new(&language),
                                as_json: serde_json::Value::from_str(&json).ok()?,
                            },
                           Arc::from(worktree_root_path.as_ref()),
                            RelPath::from_unix_str(&relative_worktree_path).log_err()?.into(),
                        ))
                    },
                )
                .collect())
        })
        .await
    }

    pub async fn set_toolchain(
        &self,
        workspace_id: WorkspaceId,
        worktree_root_path: Arc<Path>,
        relative_worktree_path: Arc<RelPath>,
        toolchain: Toolchain,
    ) -> Result<()> {
        log::debug!(
            "Setting toolchain for workspace, worktree: {worktree_root_path:?}, relative path: {relative_worktree_path:?}, toolchain: {}",
            toolchain.name
        );
        self.write(move |conn| {
            let mut insert = conn
                .exec_bound(sql!(
                    INSERT INTO toolchains(workspace_id, worktree_root_path, relative_worktree_path, language_name, name, path, raw_json) VALUES (?, ?, ?, ?, ?,  ?, ?)
                    ON CONFLICT DO
                    UPDATE SET
                        name = ?5,
                        path = ?6,
                        raw_json = ?7
                ))
                .context("Preparing insertion")?;

            insert((
                workspace_id,
                worktree_root_path.to_string_lossy().into_owned(),
                relative_worktree_path.as_unix_str(),
                toolchain.language_name.as_ref(),
                toolchain.name.as_ref(),
                toolchain.path.as_ref(),
                toolchain.as_json.to_string(),
            ))?;

            Ok(())
        }).await
    }

    pub(crate) async fn save_trusted_worktrees(
        &self,
        trusted_worktrees: HashMap<Option<RemoteHostLocation>, HashSet<PathBuf>>,
    ) -> anyhow::Result<()> {
        use anyhow::Context as _;
        use db::sqlez::statement::Statement;
        use itertools::Itertools as _;

        self.clear_trusted_worktrees()
            .await
            .context("clearing previous trust state")?;

        let trusted_worktrees = trusted_worktrees
            .into_iter()
            .flat_map(|(host, abs_paths)| {
                abs_paths
                    .into_iter()
                    .map(move |abs_path| (Some(abs_path), host.clone()))
            })
            .collect::<Vec<_>>();
        let mut first_worktree;
        let mut last_worktree = 0_usize;
        for (count, placeholders) in std::iter::once("(?, ?, ?)")
            .cycle()
            .take(trusted_worktrees.len())
            .chunks(MAX_QUERY_PLACEHOLDERS / 3)
            .into_iter()
            .map(|chunk| {
                let mut count = 0;
                let placeholders = chunk
                    .inspect(|_| {
                        count += 1;
                    })
                    .join(", ");
                (count, placeholders)
            })
            .collect::<Vec<_>>()
        {
            first_worktree = last_worktree;
            last_worktree = last_worktree + count;
            let query = format!(
                r#"INSERT INTO trusted_worktrees(absolute_path, user_name, host_name)
VALUES {placeholders};"#
            );

            let trusted_worktrees = trusted_worktrees[first_worktree..last_worktree].to_vec();
            self.write(move |conn| {
                let mut statement = Statement::prepare(conn, query)?;
                let mut next_index = 1;
                for (abs_path, host) in trusted_worktrees {
                    let abs_path = abs_path.as_ref().map(|abs_path| abs_path.to_string_lossy());
                    next_index = statement.bind(
                        &abs_path.as_ref().map(|abs_path| abs_path.as_ref()),
                        next_index,
                    )?;
                    next_index = statement.bind(
                        &host
                            .as_ref()
                            .and_then(|host| Some(host.user_name.as_ref()?.as_str())),
                        next_index,
                    )?;
                    next_index = statement.bind(
                        &host.as_ref().map(|host| host.host_identifier.as_str()),
                        next_index,
                    )?;
                }
                statement.exec()
            })
            .await
            .context("inserting new trusted state")?;
        }
        Ok(())
    }

    pub fn fetch_trusted_worktrees(&self) -> Result<DbTrustedPaths> {
        let trusted_worktrees = self.trusted_worktrees()?;
        Ok(trusted_worktrees
            .into_iter()
            .filter_map(|(abs_path, user_name, host_name)| {
                let db_host = match (user_name, host_name) {
                    (None, Some(host_name)) => Some(RemoteHostLocation {
                        user_name: None,
                        host_identifier: SharedString::new(host_name),
                    }),
                    (Some(user_name), Some(host_name)) => Some(RemoteHostLocation {
                        user_name: Some(SharedString::new(user_name)),
                        host_identifier: SharedString::new(host_name),
                    }),
                    _ => None,
                };
                Some((db_host, abs_path?))
            })
            .fold(HashMap::default(), |mut acc, (remote_host, abs_path)| {
                acc.entry(remote_host)
                    .or_insert_with(HashSet::default)
                    .insert(abs_path);
                acc
            }))
    }

    query! {
        fn trusted_worktrees() -> Result<Vec<(Option<PathBuf>, Option<String>, Option<String>)>> {
            SELECT absolute_path, user_name, host_name
            FROM trusted_worktrees
        }
    }

    query! {
        pub async fn clear_trusted_worktrees() -> Result<()> {
            DELETE FROM trusted_worktrees
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecentWorkspace {
    pub workspace_id: WorkspaceId,
    pub location: SerializedWorkspaceLocation,
    pub paths: PathList,
    pub identity_paths: PathList,
    pub timestamp: DateTime<Utc>,
}

impl RecentWorkspace {
    pub fn project_group_key(&self) -> ProjectGroupKey {
        let host = match &self.location {
            SerializedWorkspaceLocation::Local => None,
            SerializedWorkspaceLocation::Remote(options) => Some(options.clone()),
        };
        ProjectGroupKey::new(host, self.identity_paths.clone())
    }
}

async fn resolve_local_workspace_identity(fs: &dyn Fs, paths: &PathList) -> Option<PathList> {
    let raw_paths = paths.paths();
    let resolved_paths = futures::future::join_all(
        raw_paths
            .iter()
            .map(|path| project::git_store::resolve_git_worktree_to_main_repo(fs, path)),
    )
    .await;

    if resolved_paths.iter().all(|resolved| resolved.is_none()) {
        return None;
    }

    let resolved_paths: Vec<PathBuf> = raw_paths
        .iter()
        .zip(resolved_paths.iter())
        .map(|(original, resolved)| {
            resolved
                .as_ref()
                .cloned()
                .unwrap_or_else(|| original.clone())
        })
        .collect();
    let resolved_path_refs: Vec<&Path> = resolved_paths.iter().map(PathBuf::as_path).collect();
    Some(PathList::new(&resolved_path_refs))
}

fn dedupe_recent_workspaces(
    workspaces: impl IntoIterator<Item = RecentWorkspace>,
) -> Vec<RecentWorkspace> {
    let mut indices_by_key: HashMap<(Option<RemoteConnectionIdentity>, Vec<PathBuf>), usize> =
        HashMap::default();
    let mut result: Vec<RecentWorkspace> = Vec::new();
    for workspace in workspaces {
        let location_identity = match &workspace.location {
            SerializedWorkspaceLocation::Local => None,
            SerializedWorkspaceLocation::Remote(connection) => {
                Some(remote_connection_identity(connection))
            }
        };
        let key = (location_identity, workspace.identity_paths.paths().to_vec());
        if let Some(&existing_index) = indices_by_key.get(&key) {
            if workspace.timestamp > result[existing_index].timestamp {
                result[existing_index] = workspace;
            }
        } else {
            indices_by_key.insert(key, result.len());
            result.push(workspace);
        }
    }

    result
}

pub fn delete_unloaded_items(
    alive_items: Vec<ItemId>,
    workspace_id: WorkspaceId,
    table: &'static str,
    db: &ThreadSafeConnection,
    cx: &mut App,
) -> Task<Result<()>> {
    let db = db.clone();
    cx.spawn(async move |_| {
        let placeholders = alive_items
            .iter()
            .map(|_| "?")
            .collect::<Vec<&str>>()
            .join(", ");

        let query = format!(
            "DELETE FROM {table} WHERE workspace_id = ? AND item_id NOT IN ({placeholders})"
        );

        db.write(move |conn| {
            let mut statement = Statement::prepare(conn, query)?;
            let mut next_index = statement.bind(&workspace_id, 1)?;
            for id in alive_items {
                next_index = statement.bind(&id, next_index)?;
            }
            statement.exec()
        })
        .await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathList;
    use crate::ProjectGroupKey;
    use crate::RemovalIntent;
    use crate::{
        multi_workspace::MultiWorkspace,
        persistence::{
            model::{
                SerializedItem, SerializedPane, SerializedPaneGroup, SerializedWorkspace,
                SessionWorkspace,
            },
            read_multi_workspace_state,
        },
    };
    use gpui::TaskExt;

    use pretty_assertions::assert_eq;
    use project::Project;
    use remote::SshConnectionOptions;
    use serde_json::json;
    use std::{thread, time::Duration};

    /// Creates a unique directory in a FakeFs, returning the path.
    /// Uses a UUID suffix to avoid collisions with other tests sharing the global DB.
    async fn unique_test_dir(fs: &fs::FakeFs, prefix: &str) -> PathBuf {
        let dir = PathBuf::from(format!("/test-dirs/{}-{}", prefix, uuid::Uuid::new_v4()));
        fs.insert_tree(&dir, json!({})).await;
        dir
    }

    #[gpui::test]
    async fn test_multi_workspace_serializes_on_add_and_remove(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let project1 = Project::test(fs.clone(), [], cx).await;
        let project2 = Project::test(fs.clone(), [], cx).await;

        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project1.clone(), window, cx));

        multi_workspace.update(cx, |mw, cx| {
            mw.open_sidebar(cx);
        });

        multi_workspace.update_in(cx, |mw, _, cx| {
            mw.set_random_database_id(cx);
        });

        let window_id =
            multi_workspace.update_in(cx, |_, window, _cx| window.window_handle().window_id());

        // --- Add a second workspace ---
        let workspace2 = multi_workspace.update_in(cx, |mw, window, cx| {
            let workspace = cx.new(|cx| crate::Workspace::test_new(project2.clone(), window, cx));
            workspace.update(cx, |ws, _cx| ws.set_random_database_id());
            mw.activate(workspace.clone(), None, window, cx);
            workspace
        });

        // Run background tasks so serialize has a chance to flush.
        cx.run_until_parked();

        // Read back the persisted state and check that the active workspace ID was written.
        let state_after_add = cx.update(|_, cx| read_multi_workspace_state(window_id, cx));
        let active_workspace2_db_id = workspace2.read_with(cx, |ws, _| ws.database_id());
        assert_eq!(
            state_after_add.active_workspace_id, active_workspace2_db_id,
            "After adding a second workspace, the serialized active_workspace_id should match \
             the newly activated workspace's database id"
        );

        // --- Remove the non-active workspace ---
        multi_workspace.update_in(cx, |mw, _window, cx| {
            let active = mw.workspace().clone();
            let ws = mw
                .workspaces()
                .find(|ws| *ws != &active)
                .expect("should have a non-active workspace");
            mw.remove([ws.clone()], RemovalIntent::CloseProject, _window, cx)
                .detach_and_log_err(cx);
        });

        cx.run_until_parked();

        let state_after_remove = cx.update(|_, cx| read_multi_workspace_state(window_id, cx));
        let remaining_db_id =
            multi_workspace.read_with(cx, |mw, cx| mw.workspace().read(cx).database_id());
        assert_eq!(
            state_after_remove.active_workspace_id, remaining_db_id,
            "After removing a workspace, the serialized active_workspace_id should match \
             the remaining active workspace's database id"
        );
    }

    #[gpui::test]
    async fn test_breakpoints() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("test_breakpoints").await;
        let id = db.next_id().await.unwrap();

        let path = Path::new("/tmp/test.rs");

        let breakpoint = Breakpoint {
            position: 123,
            message: None,
            state: BreakpointState::Enabled,
            condition: None,
            hit_condition: None,
        };

        let log_breakpoint = Breakpoint {
            position: 456,
            message: Some("Test log message".into()),
            state: BreakpointState::Enabled,
            condition: None,
            hit_condition: None,
        };

        let disable_breakpoint = Breakpoint {
            position: 578,
            message: None,
            state: BreakpointState::Disabled,
            condition: None,
            hit_condition: None,
        };

        let condition_breakpoint = Breakpoint {
            position: 789,
            message: None,
            state: BreakpointState::Enabled,
            condition: Some("x > 5".into()),
            hit_condition: None,
        };

        let hit_condition_breakpoint = Breakpoint {
            position: 999,
            message: None,
            state: BreakpointState::Enabled,
            condition: None,
            hit_condition: Some(">= 3".into()),
        };

        let workspace = SerializedWorkspace {
            id,
            paths: PathList::new(&["/tmp"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: {
                let mut map = collections::BTreeMap::default();
                map.insert(
                    Arc::from(path),
                    vec![
                        SourceBreakpoint {
                            row: breakpoint.position,
                            path: Arc::from(path),
                            message: breakpoint.message.clone(),
                            state: breakpoint.state,
                            condition: breakpoint.condition.clone(),
                            hit_condition: breakpoint.hit_condition.clone(),
                        },
                        SourceBreakpoint {
                            row: log_breakpoint.position,
                            path: Arc::from(path),
                            message: log_breakpoint.message.clone(),
                            state: log_breakpoint.state,
                            condition: log_breakpoint.condition.clone(),
                            hit_condition: log_breakpoint.hit_condition.clone(),
                        },
                        SourceBreakpoint {
                            row: disable_breakpoint.position,
                            path: Arc::from(path),
                            message: disable_breakpoint.message.clone(),
                            state: disable_breakpoint.state,
                            condition: disable_breakpoint.condition.clone(),
                            hit_condition: disable_breakpoint.hit_condition.clone(),
                        },
                        SourceBreakpoint {
                            row: condition_breakpoint.position,
                            path: Arc::from(path),
                            message: condition_breakpoint.message.clone(),
                            state: condition_breakpoint.state,
                            condition: condition_breakpoint.condition.clone(),
                            hit_condition: condition_breakpoint.hit_condition.clone(),
                        },
                        SourceBreakpoint {
                            row: hit_condition_breakpoint.position,
                            path: Arc::from(path),
                            message: hit_condition_breakpoint.message.clone(),
                            state: hit_condition_breakpoint.state,
                            condition: hit_condition_breakpoint.condition.clone(),
                            hit_condition: hit_condition_breakpoint.hit_condition.clone(),
                        },
                    ],
                );
                map
            },
            session_id: None,
            window_id: None,
            user_toolchains: Default::default(),
        };

        db.save_workspace(workspace.clone()).await;

        let loaded = db.workspace_for_roots(&["/tmp"]).unwrap();
        let loaded_breakpoints = loaded.breakpoints.get(&Arc::from(path)).unwrap();

        assert_eq!(loaded_breakpoints.len(), 5);

        // normal breakpoint
        assert_eq!(loaded_breakpoints[0].row, breakpoint.position);
        assert_eq!(loaded_breakpoints[0].message, breakpoint.message);
        assert_eq!(loaded_breakpoints[0].condition, breakpoint.condition);
        assert_eq!(
            loaded_breakpoints[0].hit_condition,
            breakpoint.hit_condition
        );
        assert_eq!(loaded_breakpoints[0].state, breakpoint.state);
        assert_eq!(loaded_breakpoints[0].path, Arc::from(path));

        // enabled breakpoint
        assert_eq!(loaded_breakpoints[1].row, log_breakpoint.position);
        assert_eq!(loaded_breakpoints[1].message, log_breakpoint.message);
        assert_eq!(loaded_breakpoints[1].condition, log_breakpoint.condition);
        assert_eq!(
            loaded_breakpoints[1].hit_condition,
            log_breakpoint.hit_condition
        );
        assert_eq!(loaded_breakpoints[1].state, log_breakpoint.state);
        assert_eq!(loaded_breakpoints[1].path, Arc::from(path));

        // disable breakpoint
        assert_eq!(loaded_breakpoints[2].row, disable_breakpoint.position);
        assert_eq!(loaded_breakpoints[2].message, disable_breakpoint.message);
        assert_eq!(
            loaded_breakpoints[2].condition,
            disable_breakpoint.condition
        );
        assert_eq!(
            loaded_breakpoints[2].hit_condition,
            disable_breakpoint.hit_condition
        );
        assert_eq!(loaded_breakpoints[2].state, disable_breakpoint.state);
        assert_eq!(loaded_breakpoints[2].path, Arc::from(path));

        // condition breakpoint
        assert_eq!(loaded_breakpoints[3].row, condition_breakpoint.position);
        assert_eq!(loaded_breakpoints[3].message, condition_breakpoint.message);
        assert_eq!(
            loaded_breakpoints[3].condition,
            condition_breakpoint.condition
        );
        assert_eq!(
            loaded_breakpoints[3].hit_condition,
            condition_breakpoint.hit_condition
        );
        assert_eq!(loaded_breakpoints[3].state, condition_breakpoint.state);
        assert_eq!(loaded_breakpoints[3].path, Arc::from(path));

        // hit condition breakpoint
        assert_eq!(loaded_breakpoints[4].row, hit_condition_breakpoint.position);
        assert_eq!(
            loaded_breakpoints[4].message,
            hit_condition_breakpoint.message
        );
        assert_eq!(
            loaded_breakpoints[4].condition,
            hit_condition_breakpoint.condition
        );
        assert_eq!(
            loaded_breakpoints[4].hit_condition,
            hit_condition_breakpoint.hit_condition
        );
        assert_eq!(loaded_breakpoints[4].state, hit_condition_breakpoint.state);
        assert_eq!(loaded_breakpoints[4].path, Arc::from(path));
    }

    #[gpui::test]
    async fn test_remove_last_breakpoint() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("test_remove_last_breakpoint").await;
        let id = db.next_id().await.unwrap();

        let singular_path = Path::new("/tmp/test_remove_last_breakpoint.rs");

        let breakpoint_to_remove = Breakpoint {
            position: 100,
            message: None,
            state: BreakpointState::Enabled,
            condition: None,
            hit_condition: None,
        };

        let workspace = SerializedWorkspace {
            id,
            paths: PathList::new(&["/tmp"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: {
                let mut map = collections::BTreeMap::default();
                map.insert(
                    Arc::from(singular_path),
                    vec![SourceBreakpoint {
                        row: breakpoint_to_remove.position,
                        path: Arc::from(singular_path),
                        message: None,
                        state: BreakpointState::Enabled,
                        condition: None,
                        hit_condition: None,
                    }],
                );
                map
            },
            session_id: None,
            window_id: None,
            user_toolchains: Default::default(),
        };

        db.save_workspace(workspace.clone()).await;

        let loaded = db.workspace_for_roots(&["/tmp"]).unwrap();
        let loaded_breakpoints = loaded.breakpoints.get(&Arc::from(singular_path)).unwrap();

        assert_eq!(loaded_breakpoints.len(), 1);
        assert_eq!(loaded_breakpoints[0].row, breakpoint_to_remove.position);
        assert_eq!(loaded_breakpoints[0].message, breakpoint_to_remove.message);
        assert_eq!(
            loaded_breakpoints[0].condition,
            breakpoint_to_remove.condition
        );
        assert_eq!(
            loaded_breakpoints[0].hit_condition,
            breakpoint_to_remove.hit_condition
        );
        assert_eq!(loaded_breakpoints[0].state, breakpoint_to_remove.state);
        assert_eq!(loaded_breakpoints[0].path, Arc::from(singular_path));

        let workspace_without_breakpoint = SerializedWorkspace {
            id,
            paths: PathList::new(&["/tmp"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: collections::BTreeMap::default(),
            session_id: None,
            window_id: None,
            user_toolchains: Default::default(),
        };

        db.save_workspace(workspace_without_breakpoint.clone())
            .await;

        let loaded_after_remove = db.workspace_for_roots(&["/tmp"]).unwrap();
        let empty_breakpoints = loaded_after_remove
            .breakpoints
            .get(&Arc::from(singular_path));

        assert!(empty_breakpoints.is_none());
    }

    #[gpui::test]
    async fn test_next_id_stability() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("test_next_id_stability").await;

        db.write(|conn| {
            conn.migrate(
                "test_table",
                &[sql!(
                    CREATE TABLE test_table(
                        text TEXT,
                        workspace_id INTEGER,
                        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                        ON DELETE CASCADE
                    ) STRICT;
                )],
                &mut |_, _, _| false,
            )
            .unwrap();
        })
        .await;

        let id = db.next_id().await.unwrap();
        // Assert the empty row got inserted
        assert_eq!(
            Some(id),
            db.select_row_bound::<WorkspaceId, WorkspaceId>(sql!(
                SELECT workspace_id FROM workspaces WHERE workspace_id = ?
            ))
            .unwrap()(id)
            .unwrap()
        );

        db.write(move |conn| {
            conn.exec_bound(sql!(INSERT INTO test_table(text, workspace_id) VALUES (?, ?)))
                .unwrap()(("test-text-1", id))
            .unwrap()
        })
        .await;

        let test_text_1 = db
            .select_row_bound::<_, String>(sql!(SELECT text FROM test_table WHERE workspace_id = ?))
            .unwrap()(1)
        .unwrap()
        .unwrap();
        assert_eq!(test_text_1, "test-text-1");
    }

    #[gpui::test]
    async fn test_workspace_id_stability() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("test_workspace_id_stability").await;

        db.write(|conn| {
            conn.migrate(
                "test_table",
                &[sql!(
                        CREATE TABLE test_table(
                            text TEXT,
                            workspace_id INTEGER,
                            FOREIGN KEY(workspace_id)
                                REFERENCES workspaces(workspace_id)
                            ON DELETE CASCADE
                        ) STRICT;)],
                &mut |_, _, _| false,
            )
        })
        .await
        .unwrap();

        let mut workspace_1 = SerializedWorkspace {
            id: WorkspaceId(1),
            paths: PathList::new(&["/tmp", "/tmp2"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            session_id: None,
            window_id: None,
            user_toolchains: Default::default(),
        };

        let workspace_2 = SerializedWorkspace {
            id: WorkspaceId(2),
            paths: PathList::new(&["/tmp"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            session_id: None,
            window_id: None,
            user_toolchains: Default::default(),
        };

        db.save_workspace(workspace_1.clone()).await;

        db.write(|conn| {
            conn.exec_bound(sql!(INSERT INTO test_table(text, workspace_id) VALUES (?, ?)))
                .unwrap()(("test-text-1", 1))
            .unwrap();
        })
        .await;

        db.save_workspace(workspace_2.clone()).await;

        db.write(|conn| {
            conn.exec_bound(sql!(INSERT INTO test_table(text, workspace_id) VALUES (?, ?)))
                .unwrap()(("test-text-2", 2))
            .unwrap();
        })
        .await;

        workspace_1.paths = PathList::new(&["/tmp", "/tmp3"]);
        db.save_workspace(workspace_1.clone()).await;
        db.save_workspace(workspace_1).await;
        db.save_workspace(workspace_2).await;

        let test_text_2 = db
            .select_row_bound::<_, String>(sql!(SELECT text FROM test_table WHERE workspace_id = ?))
            .unwrap()(2)
        .unwrap()
        .unwrap();
        assert_eq!(test_text_2, "test-text-2");

        let test_text_1 = db
            .select_row_bound::<_, String>(sql!(SELECT text FROM test_table WHERE workspace_id = ?))
            .unwrap()(1)
        .unwrap()
        .unwrap();
        assert_eq!(test_text_1, "test-text-1");
    }

    fn group(axis: Axis, children: Vec<SerializedPaneGroup>) -> SerializedPaneGroup {
        SerializedPaneGroup::Group {
            axis: SerializedAxis(axis),
            flexes: None,
            children,
        }
    }

    #[gpui::test]
    async fn test_full_workspace_serialization() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("test_full_workspace_serialization").await;

        //  -----------------
        //  | 1,2   | 5,6   |
        //  | - - - |       |
        //  | 3,4   |       |
        //  -----------------
        let center_group = group(
            Axis::Horizontal,
            vec![
                group(
                    Axis::Vertical,
                    vec![
                        SerializedPaneGroup::Pane(SerializedPane::new(
                            vec![
                                SerializedItem::new("Terminal", 5, false, false),
                                SerializedItem::new("Terminal", 6, true, false),
                            ],
                            false,
                            0,
                        )),
                        SerializedPaneGroup::Pane(SerializedPane::new(
                            vec![
                                SerializedItem::new("Terminal", 7, true, false),
                                SerializedItem::new("Terminal", 8, false, false),
                            ],
                            false,
                            0,
                        )),
                    ],
                ),
                SerializedPaneGroup::Pane(SerializedPane::new(
                    vec![
                        SerializedItem::new("Terminal", 9, false, false),
                        SerializedItem::new("Terminal", 10, true, false),
                    ],
                    false,
                    0,
                )),
            ],
        );

        let workspace = SerializedWorkspace {
            id: WorkspaceId(5),
            paths: PathList::new(&["/tmp", "/tmp2"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group,
            window_bounds: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: None,
            window_id: Some(999),
            user_toolchains: Default::default(),
        };

        db.save_workspace(workspace.clone()).await;

        let round_trip_workspace = db.workspace_for_roots(&["/tmp2", "/tmp"]);
        assert_eq!(workspace, round_trip_workspace.unwrap());

        // Test guaranteed duplicate IDs
        db.save_workspace(workspace.clone()).await;
        db.save_workspace(workspace.clone()).await;

        let round_trip_workspace = db.workspace_for_roots(&["/tmp", "/tmp2"]);
        assert_eq!(workspace, round_trip_workspace.unwrap());
    }

    #[gpui::test]
    async fn test_workspace_assignment() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("test_basic_functionality").await;

        let workspace_1 = SerializedWorkspace {
            id: WorkspaceId(1),
            paths: PathList::new(&["/tmp", "/tmp2"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: None,
            window_id: Some(1),
            user_toolchains: Default::default(),
        };

        let mut workspace_2 = SerializedWorkspace {
            id: WorkspaceId(2),
            paths: PathList::new(&["/tmp"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            session_id: None,
            window_id: Some(2),
            user_toolchains: Default::default(),
        };

        db.save_workspace(workspace_1.clone()).await;
        db.save_workspace(workspace_2.clone()).await;

        // Test that paths are treated as a set
        assert_eq!(
            db.workspace_for_roots(&["/tmp", "/tmp2"]).unwrap(),
            workspace_1
        );
        assert_eq!(
            db.workspace_for_roots(&["/tmp2", "/tmp"]).unwrap(),
            workspace_1
        );

        // Make sure that other keys work
        assert_eq!(db.workspace_for_roots(&["/tmp"]).unwrap(), workspace_2);
        assert_eq!(db.workspace_for_roots(&["/tmp3", "/tmp2", "/tmp4"]), None);

        // Test 'mutate' case of updating a pre-existing id
        workspace_2.paths = PathList::new(&["/tmp", "/tmp2"]);

        db.save_workspace(workspace_2.clone()).await;
        assert_eq!(
            db.workspace_for_roots(&["/tmp", "/tmp2"]).unwrap(),
            workspace_2
        );

        // Test other mechanism for mutating
        let mut workspace_3 = SerializedWorkspace {
            id: WorkspaceId(3),
            paths: PathList::new(&["/tmp2", "/tmp"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: None,
            window_id: Some(3),
            user_toolchains: Default::default(),
        };

        db.save_workspace(workspace_3.clone()).await;
        assert_eq!(
            db.workspace_for_roots(&["/tmp", "/tmp2"]).unwrap(),
            workspace_3
        );

        // Make sure that updating paths differently also works
        workspace_3.paths = PathList::new(&["/tmp3", "/tmp4", "/tmp2"]);
        db.save_workspace(workspace_3.clone()).await;
        assert_eq!(db.workspace_for_roots(&["/tmp2", "tmp"]), None);
        assert_eq!(
            db.workspace_for_roots(&["/tmp2", "/tmp3", "/tmp4"])
                .unwrap(),
            workspace_3
        );
    }

    #[gpui::test]
    async fn test_session_workspaces() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("test_serializing_workspaces_session_id").await;

        let workspace_1 = SerializedWorkspace {
            id: WorkspaceId(1),
            paths: PathList::new(&["/tmp1"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            session_id: Some("session-id-1".to_owned()),
            window_id: Some(10),
            user_toolchains: Default::default(),
        };

        let workspace_2 = SerializedWorkspace {
            id: WorkspaceId(2),
            paths: PathList::new(&["/tmp2"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            session_id: Some("session-id-1".to_owned()),
            window_id: Some(20),
            user_toolchains: Default::default(),
        };

        let workspace_3 = SerializedWorkspace {
            id: WorkspaceId(3),
            paths: PathList::new(&["/tmp3"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            session_id: Some("session-id-2".to_owned()),
            window_id: Some(30),
            user_toolchains: Default::default(),
        };

        let workspace_4 = SerializedWorkspace {
            id: WorkspaceId(4),
            paths: PathList::new(&["/tmp4"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            session_id: None,
            window_id: None,
            user_toolchains: Default::default(),
        };

        let connection_id = db
            .get_or_create_remote_connection(RemoteConnectionOptions::Ssh(SshConnectionOptions {
                host: "my-host".into(),
                port: Some(1234),
                ..Default::default()
            }))
            .await
            .unwrap();

        let workspace_5 = SerializedWorkspace {
            id: WorkspaceId(5),
            paths: PathList::default(),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Remote(
                db.remote_connection(connection_id).unwrap(),
            ),
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            session_id: Some("session-id-2".to_owned()),
            window_id: Some(50),
            user_toolchains: Default::default(),
        };

        let workspace_6 = SerializedWorkspace {
            id: WorkspaceId(6),
            paths: PathList::new(&["/tmp6c", "/tmp6b", "/tmp6a"]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: Some("session-id-3".to_owned()),
            window_id: Some(60),
            user_toolchains: Default::default(),
        };

        db.save_workspace(workspace_1.clone()).await;
        thread::sleep(Duration::from_millis(1000)); // Force timestamps to increment
        db.save_workspace(workspace_2.clone()).await;
        db.save_workspace(workspace_3.clone()).await;
        thread::sleep(Duration::from_millis(1000)); // Force timestamps to increment
        db.save_workspace(workspace_4.clone()).await;
        db.save_workspace(workspace_5.clone()).await;
        db.save_workspace(workspace_6.clone()).await;

        let locations = db.session_workspaces("session-id-1".to_owned()).unwrap();
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].0, WorkspaceId(2));
        assert_eq!(locations[0].1, PathList::new(&["/tmp2"]));
        assert_eq!(locations[0].2, Some(20));
        assert_eq!(locations[1].0, WorkspaceId(1));
        assert_eq!(locations[1].1, PathList::new(&["/tmp1"]));
        assert_eq!(locations[1].2, Some(10));

        let locations = db.session_workspaces("session-id-2".to_owned()).unwrap();
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].0, WorkspaceId(5));
        assert_eq!(locations[0].1, PathList::default());
        assert_eq!(locations[0].2, Some(50));
        assert_eq!(locations[0].3, Some(connection_id));
        assert_eq!(locations[1].0, WorkspaceId(3));
        assert_eq!(locations[1].1, PathList::new(&["/tmp3"]));
        assert_eq!(locations[1].2, Some(30));

        let locations = db.session_workspaces("session-id-3".to_owned()).unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].0, WorkspaceId(6));
        assert_eq!(
            locations[0].1,
            PathList::new(&["/tmp6c", "/tmp6b", "/tmp6a"]),
        );
        assert_eq!(locations[0].2, Some(60));
    }

    fn default_workspace<P: AsRef<Path>>(
        paths: &[P],
        center_group: &SerializedPaneGroup,
    ) -> SerializedWorkspace {
        SerializedWorkspace {
            id: WorkspaceId(4),
            paths: PathList::new(paths),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: center_group.clone(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            centered_layout: false,
            session_id: None,
            window_id: None,
            user_toolchains: Default::default(),
        }
    }

    #[gpui::test]
    async fn test_last_session_workspace_locations(cx: &mut gpui::TestAppContext) {
        let dir1 = tempfile::TempDir::with_prefix("dir1").unwrap();
        let dir2 = tempfile::TempDir::with_prefix("dir2").unwrap();
        let dir3 = tempfile::TempDir::with_prefix("dir3").unwrap();
        let dir4 = tempfile::TempDir::with_prefix("dir4").unwrap();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(dir1.path(), json!({})).await;
        fs.insert_tree(dir2.path(), json!({})).await;
        fs.insert_tree(dir3.path(), json!({})).await;
        fs.insert_tree(dir4.path(), json!({})).await;

        let db =
            WorkspaceDb::open_test_db("test_serializing_workspaces_last_session_workspaces").await;

        let workspaces = [
            (1, vec![dir1.path()], 9),
            (2, vec![dir2.path()], 5),
            (3, vec![dir3.path()], 8),
            (4, vec![dir4.path()], 2),
            (5, vec![dir1.path(), dir2.path(), dir3.path()], 3),
            (6, vec![dir4.path(), dir3.path(), dir2.path()], 4),
        ]
        .into_iter()
        .map(|(id, paths, window_id)| SerializedWorkspace {
            id: WorkspaceId(id),
            paths: PathList::new(paths.as_slice()),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: Some("one-session".to_owned()),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            window_id: Some(window_id),
            user_toolchains: Default::default(),
        })
        .collect::<Vec<_>>();

        for workspace in workspaces.iter() {
            db.save_workspace(workspace.clone()).await;
        }

        let stack = Some(Vec::from([
            WindowId::from(2), // Top
            WindowId::from(8),
            WindowId::from(5),
            WindowId::from(9),
            WindowId::from(3),
            WindowId::from(4), // Bottom
        ]));

        let locations = db
            .last_session_workspace_locations("one-session", stack, fs.as_ref())
            .await
            .unwrap();
        assert_eq!(
            locations,
            [
                SessionWorkspace {
                    workspace_id: WorkspaceId(4),
                    location: SerializedWorkspaceLocation::Local,
                    paths: PathList::new(&[dir4.path()]),
                    window_id: Some(WindowId::from(2u64)),
                },
                SessionWorkspace {
                    workspace_id: WorkspaceId(3),
                    location: SerializedWorkspaceLocation::Local,
                    paths: PathList::new(&[dir3.path()]),
                    window_id: Some(WindowId::from(8u64)),
                },
                SessionWorkspace {
                    workspace_id: WorkspaceId(2),
                    location: SerializedWorkspaceLocation::Local,
                    paths: PathList::new(&[dir2.path()]),
                    window_id: Some(WindowId::from(5u64)),
                },
                SessionWorkspace {
                    workspace_id: WorkspaceId(1),
                    location: SerializedWorkspaceLocation::Local,
                    paths: PathList::new(&[dir1.path()]),
                    window_id: Some(WindowId::from(9u64)),
                },
                SessionWorkspace {
                    workspace_id: WorkspaceId(5),
                    location: SerializedWorkspaceLocation::Local,
                    paths: PathList::new(&[dir1.path(), dir2.path(), dir3.path()]),
                    window_id: Some(WindowId::from(3u64)),
                },
                SessionWorkspace {
                    workspace_id: WorkspaceId(6),
                    location: SerializedWorkspaceLocation::Local,
                    paths: PathList::new(&[dir4.path(), dir3.path(), dir2.path()]),
                    window_id: Some(WindowId::from(4u64)),
                },
            ]
        );
    }

    fn pane_with_items(item_ids: &[ItemId]) -> SerializedPaneGroup {
        SerializedPaneGroup::Pane(SerializedPane::new(
            item_ids
                .iter()
                .map(|id| SerializedItem::new("Terminal", *id, true, false))
                .collect(),
            true,
            0,
        ))
    }

    fn empty_pane_group() -> SerializedPaneGroup {
        SerializedPaneGroup::Pane(SerializedPane::default())
    }

    fn workspace_with(
        id: u64,
        paths: &[&Path],
        center_group: SerializedPaneGroup,
        session_id: Option<&str>,
    ) -> SerializedWorkspace {
        SerializedWorkspace {
            id: WorkspaceId(id as i64),
            paths: PathList::new(paths),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group,
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            centered_layout: false,
            session_id: session_id.map(|s| s.to_owned()),
            window_id: Some(id),
            user_toolchains: Default::default(),
        }
    }

    #[gpui::test]
    async fn workspace_for_id_checked_propagates_constituent_query_failure() {
        let db = WorkspaceDb::open_test_db("workspace_for_id_checked_query_failure").await;
        let workspace_id = WorkspaceId(8801);
        db.save_workspace_checked(workspace_with(
            workspace_id.0 as u64,
            &[Path::new("/strict-query-failure")],
            empty_pane_group(),
            None,
        ))
        .await
        .unwrap();
        db.write(|connection| {
            connection
                .exec("DROP TABLE bookmarks")
                .and_then(|mut statement| statement())
        })
        .await
        .unwrap();

        let error = db
            .workspace_for_id_checked(workspace_id)
            .expect_err("strict restore must propagate a constituent query failure");
        assert!(
            format!("{error:#}").contains("bookmarks"),
            "the strict error should identify the failed constituent query"
        );
    }

    fn remote_workspace_with(id: u64, host: &str, paths: &[&Path]) -> SerializedWorkspace {
        SerializedWorkspace {
            id: WorkspaceId(id as i64),
            paths: PathList::new(paths),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Remote(RemoteConnectionOptions::Ssh(
                SshConnectionOptions {
                    host: host.into(),
                    ..Default::default()
                },
            )),
            center_group: empty_pane_group(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            centered_layout: false,
            session_id: None,
            window_id: Some(id),
            user_toolchains: Default::default(),
        }
    }

    async fn local_recent_workspace(
        workspace_id: WorkspaceId,
        paths: PathList,
        timestamp: DateTime<Utc>,
        fs: &dyn Fs,
    ) -> RecentWorkspace {
        let identity_paths = resolve_local_workspace_identity(fs, &paths)
            .await
            .unwrap_or_else(|| paths.clone());
        RecentWorkspace {
            workspace_id,
            location: SerializedWorkspaceLocation::Local,
            paths,
            identity_paths,
            timestamp,
        }
    }

    #[gpui::test]
    async fn test_scratch_only_workspace_restores_from_last_session(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());
        let db =
            WorkspaceDb::open_test_db("test_scratch_only_workspace_restores_from_last_session")
                .await;

        db.save_workspace(workspace_with(1, &[], pane_with_items(&[100]), Some("s1")))
            .await;

        let sessions = db
            .last_session_workspace_locations("s1", None, fs.as_ref())
            .await
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].workspace_id, WorkspaceId(1));
        assert!(sessions[0].paths.is_empty());

        let recents = db.recent_project_workspaces(fs.as_ref()).await.unwrap();
        assert!(
            recents
                .iter()
                .all(|workspace| workspace.workspace_id != WorkspaceId(1)),
            "scratch-only workspace must not appear in the recent-projects UI"
        );
    }

    #[gpui::test]
    async fn test_gc_preserves_scratch_inside_window(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());
        let db = WorkspaceDb::open_test_db("test_gc_preserves_scratch_inside_window").await;

        db.save_workspace(workspace_with(1, &[], empty_pane_group(), None))
            .await;

        db.garbage_collect_workspaces(fs.as_ref(), "current", None, &Default::default())
            .await
            .unwrap();
        assert!(
            db.workspace_for_id(WorkspaceId(1)).is_some(),
            "fresh stale workspace must not be deleted before the 7-day window"
        );
    }

    #[gpui::test]
    async fn test_gc_deletes_stale_outside_window(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());
        let db = WorkspaceDb::open_test_db("test_gc_deletes_stale_outside_window").await;

        db.save_workspace(workspace_with(1, &[], empty_pane_group(), None))
            .await;
        db.set_timestamp_for_tests(WorkspaceId(1), "2000-01-01 00:00:00".to_owned())
            .await
            .expect("failed to age the configuration-referenced workspace");

        db.garbage_collect_workspaces(fs.as_ref(), "current", None, &Default::default())
            .await
            .unwrap();
        assert!(
            db.workspace_for_id(WorkspaceId(1)).is_none(),
            "stale empty workspace older than the retention window must be deleted"
        );
    }

    #[gpui::test]
    async fn workspace_configuration_row_retention_preserves_referenced_workspace_during_gc(
        cx: &mut gpui::TestAppContext,
    ) {
        let fs = fs::FakeFs::new(cx.executor());
        let db = WorkspaceDb::open_test_db("test_gc_preserves_configuration_reference").await;

        db.save_workspace(workspace_with(1, &[], empty_pane_group(), None))
            .await;
        db.set_timestamp_for_tests(WorkspaceId(1), "2000-01-01 00:00:00".to_owned())
            .await
            .expect("failed to age the configuration-referenced workspace");

        let mut protected_workspace_ids = HashSet::default();
        protected_workspace_ids.insert(WorkspaceId(1));
        db.garbage_collect_workspaces(fs.as_ref(), "current", None, &protected_workspace_ids)
            .await
            .expect("failed to garbage collect while preserving referenced workspaces");

        assert!(
            db.workspace_for_id(WorkspaceId(1)).is_some(),
            "configuration-referenced exact rows must survive startup GC"
        );
    }

    #[gpui::test]
    async fn test_gc_preserves_directory_workspace_with_missing_path(
        cx: &mut gpui::TestAppContext,
    ) {
        let fs = fs::FakeFs::new(cx.executor());
        let db =
            WorkspaceDb::open_test_db("test_gc_preserves_directory_workspace_with_missing_path")
                .await;

        let missing_dir = PathBuf::from("/missing-project-dir");
        db.save_workspace(workspace_with(
            1,
            &[missing_dir.as_path()],
            empty_pane_group(),
            None,
        ))
        .await;

        db.garbage_collect_workspaces(fs.as_ref(), "current", None, &Default::default())
            .await
            .unwrap();
        assert!(
            db.workspace_for_id(WorkspaceId(1)).is_some(),
            "a stale workspace within the retention window must be kept"
        );

        db.set_timestamp_for_tests(WorkspaceId(1), "2000-01-01 00:00:00".to_owned())
            .await
            .unwrap();
        db.garbage_collect_workspaces(fs.as_ref(), "current", None, &Default::default())
            .await
            .unwrap();
        assert!(
            db.workspace_for_id(WorkspaceId(1)).is_none(),
            "a stale workspace past the retention window must be deleted"
        );
    }

    #[gpui::test]
    async fn test_gc_preserves_current_and_last_sessions(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());
        let db = WorkspaceDb::open_test_db("test_gc_preserves_current_and_last_sessions").await;

        db.save_workspace(workspace_with(1, &[], empty_pane_group(), Some("current")))
            .await;
        db.save_workspace(workspace_with(2, &[], empty_pane_group(), Some("last")))
            .await;
        db.save_workspace(workspace_with(3, &[], empty_pane_group(), Some("stale")))
            .await;

        for id in [1, 2, 3] {
            db.set_timestamp_for_tests(WorkspaceId(id), "2000-01-01 00:00:00".to_owned())
                .await
                .unwrap();
        }

        db.garbage_collect_workspaces(fs.as_ref(), "current", Some("last"), &Default::default())
            .await
            .unwrap();

        assert!(
            db.workspace_for_id(WorkspaceId(1)).is_some(),
            "GC must not delete workspaces belonging to the current session"
        );
        assert!(
            db.workspace_for_id(WorkspaceId(2)).is_some(),
            "GC must not delete workspaces belonging to the last session"
        );
        assert!(
            db.workspace_for_id(WorkspaceId(3)).is_none(),
            "GC should still delete stale workspaces from other sessions"
        );
    }

    #[gpui::test]
    async fn test_gc_deletes_empty_workspace_with_items(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());
        let db = WorkspaceDb::open_test_db("test_gc_deletes_empty_workspace_with_items").await;

        db.save_workspace(workspace_with(1, &[], pane_with_items(&[100]), None))
            .await;
        db.set_timestamp_for_tests(WorkspaceId(1), "2000-01-01 00:00:00".to_owned())
            .await
            .unwrap();

        db.garbage_collect_workspaces(fs.as_ref(), "current", None, &Default::default())
            .await
            .unwrap();
        assert!(
            db.workspace_for_id(WorkspaceId(1)).is_none(),
            "a stale empty-path workspace must be deleted regardless of its items"
        );
    }

    #[gpui::test]
    async fn test_last_session_restores_workspace_with_missing_paths(
        cx: &mut gpui::TestAppContext,
    ) {
        let fs = fs::FakeFs::new(cx.executor());
        let db =
            WorkspaceDb::open_test_db("test_last_session_restores_workspace_with_missing_paths")
                .await;

        let missing = PathBuf::from("/gone/file.rs");
        db.save_workspace(workspace_with(
            1,
            &[missing.as_path()],
            empty_pane_group(),
            Some("s"),
        ))
        .await;

        let sessions = db
            .last_session_workspace_locations("s", None, fs.as_ref())
            .await
            .unwrap();
        assert!(
            sessions.is_empty(),
            "workspaces whose paths no longer exist on disk must not restore"
        );
    }

    #[gpui::test]
    async fn test_last_session_workspace_locations_remote(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());
        let db =
            WorkspaceDb::open_test_db("test_serializing_workspaces_last_session_workspaces_remote")
                .await;

        let remote_connections = [
            ("host-1", "my-user-1"),
            ("host-2", "my-user-2"),
            ("host-3", "my-user-3"),
            ("host-4", "my-user-4"),
        ]
        .into_iter()
        .map(|(host, user)| async {
            let options = RemoteConnectionOptions::Ssh(SshConnectionOptions {
                host: host.into(),
                username: Some(user.to_string()),
                ..Default::default()
            });
            db.get_or_create_remote_connection(options.clone())
                .await
                .unwrap();
            options
        })
        .collect::<Vec<_>>();

        let remote_connections = futures::future::join_all(remote_connections).await;

        let workspaces = [
            (1, remote_connections[0].clone(), 9),
            (2, remote_connections[1].clone(), 5),
            (3, remote_connections[2].clone(), 8),
            (4, remote_connections[3].clone(), 2),
        ]
        .into_iter()
        .map(|(id, remote_connection, window_id)| SerializedWorkspace {
            id: WorkspaceId(id),
            paths: PathList::default(),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Remote(remote_connection),
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: Some("one-session".to_owned()),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            window_id: Some(window_id),
            user_toolchains: Default::default(),
        })
        .collect::<Vec<_>>();

        for workspace in workspaces.iter() {
            db.save_workspace(workspace.clone()).await;
        }

        let stack = Some(Vec::from([
            WindowId::from(2), // Top
            WindowId::from(8),
            WindowId::from(5),
            WindowId::from(9), // Bottom
        ]));

        let have = db
            .last_session_workspace_locations("one-session", stack, fs.as_ref())
            .await
            .unwrap();
        assert_eq!(have.len(), 4);
        assert_eq!(
            have[0],
            SessionWorkspace {
                workspace_id: WorkspaceId(4),
                location: SerializedWorkspaceLocation::Remote(remote_connections[3].clone()),
                paths: PathList::default(),
                window_id: Some(WindowId::from(2u64)),
            }
        );
        assert_eq!(
            have[1],
            SessionWorkspace {
                workspace_id: WorkspaceId(3),
                location: SerializedWorkspaceLocation::Remote(remote_connections[2].clone()),
                paths: PathList::default(),
                window_id: Some(WindowId::from(8u64)),
            }
        );
        assert_eq!(
            have[2],
            SessionWorkspace {
                workspace_id: WorkspaceId(2),
                location: SerializedWorkspaceLocation::Remote(remote_connections[1].clone()),
                paths: PathList::default(),
                window_id: Some(WindowId::from(5u64)),
            }
        );
        assert_eq!(
            have[3],
            SessionWorkspace {
                workspace_id: WorkspaceId(1),
                location: SerializedWorkspaceLocation::Remote(remote_connections[0].clone()),
                paths: PathList::default(),
                window_id: Some(WindowId::from(9u64)),
            }
        );
    }

    #[gpui::test]
    async fn test_get_or_create_ssh_project() {
        let db = WorkspaceDb::open_test_db("test_get_or_create_ssh_project").await;

        let host = "example.com".to_string();
        let port = Some(22_u16);
        let user = Some("user".to_string());

        let connection_id = db
            .get_or_create_remote_connection(RemoteConnectionOptions::Ssh(SshConnectionOptions {
                host: host.clone().into(),
                port,
                username: user.clone(),
                ..Default::default()
            }))
            .await
            .unwrap();

        // Test that calling the function again with the same parameters returns the same project
        let same_connection = db
            .get_or_create_remote_connection(RemoteConnectionOptions::Ssh(SshConnectionOptions {
                host: host.clone().into(),
                port,
                username: user.clone(),
                ..Default::default()
            }))
            .await
            .unwrap();

        assert_eq!(connection_id, same_connection);

        // Test with different parameters
        let host2 = "otherexample.com".to_string();
        let port2 = None;
        let user2 = Some("otheruser".to_string());

        let different_connection = db
            .get_or_create_remote_connection(RemoteConnectionOptions::Ssh(SshConnectionOptions {
                host: host2.clone().into(),
                port: port2,
                username: user2.clone(),
                ..Default::default()
            }))
            .await
            .unwrap();

        assert_ne!(connection_id, different_connection);
    }

    #[gpui::test]
    async fn test_get_or_create_ssh_project_with_null_user() {
        let db = WorkspaceDb::open_test_db("test_get_or_create_ssh_project_with_null_user").await;

        let (host, port, user) = ("example.com".to_string(), None, None);

        let connection_id = db
            .get_or_create_remote_connection(RemoteConnectionOptions::Ssh(SshConnectionOptions {
                host: host.clone().into(),
                port,
                username: None,
                ..Default::default()
            }))
            .await
            .unwrap();

        let same_connection_id = db
            .get_or_create_remote_connection(RemoteConnectionOptions::Ssh(SshConnectionOptions {
                host: host.clone().into(),
                port,
                username: user.clone(),
                ..Default::default()
            }))
            .await
            .unwrap();

        assert_eq!(connection_id, same_connection_id);
    }

    #[gpui::test]
    async fn test_get_remote_connections() {
        let db = WorkspaceDb::open_test_db("test_get_remote_connections").await;

        let connections = [
            ("example.com".to_string(), None, None),
            (
                "anotherexample.com".to_string(),
                Some(123_u16),
                Some("user2".to_string()),
            ),
            ("yetanother.com".to_string(), Some(345_u16), None),
        ];

        let mut ids = Vec::new();
        for (host, port, user) in connections.iter() {
            ids.push(
                db.get_or_create_remote_connection(RemoteConnectionOptions::Ssh(
                    SshConnectionOptions {
                        host: host.clone().into(),
                        port: *port,
                        username: user.clone(),
                        ..Default::default()
                    },
                ))
                .await
                .unwrap(),
            );
        }

        let stored_connections = db.remote_connections().unwrap();
        assert_eq!(
            stored_connections,
            [
                (
                    ids[0],
                    RemoteConnectionOptions::Ssh(SshConnectionOptions {
                        host: "example.com".into(),
                        port: None,
                        username: None,
                        ..Default::default()
                    }),
                ),
                (
                    ids[1],
                    RemoteConnectionOptions::Ssh(SshConnectionOptions {
                        host: "anotherexample.com".into(),
                        port: Some(123),
                        username: Some("user2".into()),
                        ..Default::default()
                    }),
                ),
                (
                    ids[2],
                    RemoteConnectionOptions::Ssh(SshConnectionOptions {
                        host: "yetanother.com".into(),
                        port: Some(345),
                        username: None,
                        ..Default::default()
                    }),
                ),
            ]
            .into_iter()
            .collect::<HashMap<_, _>>(),
        );
    }

    #[gpui::test]
    async fn test_simple_split() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("simple_split").await;

        //  -----------------
        //  | 1,2   | 5,6   |
        //  | - - - |       |
        //  | 3,4   |       |
        //  -----------------
        let center_pane = group(
            Axis::Horizontal,
            vec![
                group(
                    Axis::Vertical,
                    vec![
                        SerializedPaneGroup::Pane(SerializedPane::new(
                            vec![
                                SerializedItem::new("Terminal", 1, false, false),
                                SerializedItem::new("Terminal", 2, true, false),
                            ],
                            false,
                            0,
                        )),
                        SerializedPaneGroup::Pane(SerializedPane::new(
                            vec![
                                SerializedItem::new("Terminal", 4, false, false),
                                SerializedItem::new("Terminal", 3, true, false),
                            ],
                            true,
                            0,
                        )),
                    ],
                ),
                SerializedPaneGroup::Pane(SerializedPane::new(
                    vec![
                        SerializedItem::new("Terminal", 5, true, false),
                        SerializedItem::new("Terminal", 6, false, false),
                    ],
                    false,
                    0,
                )),
            ],
        );

        let workspace = default_workspace(&["/tmp"], &center_pane);

        db.save_workspace(workspace.clone()).await;

        let new_workspace = db.workspace_for_roots(&["/tmp"]).unwrap();

        assert_eq!(workspace.center_group, new_workspace.center_group);
    }

    #[gpui::test]
    async fn test_cleanup_panes() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("test_cleanup_panes").await;

        let center_pane = group(
            Axis::Horizontal,
            vec![
                group(
                    Axis::Vertical,
                    vec![
                        SerializedPaneGroup::Pane(SerializedPane::new(
                            vec![
                                SerializedItem::new("Terminal", 1, false, false),
                                SerializedItem::new("Terminal", 2, true, false),
                            ],
                            false,
                            0,
                        )),
                        SerializedPaneGroup::Pane(SerializedPane::new(
                            vec![
                                SerializedItem::new("Terminal", 4, false, false),
                                SerializedItem::new("Terminal", 3, true, false),
                            ],
                            true,
                            0,
                        )),
                    ],
                ),
                SerializedPaneGroup::Pane(SerializedPane::new(
                    vec![
                        SerializedItem::new("Terminal", 5, false, false),
                        SerializedItem::new("Terminal", 6, true, false),
                    ],
                    false,
                    0,
                )),
            ],
        );

        let id = &["/tmp"];

        let mut workspace = default_workspace(id, &center_pane);

        db.save_workspace(workspace.clone()).await;

        workspace.center_group = group(
            Axis::Vertical,
            vec![
                SerializedPaneGroup::Pane(SerializedPane::new(
                    vec![
                        SerializedItem::new("Terminal", 1, false, false),
                        SerializedItem::new("Terminal", 2, true, false),
                    ],
                    false,
                    0,
                )),
                SerializedPaneGroup::Pane(SerializedPane::new(
                    vec![
                        SerializedItem::new("Terminal", 4, true, false),
                        SerializedItem::new("Terminal", 3, false, false),
                    ],
                    true,
                    0,
                )),
            ],
        );

        db.save_workspace(workspace.clone()).await;

        let new_workspace = db.workspace_for_roots(id).unwrap();

        assert_eq!(workspace.center_group, new_workspace.center_group);
    }

    #[gpui::test]
    async fn test_empty_workspace_window_bounds() {
        zlog::init_test();

        let db = WorkspaceDb::open_test_db("test_empty_workspace_window_bounds").await;
        let id = db.next_id().await.unwrap();

        // Create a workspace with empty paths (empty workspace)
        let empty_paths: &[&str] = &[];
        let display_uuid = Uuid::new_v4();
        let window_bounds = SerializedWindowBounds(WindowBounds::Windowed(Bounds {
            origin: point(px(100.0), px(200.0)),
            size: size(px(800.0), px(600.0)),
        }));

        let workspace = SerializedWorkspace {
            id,
            paths: PathList::new(empty_paths),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: None,
            display: None,
            docks: Default::default(),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            centered_layout: false,
            session_id: None,
            window_id: None,
            user_toolchains: Default::default(),
        };

        // Save the workspace (this creates the record with empty paths)
        db.save_workspace(workspace.clone()).await;

        // Save window bounds separately (as the actual code does via set_window_open_status)
        db.set_window_open_status(id, window_bounds, display_uuid)
            .await
            .unwrap();

        // Empty workspaces cannot be retrieved by paths (they'd all match).
        // They must be retrieved by workspace_id.
        assert!(db.workspace_for_roots(empty_paths).is_none());

        // Retrieve using workspace_for_id instead
        let retrieved = db.workspace_for_id(id).unwrap();

        // Verify window bounds were persisted
        assert_eq!(retrieved.id, id);
        assert!(retrieved.window_bounds.is_some());
        assert_eq!(retrieved.window_bounds.unwrap().0, window_bounds.0);
        assert!(retrieved.display.is_some());
        assert_eq!(retrieved.display.unwrap(), display_uuid);
    }

    #[gpui::test]
    async fn test_last_session_workspace_locations_groups_by_window_id(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir1 = tempfile::TempDir::with_prefix("dir1").unwrap();
        let dir2 = tempfile::TempDir::with_prefix("dir2").unwrap();
        let dir3 = tempfile::TempDir::with_prefix("dir3").unwrap();
        let dir4 = tempfile::TempDir::with_prefix("dir4").unwrap();
        let dir5 = tempfile::TempDir::with_prefix("dir5").unwrap();

        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(dir1.path(), json!({})).await;
        fs.insert_tree(dir2.path(), json!({})).await;
        fs.insert_tree(dir3.path(), json!({})).await;
        fs.insert_tree(dir4.path(), json!({})).await;
        fs.insert_tree(dir5.path(), json!({})).await;

        let db =
            WorkspaceDb::open_test_db("test_last_session_workspace_locations_groups_by_window_id")
                .await;

        // Simulate two MultiWorkspace windows each containing two workspaces,
        // plus one single-workspace window:
        //   Window 10: workspace 1, workspace 2
        //   Window 20: workspace 3, workspace 4
        //   Window 30: workspace 5 (only one)
        //
        // On session restore, the caller should be able to group these by
        // window_id to reconstruct the MultiWorkspace windows.
        let workspaces_data: Vec<(i64, &Path, u64)> = vec![
            (1, dir1.path(), 10),
            (2, dir2.path(), 10),
            (3, dir3.path(), 20),
            (4, dir4.path(), 20),
            (5, dir5.path(), 30),
        ];

        for (id, dir, window_id) in &workspaces_data {
            db.save_workspace(SerializedWorkspace {
                id: WorkspaceId(*id),
                paths: PathList::new(&[*dir]),
                identity_paths: None,
                location: SerializedWorkspaceLocation::Local,
                center_group: Default::default(),
                window_bounds: Default::default(),
                display: Default::default(),
                docks: Default::default(),
                centered_layout: false,
                session_id: Some("test-session".to_owned()),
                bookmarks: Default::default(),
                breakpoints: Default::default(),
                window_id: Some(*window_id),
                user_toolchains: Default::default(),
            })
            .await;
        }

        let locations = db
            .last_session_workspace_locations("test-session", None, fs.as_ref())
            .await
            .unwrap();

        // All 5 workspaces should be returned with their window_ids.
        assert_eq!(locations.len(), 5);

        // Every entry should have a window_id so the caller can group them.
        for session_workspace in &locations {
            assert!(
                session_workspace.window_id.is_some(),
                "workspace {:?} missing window_id",
                session_workspace.workspace_id
            );
        }

        // Group by window_id, simulating what the restoration code should do.
        let mut by_window: HashMap<WindowId, Vec<WorkspaceId>> = HashMap::default();
        for session_workspace in &locations {
            if let Some(window_id) = session_workspace.window_id {
                by_window
                    .entry(window_id)
                    .or_default()
                    .push(session_workspace.workspace_id);
            }
        }

        // Should produce 3 windows, not 5.
        assert_eq!(
            by_window.len(),
            3,
            "Expected 3 window groups, got {}: {:?}",
            by_window.len(),
            by_window
        );

        // Window 10 should contain workspaces 1 and 2.
        let window_10 = by_window.get(&WindowId::from(10u64)).unwrap();
        assert_eq!(window_10.len(), 2);
        assert!(window_10.contains(&WorkspaceId(1)));
        assert!(window_10.contains(&WorkspaceId(2)));

        // Window 20 should contain workspaces 3 and 4.
        let window_20 = by_window.get(&WindowId::from(20u64)).unwrap();
        assert_eq!(window_20.len(), 2);
        assert!(window_20.contains(&WorkspaceId(3)));
        assert!(window_20.contains(&WorkspaceId(4)));

        // Window 30 should contain only workspace 5.
        let window_30 = by_window.get(&WindowId::from(30u64)).unwrap();
        assert_eq!(window_30.len(), 1);
        assert!(window_30.contains(&WorkspaceId(5)));
    }

    #[gpui::test]
    async fn test_read_serialized_multi_workspaces_with_state(cx: &mut gpui::TestAppContext) {
        use crate::persistence::model::MultiWorkspaceState;

        // Write multi-workspace state for two windows via the scoped KVP.
        let window_10 = WindowId::from(10u64);
        let window_20 = WindowId::from(20u64);

        let kvp = cx.update(|cx| KeyValueStore::global(cx));

        write_multi_workspace_state(
            &kvp,
            window_10,
            MultiWorkspaceState {
                active_workspace_id: Some(WorkspaceId(2)),
                project_groups: vec![],
                sidebar_open: true,
                sidebar_state: None,
                active_configuration_id: None,
            },
        )
        .await;

        write_multi_workspace_state(
            &kvp,
            window_20,
            MultiWorkspaceState {
                active_workspace_id: Some(WorkspaceId(3)),
                project_groups: vec![],
                sidebar_open: false,
                sidebar_state: None,
                active_configuration_id: None,
            },
        )
        .await;

        // Build session workspaces: two in window 10, one in window 20, one with no window.
        let session_workspaces = vec![
            SessionWorkspace {
                workspace_id: WorkspaceId(1),
                location: SerializedWorkspaceLocation::Local,
                paths: PathList::new(&["/a"]),
                window_id: Some(window_10),
            },
            SessionWorkspace {
                workspace_id: WorkspaceId(2),
                location: SerializedWorkspaceLocation::Local,
                paths: PathList::new(&["/b"]),
                window_id: Some(window_10),
            },
            SessionWorkspace {
                workspace_id: WorkspaceId(3),
                location: SerializedWorkspaceLocation::Local,
                paths: PathList::new(&["/c"]),
                window_id: Some(window_20),
            },
            SessionWorkspace {
                workspace_id: WorkspaceId(4),
                location: SerializedWorkspaceLocation::Local,
                paths: PathList::new(&["/d"]),
                window_id: None,
            },
        ];

        let results = cx.update(|cx| read_serialized_multi_workspaces(session_workspaces, cx));

        // Should produce 3 results: window 10, window 20, and the orphan.
        assert_eq!(results.len(), 3);

        // Window 10: active_workspace_id = 2 picks workspace 2 (paths /b), sidebar open.
        let group_10 = &results[0];
        assert_eq!(group_10.active_workspace.workspace_id, WorkspaceId(2));
        assert_eq!(
            group_10
                .workspaces
                .iter()
                .map(|workspace| workspace.workspace_id)
                .collect::<Vec<_>>(),
            vec![WorkspaceId(1), WorkspaceId(2)]
        );
        assert_eq!(group_10.state.active_workspace_id, Some(WorkspaceId(2)));
        assert_eq!(group_10.state.sidebar_open, true);

        // Window 20: active_workspace_id = 3 picks workspace 3 (paths /c), sidebar closed.
        let group_20 = &results[1];
        assert_eq!(group_20.active_workspace.workspace_id, WorkspaceId(3));
        assert_eq!(
            group_20
                .workspaces
                .iter()
                .map(|workspace| workspace.workspace_id)
                .collect::<Vec<_>>(),
            vec![WorkspaceId(3)]
        );
        assert_eq!(group_20.state.active_workspace_id, Some(WorkspaceId(3)));
        assert_eq!(group_20.state.sidebar_open, false);

        // Orphan: no active_workspace_id, falls back to first workspace (id 4).
        let group_none = &results[2];
        assert_eq!(group_none.active_workspace.workspace_id, WorkspaceId(4));
        assert_eq!(
            group_none
                .workspaces
                .iter()
                .map(|workspace| workspace.workspace_id)
                .collect::<Vec<_>>(),
            vec![WorkspaceId(4)]
        );
        assert_eq!(group_none.state.active_workspace_id, None);
        assert_eq!(group_none.state.sidebar_open, false);
    }

    #[gpui::test]
    async fn test_flush_serialization_completes_before_quit(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let project = Project::test(fs.clone(), [], cx).await;

        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        let db = cx.update(|_, cx| WorkspaceDb::global(cx));

        // Assign a database_id so serialization will actually persist.
        let workspace_id = db.next_id().await.unwrap();
        workspace.update(cx, |ws, _cx| {
            ws.set_database_id(workspace_id);
        });

        // Mutate some workspace state.
        db.set_centered_layout(workspace_id, true).await.unwrap();

        // Call flush_serialization and await the returned task directly
        // (without run_until_parked — the point is that awaiting the task
        // alone is sufficient).
        let task = multi_workspace.update_in(cx, |mw, window, cx| {
            mw.workspace()
                .update(cx, |ws, cx| ws.flush_serialization(window, cx))
        });
        task.await;

        // Read the workspace back from the DB and verify serialization happened.
        let serialized = db.workspace_for_id(workspace_id);
        assert!(
            serialized.is_some(),
            "flush_serialization should have persisted the workspace to DB"
        );
    }

    #[gpui::test]
    async fn test_create_workspace_serialization(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let project = Project::test(fs.clone(), [], cx).await;

        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        // Give the first workspace a database_id.
        multi_workspace.update_in(cx, |mw, _, cx| {
            mw.set_random_database_id(cx);
        });

        let window_id =
            multi_workspace.update_in(cx, |_, window, _cx| window.window_handle().window_id());

        // Create a new workspace via the MultiWorkspace API (triggers next_id()).
        multi_workspace.update_in(cx, |mw, window, cx| {
            mw.create_test_workspace(window, cx).detach();
        });

        // Let the async next_id() and re-serialization tasks complete.
        cx.run_until_parked();

        // The new workspace should now have a database_id.
        let new_workspace_db_id =
            multi_workspace.read_with(cx, |mw, cx| mw.workspace().read(cx).database_id());
        assert!(
            new_workspace_db_id.is_some(),
            "New workspace should have a database_id after run_until_parked"
        );

        // The multi-workspace state should record it as the active workspace.
        let state = cx.update(|_, cx| read_multi_workspace_state(window_id, cx));
        assert_eq!(
            state.active_workspace_id, new_workspace_db_id,
            "Serialized active_workspace_id should match the new workspace's database_id"
        );

        // The individual workspace row should exist with real data
        // (not just the bare DEFAULT VALUES row from next_id).
        let workspace_id = new_workspace_db_id.unwrap();
        let db = cx.update(|_, cx| WorkspaceDb::global(cx));
        let serialized = db.workspace_for_id(workspace_id);
        assert!(
            serialized.is_some(),
            "Newly created workspace should be fully serialized in the DB after database_id assignment"
        );
    }

    #[gpui::test]
    async fn test_remove_workspace_clears_session_binding(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let dir = unique_test_dir(&fs, "remove").await;
        let project1 = Project::test(fs.clone(), [], cx).await;
        let project2 = Project::test(fs.clone(), [], cx).await;

        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project1.clone(), window, cx));

        multi_workspace.update(cx, |mw, cx| {
            mw.open_sidebar(cx);
        });

        multi_workspace.update_in(cx, |mw, _, cx| {
            mw.set_random_database_id(cx);
        });

        let db = cx.update(|_, cx| WorkspaceDb::global(cx));

        // Get a real DB id for workspace2 so the row actually exists.
        let workspace2_db_id = db.next_id().await.unwrap();

        multi_workspace.update_in(cx, |mw, window, cx| {
            let workspace = cx.new(|cx| crate::Workspace::test_new(project2.clone(), window, cx));
            workspace.update(cx, |ws: &mut crate::Workspace, _cx| {
                ws.set_database_id(workspace2_db_id)
            });
            mw.add(workspace.clone(), window, cx);
        });

        // Save a full workspace row to the DB directly.
        let session_id = format!("remove-test-session-{}", Uuid::new_v4());
        db.save_workspace(SerializedWorkspace {
            id: workspace2_db_id,
            paths: PathList::new(&[&dir]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: Some(session_id.clone()),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            window_id: Some(99),
            user_toolchains: Default::default(),
        })
        .await;

        assert!(
            db.workspace_for_id(workspace2_db_id).is_some(),
            "Workspace2 should exist in DB before removal"
        );

        // Remove workspace at index 1 (the second workspace).
        multi_workspace.update_in(cx, |mw, window, cx| {
            let ws = mw.workspaces().nth(1).unwrap().clone();
            mw.remove([ws], RemovalIntent::CloseProject, window, cx)
                .detach_and_log_err(cx);
        });

        cx.run_until_parked();

        // The row should still exist so it continues to appear in recent
        // projects, but the session binding should be cleared so it is not
        // restored as part of any future session.
        assert!(
            db.workspace_for_id(workspace2_db_id).is_some(),
            "Removed workspace's DB row should be preserved for recent projects"
        );

        let session_workspaces = db
            .last_session_workspace_locations("remove-test-session", None, fs.as_ref())
            .await
            .unwrap();
        let restored_ids: Vec<WorkspaceId> = session_workspaces
            .iter()
            .map(|sw| sw.workspace_id)
            .collect();
        assert!(
            !restored_ids.contains(&workspace2_db_id),
            "Removed workspace should not appear in session restoration"
        );
    }

    #[gpui::test]
    async fn test_remove_workspace_not_restored_as_zombie(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let dir1 = tempfile::TempDir::with_prefix("zombie_test1").unwrap();
        let dir2 = tempfile::TempDir::with_prefix("zombie_test2").unwrap();
        fs.insert_tree(dir1.path(), json!({})).await;
        fs.insert_tree(dir2.path(), json!({})).await;

        let project1 = Project::test(fs.clone(), [], cx).await;
        let project2 = Project::test(fs.clone(), [], cx).await;

        let db = cx.update(|cx| WorkspaceDb::global(cx));

        // Get real DB ids so the rows actually exist.
        let ws1_id = db.next_id().await.unwrap();
        let ws2_id = db.next_id().await.unwrap();

        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project1.clone(), window, cx));

        multi_workspace.update(cx, |mw, cx| {
            mw.open_sidebar(cx);
        });

        multi_workspace.update_in(cx, |mw, _, cx| {
            mw.workspace().update(cx, |ws, _cx| {
                ws.set_database_id(ws1_id);
            });
        });

        multi_workspace.update_in(cx, |mw, window, cx| {
            let workspace = cx.new(|cx| crate::Workspace::test_new(project2.clone(), window, cx));
            workspace.update(cx, |ws: &mut crate::Workspace, _cx| {
                ws.set_database_id(ws2_id)
            });
            mw.add(workspace.clone(), window, cx);
        });

        let session_id = "test-zombie-session";
        let window_id_val: u64 = 42;

        db.save_workspace(SerializedWorkspace {
            id: ws1_id,
            paths: PathList::new(&[dir1.path()]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: Some(session_id.to_owned()),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            window_id: Some(window_id_val),
            user_toolchains: Default::default(),
        })
        .await;

        db.save_workspace(SerializedWorkspace {
            id: ws2_id,
            paths: PathList::new(&[dir2.path()]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: Some(session_id.to_owned()),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            window_id: Some(window_id_val),
            user_toolchains: Default::default(),
        })
        .await;

        // Remove workspace2 (index 1).
        multi_workspace.update_in(cx, |mw, window, cx| {
            let ws = mw.workspaces().nth(1).unwrap().clone();
            mw.remove([ws], RemovalIntent::CloseProject, window, cx)
                .detach_and_log_err(cx);
        });

        cx.run_until_parked();

        // The removed workspace should NOT appear in session restoration.
        let locations = db
            .last_session_workspace_locations(session_id, None, fs.as_ref())
            .await
            .unwrap();

        let restored_ids: Vec<WorkspaceId> = locations.iter().map(|sw| sw.workspace_id).collect();
        assert!(
            !restored_ids.contains(&ws2_id),
            "Removed workspace should not appear in session restoration list. Found: {:?}",
            restored_ids
        );
        assert!(
            restored_ids.contains(&ws1_id),
            "Remaining workspace should still appear in session restoration list"
        );
    }

    #[gpui::test]
    async fn test_pending_removal_tasks_drained_on_flush(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let dir = unique_test_dir(&fs, "pending-removal").await;
        let project1 = Project::test(fs.clone(), [], cx).await;
        let project2 = Project::test(fs.clone(), [], cx).await;

        let db = cx.update(|cx| WorkspaceDb::global(cx));

        // Get a real DB id for workspace2 so the row actually exists.
        let workspace2_db_id = db.next_id().await.unwrap();

        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project1.clone(), window, cx));

        multi_workspace.update(cx, |mw, cx| {
            mw.open_sidebar(cx);
        });

        multi_workspace.update_in(cx, |mw, _, cx| {
            mw.set_random_database_id(cx);
        });

        multi_workspace.update_in(cx, |mw, window, cx| {
            let workspace = cx.new(|cx| crate::Workspace::test_new(project2.clone(), window, cx));
            workspace.update(cx, |ws: &mut crate::Workspace, _cx| {
                ws.set_database_id(workspace2_db_id)
            });
            mw.add(workspace.clone(), window, cx);
        });

        // Save a full workspace row to the DB directly and let it settle.
        let session_id = format!("pending-removal-session-{}", Uuid::new_v4());
        db.save_workspace(SerializedWorkspace {
            id: workspace2_db_id,
            paths: PathList::new(&[&dir]),
            identity_paths: None,
            location: SerializedWorkspaceLocation::Local,
            center_group: Default::default(),
            window_bounds: Default::default(),
            display: Default::default(),
            docks: Default::default(),
            centered_layout: false,
            session_id: Some(session_id.clone()),
            bookmarks: Default::default(),
            breakpoints: Default::default(),
            window_id: Some(88),
            user_toolchains: Default::default(),
        })
        .await;
        cx.run_until_parked();

        // Remove workspace2 — this pushes a task to pending_removal_tasks.
        multi_workspace.update_in(cx, |mw, window, cx| {
            let ws = mw.workspaces().nth(1).unwrap().clone();
            mw.remove([ws], RemovalIntent::CloseProject, window, cx)
                .detach_and_log_err(cx);
        });

        // Simulate the quit handler pattern: collect flush tasks + pending
        // removal tasks and await them all.
        let all_tasks = multi_workspace.update_in(cx, |mw, window, cx| {
            let mut tasks: Vec<Task<()>> = mw
                .workspaces()
                .map(|workspace| {
                    workspace.update(cx, |workspace, cx| {
                        workspace.flush_serialization(window, cx)
                    })
                })
                .collect();
            let mut removal_tasks = mw.take_pending_removal_tasks();
            // Note: removal_tasks may be empty if the background task already
            // completed (take_pending_removal_tasks filters out ready tasks).
            tasks.append(&mut removal_tasks);
            tasks.push(mw.flush_serialization());
            tasks
        });
        futures::future::join_all(all_tasks).await;

        // The row should still exist (for recent projects), but the session
        // binding should have been cleared by the pending removal task.
        assert!(
            db.workspace_for_id(workspace2_db_id).is_some(),
            "Workspace row should be preserved for recent projects"
        );

        let session_workspaces = db
            .last_session_workspace_locations("pending-removal-session", None, fs.as_ref())
            .await
            .unwrap();
        let restored_ids: Vec<WorkspaceId> = session_workspaces
            .iter()
            .map(|sw| sw.workspace_id)
            .collect();
        assert!(
            !restored_ids.contains(&workspace2_db_id),
            "Pending removal task should have cleared the session binding"
        );
    }

    #[gpui::test]
    async fn test_create_workspace_bounds_observer_uses_fresh_id(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let project = Project::test(fs.clone(), [], cx).await;

        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        multi_workspace.update_in(cx, |mw, _, cx| {
            mw.set_random_database_id(cx);
        });

        let task =
            multi_workspace.update_in(cx, |mw, window, cx| mw.create_test_workspace(window, cx));
        task.await;

        let new_workspace_db_id =
            multi_workspace.read_with(cx, |mw, cx| mw.workspace().read(cx).database_id());
        assert!(
            new_workspace_db_id.is_some(),
            "After run_until_parked, the workspace should have a database_id"
        );

        let workspace_id = new_workspace_db_id.unwrap();

        let db = cx.update(|_, cx| WorkspaceDb::global(cx));

        assert!(
            db.workspace_for_id(workspace_id).is_some(),
            "The workspace row should exist in the DB"
        );

        cx.simulate_resize(gpui::size(px(1024.0), px(768.0)));

        // Advance the clock past the 100ms debounce timer so the bounds
        // observer task fires
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();

        let serialized = db
            .workspace_for_id(workspace_id)
            .expect("workspace row should still exist");
        assert!(
            serialized.window_bounds.is_some(),
            "The bounds observer should write bounds for the workspace's real DB ID, \
             even when the workspace was created via create_workspace (where the ID \
             is assigned asynchronously after construction)."
        );
    }

    #[gpui::test]
    async fn test_flush_serialization_writes_bounds(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let dir = tempfile::TempDir::with_prefix("flush_bounds_test").unwrap();
        fs.insert_tree(dir.path(), json!({})).await;

        let project = Project::test(fs.clone(), [dir.path()], cx).await;

        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let db = cx.update(|_, cx| WorkspaceDb::global(cx));
        let workspace_id = db.next_id().await.unwrap();
        multi_workspace.update_in(cx, |mw, _, cx| {
            mw.workspace().update(cx, |ws, _cx| {
                ws.set_database_id(workspace_id);
            });
        });

        let task = multi_workspace.update_in(cx, |mw, window, cx| {
            mw.workspace()
                .update(cx, |ws, cx| ws.flush_serialization(window, cx))
        });
        task.await;

        let after = db
            .workspace_for_id(workspace_id)
            .expect("workspace row should exist after flush_serialization");
        assert!(
            !after.paths.is_empty(),
            "flush_serialization should have written paths via save_workspace"
        );
        assert!(
            after.window_bounds.is_some(),
            "flush_serialization should ensure window bounds are persisted to the DB \
             before the process exits."
        );
    }

    #[gpui::test]
    async fn test_recent_workspace_identity_deduplication(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());

        // Main repo with a linked worktree entry
        fs.insert_tree(
            "/repo",
            json!({
                ".git": {
                    "worktrees": {
                        "feature": {
                            "commondir": "../../",
                            "HEAD": "ref: refs/heads/feature"
                        }
                    }
                },
                "src": { "main.rs": "" }
            }),
        )
        .await;

        // Linked worktree checkout pointing back to /repo
        fs.insert_tree(
            "/worktree",
            json!({
                ".git": "gitdir: /repo/.git/worktrees/feature",
                "src": { "main.rs": "" }
            }),
        )
        .await;

        // A plain non-git project
        fs.insert_tree(
            "/plain-project",
            json!({
                "src": { "main.rs": "" }
            }),
        )
        .await;

        // Another normal git repo (used in mixed-path entry)
        fs.insert_tree(
            "/other-repo",
            json!({
                ".git": {},
                "src": { "lib.rs": "" }
            }),
        )
        .await;

        let t0 = Utc::now() - chrono::Duration::hours(4);
        let t1 = Utc::now() - chrono::Duration::hours(3);
        let t2 = Utc::now() - chrono::Duration::hours(2);
        let t3 = Utc::now() - chrono::Duration::hours(1);

        let workspaces = vec![
            local_recent_workspace(WorkspaceId(1), PathList::new(&["/repo"]), t0, fs.as_ref())
                .await,
            local_recent_workspace(
                WorkspaceId(2),
                PathList::new(&["/worktree"]),
                t1,
                fs.as_ref(),
            )
            .await,
            local_recent_workspace(
                WorkspaceId(3),
                PathList::new(&["/other-repo", "/worktree"]),
                t2,
                fs.as_ref(),
            )
            .await,
            local_recent_workspace(
                WorkspaceId(4),
                PathList::new(&["/plain-project"]),
                t3,
                fs.as_ref(),
            )
            .await,
        ];

        let result = dedupe_recent_workspaces(workspaces);

        // Should have 3 entries: #1 and #2 deduped into one, plus #3 and #4.
        assert_eq!(result.len(), 3);

        // First entry: /repo — deduplicated from #1 and #2.
        // Keeps the position of #1 (first seen), but with #2's later timestamp.
        assert_eq!(result[0].identity_paths.paths(), &[PathBuf::from("/repo")]);
        assert_eq!(result[0].timestamp, t1);

        // Second entry: mixed-path workspace with worktree resolved.
        // /worktree → /repo, so paths become [/other-repo, /repo] (sorted).
        assert_eq!(
            result[1].identity_paths.paths(),
            &[PathBuf::from("/other-repo"), PathBuf::from("/repo")]
        );
        assert_eq!(result[1].workspace_id, WorkspaceId(3));

        // Third entry: non-git project, unchanged.
        assert_eq!(
            result[2].identity_paths.paths(),
            &[PathBuf::from("/plain-project")]
        );
        assert_eq!(result[2].workspace_id, WorkspaceId(4));
    }

    #[gpui::test]
    async fn test_recent_workspace_identity_for_bare_repo(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());

        // Bare repo at /foo/.bare (commondir doesn't end with .git)
        fs.insert_tree(
            "/foo/.bare",
            json!({
                "worktrees": {
                    "my-feature": {
                        "commondir": "../../",
                        "HEAD": "ref: refs/heads/my-feature"
                    }
                }
            }),
        )
        .await;

        // Linked worktree whose commondir resolves to a bare repo (/foo/.bare)
        fs.insert_tree(
            "/foo/my-feature",
            json!({
                ".git": "gitdir: /foo/.bare/worktrees/my-feature",
                "src": { "main.rs": "" }
            }),
        )
        .await;

        let t0 = Utc::now();

        let result = local_recent_workspace(
            WorkspaceId(1),
            PathList::new(&["/foo/my-feature"]),
            t0,
            fs.as_ref(),
        )
        .await;

        // Bare-backed worktrees should resolve to the repo identity path, which
        // is the parent directory users think of as the project root.
        assert_eq!(result.identity_paths.paths(), &[PathBuf::from("/foo")]);
    }

    #[gpui::test]
    async fn test_recent_workspace_identity_for_submodule(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());

        // Superproject `/Foo` with a submodule `Bar`. A submodule's `.git` is a
        // file pointing into the superproject's `.git/modules/<name>` directory,
        // structurally like a linked worktree's `.git` file.
        fs.insert_tree(
            "/Foo",
            json!({
                ".git": {
                    "modules": {
                        "Bar": {
                            "HEAD": "ref: refs/heads/main"
                        }
                    }
                },
                "Bar": {
                    ".git": "gitdir: ../.git/modules/Bar\n",
                    "src": { "main.rs": "" }
                },
                "src": { "lib.rs": "" }
            }),
        )
        .await;

        let t0 = Utc::now();

        let result = local_recent_workspace(
            WorkspaceId(1),
            PathList::new(&["/Foo/Bar"]),
            t0,
            fs.as_ref(),
        )
        .await;

        // Submodules are independent projects: their identity is their own
        // working directory, not the superproject's `.git/modules/<name>`.
        assert_eq!(result.identity_paths.paths(), &[PathBuf::from("/Foo/Bar")]);
    }

    #[gpui::test]
    async fn test_recent_workspace_identity_deduplicates_main_and_linked_worktree(
        cx: &mut gpui::TestAppContext,
    ) {
        let fs = fs::FakeFs::new(cx.executor());

        fs.insert_tree(
            "/the-project",
            json!({
                ".git": "gitdir: ./.bare\n",
                ".bare": {
                    "worktrees": {
                        "feature-a": {
                            "commondir": "../../",
                            "HEAD": "ref: refs/heads/feature-a"
                        }
                    }
                },
                "src": { "main.rs": "" }
            }),
        )
        .await;

        fs.insert_tree(
            "/the-project/feature-a",
            json!({
                ".git": "gitdir: ../.bare/worktrees/feature-a\n",
                "src": { "lib.rs": "" }
            }),
        )
        .await;

        let t0 = Utc::now() - chrono::Duration::hours(1);
        let t1 = Utc::now();
        let workspaces = vec![
            local_recent_workspace(
                WorkspaceId(1),
                PathList::new(&["/the-project"]),
                t0,
                fs.as_ref(),
            )
            .await,
            local_recent_workspace(
                WorkspaceId(2),
                PathList::new(&["/the-project/feature-a"]),
                t1,
                fs.as_ref(),
            )
            .await,
        ];

        let result = dedupe_recent_workspaces(workspaces);

        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].identity_paths.paths(),
            &[PathBuf::from("/the-project")]
        );
        assert_eq!(result[0].workspace_id, WorkspaceId(2));
        assert_eq!(result[0].timestamp, t1);
    }

    #[gpui::test]
    async fn test_recent_project_workspaces_preserve_reopen_paths(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());
        let db =
            WorkspaceDb::open_test_db("test_recent_project_workspaces_preserve_reopen_paths").await;

        fs.insert_tree(
            "/the-project",
            json!({
                ".git": "gitdir: ./.bare\n",
                ".bare": {
                    "worktrees": {
                        "feature-a": {
                            "commondir": "../../",
                            "HEAD": "ref: refs/heads/feature-a"
                        }
                    }
                },
                "src": { "main.rs": "" }
            }),
        )
        .await;

        fs.insert_tree(
            "/the-project/feature-a",
            json!({
                ".git": "gitdir: ../.bare/worktrees/feature-a\n",
                "src": { "lib.rs": "" }
            }),
        )
        .await;

        db.save_workspace(workspace_with(
            1,
            &[Path::new("/the-project")],
            empty_pane_group(),
            None,
        ))
        .await;
        db.save_workspace(workspace_with(
            2,
            &[Path::new("/the-project/feature-a")],
            empty_pane_group(),
            None,
        ))
        .await;
        db.set_timestamp_for_tests(WorkspaceId(1), "2024-01-01 00:00:00".to_owned())
            .await
            .unwrap();
        db.set_timestamp_for_tests(WorkspaceId(2), "2024-01-01 00:00:01".to_owned())
            .await
            .unwrap();

        let recents = db.recent_project_workspaces(fs.as_ref()).await.unwrap();

        assert_eq!(recents.len(), 1);
        assert_eq!(recents[0].workspace_id, WorkspaceId(2));
        assert_eq!(
            recents[0].paths.paths(),
            &[PathBuf::from("/the-project/feature-a")]
        );
        assert_eq!(
            recents[0].identity_paths.paths(),
            &[PathBuf::from("/the-project")]
        );
    }

    #[gpui::test]
    async fn test_recent_project_workspaces_remote_identity_hint(cx: &mut gpui::TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());
        let db =
            WorkspaceDb::open_test_db("test_recent_project_workspaces_remote_identity_hint").await;

        let workspace = remote_workspace_with(1, "example.com", &[Path::new("/repo/feature-a")]);
        db.save_workspace(SerializedWorkspace {
            identity_paths: Some(PathList::new(&["/repo"])),
            ..workspace
        })
        .await;

        let recents = db.recent_project_workspaces(fs.as_ref()).await.unwrap();

        assert_eq!(recents.len(), 1);
        assert_eq!(
            recents[0].paths.paths(),
            &[PathBuf::from("/repo/feature-a")]
        );
        assert_eq!(recents[0].identity_paths.paths(), &[PathBuf::from("/repo")]);
    }

    #[gpui::test]
    async fn test_recent_project_workspaces_remote_paths_do_not_use_local_fs_identity(
        cx: &mut gpui::TestAppContext,
    ) {
        let fs = fs::FakeFs::new(cx.executor());
        let db = WorkspaceDb::open_test_db(
            "test_recent_project_workspaces_remote_paths_do_not_use_local_fs_identity",
        )
        .await;

        fs.insert_tree(
            "/repo",
            json!({
                ".git": "gitdir: ./.bare\n",
                ".bare": {
                    "worktrees": {
                        "feature-a": {
                            "commondir": "../../",
                            "HEAD": "ref: refs/heads/feature-a"
                        }
                    }
                },
                "src": { "main.rs": "" }
            }),
        )
        .await;
        fs.insert_tree(
            "/repo/feature-a",
            json!({
                ".git": "gitdir: ../.bare/worktrees/feature-a\n",
                "src": { "lib.rs": "" }
            }),
        )
        .await;

        db.save_workspace(remote_workspace_with(
            1,
            "example.com",
            &[Path::new("/repo/feature-a")],
        ))
        .await;

        let recents = db.recent_project_workspaces(fs.as_ref()).await.unwrap();

        assert_eq!(recents.len(), 1);
        assert_eq!(
            recents[0].identity_paths.paths(),
            &[PathBuf::from("/repo/feature-a")]
        );
    }

    #[gpui::test]
    async fn test_recent_project_workspaces_do_not_dedupe_remote_hosts(
        cx: &mut gpui::TestAppContext,
    ) {
        let fs = fs::FakeFs::new(cx.executor());
        let db =
            WorkspaceDb::open_test_db("test_recent_project_workspaces_do_not_dedupe_remote_hosts")
                .await;

        db.save_workspace(remote_workspace_with(1, "host-a", &[Path::new("/repo")]))
            .await;
        db.save_workspace(remote_workspace_with(2, "host-b", &[Path::new("/repo")]))
            .await;
        db.set_timestamp_for_tests(WorkspaceId(1), "2024-01-01 00:00:00".to_owned())
            .await
            .unwrap();
        db.set_timestamp_for_tests(WorkspaceId(2), "2024-01-01 00:00:01".to_owned())
            .await
            .unwrap();

        let recents = db.recent_project_workspaces(fs.as_ref()).await.unwrap();

        assert_eq!(recents.len(), 2);
        assert_eq!(recents[0].workspace_id, WorkspaceId(2));
        assert_eq!(recents[1].workspace_id, WorkspaceId(1));
    }

    #[gpui::test]
    async fn test_delete_recent_workspace_group_removes_all_matching_rows(
        cx: &mut gpui::TestAppContext,
    ) {
        let fs = fs::FakeFs::new(cx.executor());
        let db = WorkspaceDb::open_test_db(
            "test_delete_recent_workspace_group_removes_all_matching_rows",
        )
        .await;

        fs.insert_tree(
            "/the-group",
            json!({
                ".git": "gitdir: ./.bare\n",
                ".bare": {
                    "worktrees": {
                        "feature-a": {
                            "commondir": "../../",
                            "HEAD": "ref: refs/heads/feature-a"
                        }
                    }
                },
                "src": { "main.rs": "" }
            }),
        )
        .await;

        fs.insert_tree(
            "/the-group/feature-a",
            json!({
                ".git": "gitdir: ../.bare/worktrees/feature-a\n",
                "src": { "lib.rs": "" }
            }),
        )
        .await;

        db.save_workspace(SerializedWorkspace {
            identity_paths: Some(PathList::new(&["/the-group"])),
            ..workspace_with(1, &[Path::new("/the-group")], empty_pane_group(), None)
        })
        .await;
        db.save_workspace(SerializedWorkspace {
            identity_paths: Some(PathList::new(&["/the-group"])),
            ..workspace_with(
                2,
                &[Path::new("/the-group/feature-a")],
                empty_pane_group(),
                None,
            )
        })
        .await;
        db.set_timestamp_for_tests(WorkspaceId(1), "2024-01-01 00:00:00".to_owned())
            .await
            .unwrap();
        db.set_timestamp_for_tests(WorkspaceId(2), "2024-01-01 00:00:01".to_owned())
            .await
            .unwrap();

        let recents = db.recent_project_workspaces(fs.as_ref()).await.unwrap();
        assert_eq!(recents.len(), 1);

        let deleted = db
            .delete_recent_workspace_group(&recents[0], &Default::default())
            .await
            .expect("failed to delete the unprotected recent workspace group");
        assert_eq!(deleted, vec![WorkspaceId(2), WorkspaceId(1)]);

        let recents = db
            .recent_project_workspaces(fs.as_ref())
            .await
            .expect("failed to load the protected recent workspace group");
        assert!(recents.is_empty());
    }

    #[gpui::test]
    async fn workspace_configuration_row_retention_preserves_reference_during_recent_deletion(
        cx: &mut gpui::TestAppContext,
    ) {
        let fs = fs::FakeFs::new(cx.executor());
        let db = WorkspaceDb::open_test_db("test_delete_recent_preserves_reference").await;
        fs.insert_tree("/the-group", json!({ ".git": {}, "file": "" }))
            .await;
        fs.insert_tree("/the-group-linked", json!({ ".git": {}, "file": "" }))
            .await;

        for (id, path) in [(1_u64, "/the-group"), (2, "/the-group-linked")] {
            db.save_workspace(SerializedWorkspace {
                identity_paths: Some(PathList::new(&["/the-group"])),
                ..workspace_with(id, &[Path::new(path)], empty_pane_group(), None)
            })
            .await;
        }

        let recents = db
            .recent_project_workspaces(fs.as_ref())
            .await
            .expect("failed to load the protected recent workspace group");
        let mut protected_workspace_ids = HashSet::default();
        protected_workspace_ids.insert(WorkspaceId(1));
        let deleted = db
            .delete_recent_workspace_group(&recents[0], &protected_workspace_ids)
            .await
            .expect("failed to delete the recent group around its protected workspace");

        assert_eq!(deleted, vec![WorkspaceId(2)]);
        assert!(db.workspace_for_id(WorkspaceId(1)).is_some());
        assert!(db.workspace_for_id(WorkspaceId(2)).is_none());
    }

    #[gpui::test]
    async fn workspace_configuration_row_retention_identity_fallback_requires_exact_local_row(
        _cx: &mut gpui::TestAppContext,
    ) {
        let db = WorkspaceDb::open_test_db("test_workspace_identity_fallback").await;
        db.save_workspace(SerializedWorkspace {
            identity_paths: Some(PathList::new(&["/repo"])),
            ..workspace_with(
                2,
                &[Path::new("/repo-linked")],
                pane_with_items(&[200]),
                None,
            )
        })
        .await;

        let fallback = db
            .workspace_for_local_identity_paths(&[PathBuf::from("/repo")])
            .expect("an existing exact row with the same identity should be usable");
        assert_eq!(fallback.id, WorkspaceId(2));
        assert!(db.workspace_for_local_identity_paths(&[]).is_none());
        assert!(
            db.workspace_for_local_identity_paths(&[PathBuf::from("/other")])
                .is_none()
        );
    }

    #[gpui::test]
    async fn test_restore_window_with_linked_worktree_and_multiple_project_groups(
        cx: &mut gpui::TestAppContext,
    ) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());

        // Main git repo at /repo
        fs.insert_tree(
            "/repo",
            json!({
                ".git": {
                    "HEAD": "ref: refs/heads/main",
                    "worktrees": {
                        "feature": {
                            "commondir": "../../",
                            "HEAD": "ref: refs/heads/feature"
                        }
                    }
                },
                "src": { "main.rs": "" }
            }),
        )
        .await;

        // Linked worktree checkout pointing back to /repo
        fs.insert_tree(
            "/worktree-feature",
            json!({
                ".git": "gitdir: /repo/.git/worktrees/feature",
                "src": { "lib.rs": "" }
            }),
        )
        .await;

        // --- Phase 1: Set up the original multi-workspace window ---

        let project_1 = Project::test(fs.clone(), ["/repo".as_ref()], cx).await;
        let project_1_linked_worktree =
            Project::test(fs.clone(), ["/worktree-feature".as_ref()], cx).await;

        // Wait for git discovery to finish.
        cx.run_until_parked();

        // Create a second, unrelated project so we have two distinct project groups.
        fs.insert_tree(
            "/other-project",
            json!({
                ".git": { "HEAD": "ref: refs/heads/main" },
                "readme.md": ""
            }),
        )
        .await;
        let project_2 = Project::test(fs.clone(), ["/other-project".as_ref()], cx).await;
        cx.run_until_parked();

        // Create the MultiWorkspace with project_2, then add the main repo
        // and its linked worktree. The linked worktree is added last and
        // becomes the active workspace.
        let (multi_workspace, cx) = cx
            .add_window_view(|window, cx| MultiWorkspace::test_new(project_2.clone(), window, cx));

        multi_workspace.update(cx, |mw, cx| {
            mw.open_sidebar(cx);
        });

        multi_workspace.update_in(cx, |mw, window, cx| {
            mw.test_add_workspace(project_1.clone(), window, cx);
        });

        let workspace_worktree = multi_workspace.update_in(cx, |mw, window, cx| {
            mw.test_add_workspace(project_1_linked_worktree.clone(), window, cx)
        });

        let tasks =
            multi_workspace.update_in(cx, |mw, window, cx| mw.flush_all_serialization(window, cx));
        cx.run_until_parked();
        for task in tasks {
            task.await;
        }
        cx.run_until_parked();

        let active_db_id = workspace_worktree.read_with(cx, |ws, _| ws.database_id());
        assert!(
            active_db_id.is_some(),
            "Active workspace should have a database ID"
        );

        // --- Phase 2: Read back and verify the serialized state ---

        let session_id = multi_workspace
            .read_with(cx, |mw, cx| mw.workspace().read(cx).session_id())
            .unwrap();
        let db = cx.update(|_, cx| WorkspaceDb::global(cx));
        let session_workspaces = db
            .last_session_workspace_locations(&session_id, None, fs.as_ref())
            .await
            .expect("should load session workspaces");
        assert!(
            !session_workspaces.is_empty(),
            "Should have at least one session workspace"
        );

        let multi_workspaces =
            cx.update(|_, cx| read_serialized_multi_workspaces(session_workspaces, cx));
        assert_eq!(
            multi_workspaces.len(),
            1,
            "All workspaces share one window, so there should be exactly one multi-workspace"
        );

        let serialized = &multi_workspaces[0];
        assert_eq!(
            serialized.active_workspace.workspace_id,
            active_db_id.unwrap(),
        );
        assert_eq!(serialized.state.project_groups.len(), 2,);

        // Verify the serialized project group keys round-trip back to the
        // originals.
        let restored_keys: Vec<ProjectGroupKey> = serialized
            .state
            .project_groups
            .iter()
            .cloned()
            .map(Into::into)
            .collect();
        let expected_keys = vec![
            ProjectGroupKey::new(None, PathList::new(&["/repo"])),
            ProjectGroupKey::new(None, PathList::new(&["/other-project"])),
        ];
        assert_eq!(
            restored_keys, expected_keys,
            "Deserialized project group keys should match the originals"
        );

        // --- Phase 3: Restore the window and verify the result ---

        let app_state =
            multi_workspace.read_with(cx, |mw, cx| mw.workspace().read(cx).app_state().clone());

        let serialized_mw = multi_workspaces.into_iter().next().unwrap();
        let restored_handle: gpui::WindowHandle<MultiWorkspace> = cx
            .update(|_, cx| {
                cx.spawn(async move |mut cx| {
                    crate::restore_multiworkspace(serialized_mw, app_state, &mut cx).await
                })
            })
            .await
            .expect("restore_multiworkspace should succeed");

        cx.run_until_parked();

        // The restored window should have the same project group keys.
        let restored_keys: Vec<ProjectGroupKey> = restored_handle
            .read_with(cx, |mw: &MultiWorkspace, _cx| mw.project_group_keys())
            .unwrap();
        assert_eq!(
            restored_keys, expected_keys,
            "Restored window should have the same project group keys as the original"
        );
        // The active workspace in the restored window should have the linked
        // worktree paths.
        let active_paths: Vec<PathBuf> = restored_handle
            .read_with(cx, |mw: &MultiWorkspace, cx| {
                mw.workspace()
                    .read(cx)
                    .root_paths(cx)
                    .into_iter()
                    .map(|p: Arc<Path>| p.to_path_buf())
                    .collect()
            })
            .unwrap();
        assert_eq!(
            active_paths,
            vec![PathBuf::from("/worktree-feature")],
            "The restored active workspace should be the linked worktree project"
        );
    }

    #[gpui::test]
    async fn test_remove_project_group_falls_back_to_neighbor(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let dir_a = unique_test_dir(&fs, "group-a").await;
        let dir_b = unique_test_dir(&fs, "group-b").await;
        let dir_c = unique_test_dir(&fs, "group-c").await;

        let project_a = Project::test(fs.clone(), [dir_a.as_path()], cx).await;
        let project_b = Project::test(fs.clone(), [dir_b.as_path()], cx).await;
        let project_c = Project::test(fs.clone(), [dir_c.as_path()], cx).await;

        // Create a multi-workspace with project A, then add B and C.
        // project_groups stores newest first: [C, B, A].
        // Sidebar displays in the same order: C (top), B (middle), A (bottom).
        let (multi_workspace, cx) = cx
            .add_window_view(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));

        multi_workspace.update(cx, |mw, cx| mw.open_sidebar(cx));

        let workspace_b = multi_workspace.update_in(cx, |mw, window, cx| {
            mw.test_add_workspace(project_b.clone(), window, cx)
        });
        let _workspace_c = multi_workspace.update_in(cx, |mw, window, cx| {
            mw.test_add_workspace(project_c.clone(), window, cx)
        });
        cx.run_until_parked();

        let key_a = project_a.read_with(cx, |p, cx| p.project_group_key(cx));
        let key_b = project_b.read_with(cx, |p, cx| p.project_group_key(cx));
        let key_c = project_c.read_with(cx, |p, cx| p.project_group_key(cx));

        // Activate workspace B so removing its group exercises the fallback.
        multi_workspace.update_in(cx, |mw, window, cx| {
            mw.activate(workspace_b.clone(), None, window, cx);
        });
        cx.run_until_parked();

        // --- Remove group B (the middle one). ---
        // In the sidebar [C, B, A], "below" B is A.
        multi_workspace.update_in(cx, |mw, window, cx| {
            mw.remove_project_group(&key_b, window, cx)
                .detach_and_log_err(cx);
        });
        cx.run_until_parked();

        let active_paths =
            multi_workspace.read_with(cx, |mw, cx| mw.workspace().read(cx).root_paths(cx));
        assert_eq!(
            active_paths
                .iter()
                .map(|p| p.to_path_buf())
                .collect::<Vec<_>>(),
            vec![dir_a.clone()],
            "After removing the middle group, should fall back to the group below (A)"
        );

        // After removing B, keys = [A, C], sidebar = [C, A].
        // Activate workspace A (the bottom) so removing it tests the
        // "fall back upward" path.
        let workspace_a =
            multi_workspace.read_with(cx, |mw, _cx| mw.workspaces().next().unwrap().clone());
        multi_workspace.update_in(cx, |mw, window, cx| {
            mw.activate(workspace_a.clone(), None, window, cx);
        });
        cx.run_until_parked();

        // --- Remove group A (the bottom one in sidebar). ---
        // Nothing below A, so should fall back upward to C.
        multi_workspace.update_in(cx, |mw, window, cx| {
            mw.remove_project_group(&key_a, window, cx)
                .detach_and_log_err(cx);
        });
        cx.run_until_parked();

        let active_paths =
            multi_workspace.read_with(cx, |mw, cx| mw.workspace().read(cx).root_paths(cx));
        assert_eq!(
            active_paths
                .iter()
                .map(|p| p.to_path_buf())
                .collect::<Vec<_>>(),
            vec![dir_c.clone()],
            "After removing the bottom group, should fall back to the group above (C)"
        );

        // --- Remove group C (the only one remaining). ---
        // Should create an empty workspace.
        multi_workspace.update_in(cx, |mw, window, cx| {
            mw.remove_project_group(&key_c, window, cx)
                .detach_and_log_err(cx);
        });
        cx.run_until_parked();

        let active_paths =
            multi_workspace.read_with(cx, |mw, cx| mw.workspace().read(cx).root_paths(cx));
        assert!(
            active_paths.is_empty(),
            "After removing the only remaining group, should have an empty workspace"
        );
    }

    /// Regression test for a crash where `find_or_create_local_workspace`
    /// returned a workspace that was about to be removed, hitting an assert
    /// in `MultiWorkspace::remove`.
    ///
    /// The scenario: two workspaces share the same root paths (e.g. due to
    /// a provisional key mismatch). When the first is removed and the
    /// fallback searches for the same paths, `workspace_for_paths` must
    /// skip the doomed workspace so the assert in `remove` is satisfied.
    #[gpui::test]
    async fn test_remove_fallback_skips_excluded_workspaces(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);

        let fs = fs::FakeFs::new(cx.executor());
        let dir = unique_test_dir(&fs, "shared").await;

        // Two projects that open the same directory — this creates two
        // workspaces whose root_paths are identical.
        let project_a = Project::test(fs.clone(), [dir.as_path()], cx).await;
        let project_b = Project::test(fs.clone(), [dir.as_path()], cx).await;

        let (multi_workspace, cx) = cx
            .add_window_view(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));

        multi_workspace.update(cx, |mw, cx| mw.open_sidebar(cx));

        let workspace_b = multi_workspace.update_in(cx, |mw, window, cx| {
            mw.test_add_workspace(project_b.clone(), window, cx)
        });
        cx.run_until_parked();

        // workspace_a is first in the workspaces vec.
        let workspace_a =
            multi_workspace.read_with(cx, |mw, _| mw.workspaces().next().cloned().unwrap());
        assert_ne!(workspace_a, workspace_b);

        // Activate workspace_a so removing it triggers the fallback path.
        multi_workspace.update_in(cx, |mw, window, cx| {
            mw.activate(workspace_a.clone(), None, window, cx);
        });
        cx.run_until_parked();

        // Remove workspace_a. Its replacement is looked up by the same paths,
        // so workspace_a itself has to be skipped or it would be picked again.
        multi_workspace.update_in(cx, |mw, window, cx| {
            mw.remove(
                vec![workspace_a.clone()],
                RemovalIntent::CloseProject,
                window,
                cx,
            )
            .detach_and_log_err(cx);
        });
        cx.run_until_parked();

        // workspace_b should now be active — workspace_a was removed.
        multi_workspace.read_with(cx, |mw, _cx| {
            assert_eq!(
                mw.workspace(),
                &workspace_b,
                "fallback should have found workspace_b, not the excluded workspace_a"
            );
        });
    }

    fn member(workspace_id: i64) -> model::WorkspaceConfigurationMember {
        model::WorkspaceConfigurationMember {
            workspace_id: WorkspaceId(workspace_id),
            identity_paths: vec![PathBuf::from(format!("/repo-{workspace_id}"))],
        }
    }

    #[derive(Clone, Copy)]
    struct WorkspaceConfigurationStoreTestHandle;

    struct WorkspaceConfigurationStoreMutationHandle;

    impl WorkspaceConfigurationStoreMutationHandle {
        fn mutate(
            &mut self,
            mutation: WorkspaceConfigurationMutation,
            cx: &mut App,
        ) -> Task<Result<StoreCommit>> {
            WorkspaceConfigurationStore::mutate_global(mutation, cx)
        }
    }

    impl WorkspaceConfigurationStoreTestHandle {
        fn update<R>(
            &self,
            cx: &mut gpui::TestAppContext,
            update: impl FnOnce(&mut WorkspaceConfigurationStoreMutationHandle, &mut App) -> R,
        ) -> R {
            cx.update(|cx| update(&mut WorkspaceConfigurationStoreMutationHandle, cx))
        }

        fn read_with<R>(
            &self,
            cx: &mut gpui::TestAppContext,
            read: impl FnOnce(&WorkspaceConfigurationStore, &App) -> R,
        ) -> R {
            cx.update(|cx| read(WorkspaceConfigurationStore::global(cx), cx))
        }
    }

    /// Writes a raw value straight into the configuration key, bypassing the store, so a
    /// test can set up what a different build would have left behind.
    async fn write_raw_configurations(cx: &mut gpui::TestAppContext, json: &str) {
        let kvp = cx.update(|cx| KeyValueStore::global(cx));
        kvp.scoped(WORKSPACE_CONFIGURATIONS_NAMESPACE)
            .write(WORKSPACE_CONFIGURATIONS_KEY.to_string(), json.to_string())
            .await
            .unwrap();
    }

    async fn read_raw_configurations(cx: &mut gpui::TestAppContext) -> Option<String> {
        let kvp = cx.update(|cx| KeyValueStore::global(cx));
        kvp.scoped(WORKSPACE_CONFIGURATIONS_NAMESPACE)
            .read(WORKSPACE_CONFIGURATIONS_KEY)
            .unwrap()
    }

    async fn clear_raw_configurations(cx: &mut gpui::TestAppContext) {
        let kvp = cx.update(|cx| KeyValueStore::global(cx));
        if let Err(error) = kvp
            .scoped(WORKSPACE_CONFIGURATIONS_NAMESPACE)
            .delete(WORKSPACE_CONFIGURATIONS_KEY.to_string())
            .await
        {
            panic!("failed to clear workspace configurations before test: {error:#}");
        }
    }

    async fn set_configuration_store_query_only(cx: &mut gpui::TestAppContext, query_only: bool) {
        let kvp = cx.update(|cx| KeyValueStore::global(cx));
        let query = if query_only {
            "PRAGMA query_only = ON"
        } else {
            "PRAGMA query_only = OFF"
        };
        let result = kvp
            .write(move |connection| connection.exec(query).and_then(|mut statement| statement()))
            .await;
        if let Err(error) = result {
            panic!("failed to change query_only while testing configuration writes: {error:#}");
        }
    }

    #[gpui::test]
    async fn workspace_configuration_store_round_trip(cx: &mut gpui::TestAppContext) {
        clear_raw_configurations(cx).await;
        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        let commit = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "  Daily  ".to_string(),
                        members: vec![member(1), member(2)],
                        active_member: Some(WorkspaceId(2)),
                    },
                    cx,
                )
            })
            .await;
        let commit = match commit {
            Ok(commit) => commit,
            Err(error) => panic!("failed to create the configuration test fixture: {error:#}"),
        };

        store.read_with(cx, |store, _| {
            let configuration = store.configuration(commit.id).unwrap();
            assert_eq!(configuration.name, "Daily", "the name is stored trimmed");
            assert_eq!(configuration.members, vec![member(1), member(2)]);
            assert_eq!(configuration.active_member, Some(WorkspaceId(2)));
            assert_eq!(store.generation(), 1);
        });

        // A checkpoint replaces membership, order, and active workspace together.
        store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Checkpoint {
                        id: commit.id,
                        members: vec![member(2), member(1), member(3)],
                        active_member: Some(WorkspaceId(3)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();

        store.read_with(cx, |store, _| {
            let configuration = store.configuration(commit.id).unwrap();
            assert_eq!(configuration.members, vec![member(2), member(1), member(3)]);
            assert_eq!(configuration.active_member, Some(WorkspaceId(3)));
            assert_eq!(store.generation(), 2, "each commit advances the generation");
        });

        // A store loaded fresh from the same value sees the checkpointed state, which is
        // what makes a configuration survive quit and relaunch.
        let reloaded = cx.update(|cx| cx.new(|cx| WorkspaceConfigurationStore::load(cx)));
        reloaded.read_with(cx, |store, _| {
            assert!(store.blocked().is_none());
            let configuration = store.configuration(commit.id).unwrap();
            assert_eq!(configuration.name, "Daily");
            assert_eq!(configuration.members, vec![member(2), member(1), member(3)]);
            assert_eq!(configuration.active_member, Some(WorkspaceId(3)));
        });
    }

    #[gpui::test]
    async fn workspace_configuration_store_rejects_invalid_names(cx: &mut gpui::TestAppContext) {
        clear_raw_configurations(cx).await;
        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Daily".to_string(),
                        members: vec![member(1)],
                        active_member: Some(WorkspaceId(1)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();

        let before = read_raw_configurations(cx).await;

        let empty = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "   ".to_string(),
                        members: vec![member(2)],
                        active_member: None,
                    },
                    cx,
                )
            })
            .await;
        assert!(empty.is_err(), "a blank name is refused");

        // Case and surrounding whitespace do not make a name distinct.
        let duplicate = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: " daily ".to_string(),
                        members: vec![member(2)],
                        active_member: None,
                    },
                    cx,
                )
            })
            .await;
        assert!(duplicate.is_err(), "a duplicate name is refused");

        store.read_with(cx, |store, _| {
            assert_eq!(
                store.configurations().len(),
                1,
                "a refused create adds nothing"
            );
            assert_eq!(store.generation(), 1, "a refused create does not commit");
        });
        assert_eq!(
            read_raw_configurations(cx).await,
            before,
            "a refused create leaves the stored value byte-identical"
        );
    }

    #[gpui::test]
    async fn workspace_configuration_store_preserves_prior_value_when_write_fails(
        cx: &mut gpui::TestAppContext,
    ) {
        clear_raw_configurations(cx).await;
        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        let commit = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Daily".to_string(),
                        members: vec![member(1)],
                        active_member: Some(WorkspaceId(1)),
                    },
                    cx,
                )
            })
            .await;
        let commit = match commit {
            Ok(commit) => commit,
            Err(error) => panic!("failed to create the configuration test fixture: {error:#}"),
        };
        let before = read_raw_configurations(cx).await;

        set_configuration_store_query_only(cx, true).await;
        let checkpoint = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Checkpoint {
                        id: commit.id,
                        members: vec![member(2)],
                        active_member: Some(WorkspaceId(2)),
                    },
                    cx,
                )
            })
            .await;
        set_configuration_store_query_only(cx, false).await;

        assert!(
            checkpoint.is_err(),
            "the forced database failure is reported"
        );
        store.read_with(cx, |store, _| {
            let Some(configuration) = store.configuration(commit.id) else {
                panic!("the prior configuration disappeared after a failed write");
            };
            assert_eq!(configuration.members, vec![member(1)]);
            assert_eq!(configuration.active_member, Some(WorkspaceId(1)));
            assert_eq!(store.generation(), 1, "a failed write does not commit");
        });
        assert_eq!(
            read_raw_configurations(cx).await,
            before,
            "a failed write leaves the stored value byte-identical"
        );
    }

    #[gpui::test]
    async fn workspace_configuration_store_blocks_mutation_when_read_fails(
        cx: &mut gpui::TestAppContext,
    ) {
        clear_raw_configurations(cx).await;
        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Daily".to_string(),
                        members: vec![member(1)],
                        active_member: Some(WorkspaceId(1)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();
        let before = read_raw_configurations(cx).await;

        assert_eq!(
            stored_configurations_from_read_result(Err(anyhow::anyhow!(
                "forced workspace configuration read failure"
            ))),
            StoredConfigurations::ReadFailed
        );
        cx.update(|cx| {
            cx.set_global(WorkspaceConfigurationStore::new(
                Default::default(),
                Some(StoreBlock::ReadFailed),
            ));
        });

        let mutation = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Replacement".to_string(),
                        members: vec![member(2)],
                        active_member: Some(WorkspaceId(2)),
                    },
                    cx,
                )
            })
            .await;

        assert!(mutation.is_err(), "a read failure blocks every mutation");
        assert_eq!(
            read_raw_configurations(cx).await,
            before,
            "a later successful write cannot erase the unread value"
        );
    }

    #[gpui::test]
    async fn workspace_configuration_store_refuses_newer_schema(cx: &mut gpui::TestAppContext) {
        clear_raw_configurations(cx).await;
        // What a build one schema ahead of this one would have written.
        let newer = format!(
            r#"{{"schema_version":{},"configurations":[{{"id":"5f1d4e5c-0000-4000-8000-000000000001","name":"From The Future","members":[],"active_member":null}}]}}"#,
            model::WORKSPACE_CONFIGURATION_SCHEMA_VERSION + 1
        );
        write_raw_configurations(cx, &newer).await;

        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        store.read_with(cx, |store, _| {
            assert_eq!(
                store.blocked(),
                Some(&StoreBlock::WrittenByNewerBuild {
                    schema_version: model::WORKSPACE_CONFIGURATION_SCHEMA_VERSION + 1
                })
            );
            assert!(
                store.configurations().is_empty(),
                "a value this build cannot read is not presented as configurations"
            );
            assert!(
                store.referenced_workspace_ids().is_none(),
                "cleanup must stop when referenced workspace ids cannot be decoded"
            );
        });

        let refused = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Daily".to_string(),
                        members: vec![member(1)],
                        active_member: None,
                    },
                    cx,
                )
            })
            .await;
        assert!(refused.is_err(), "every mutation is refused while blocked");

        assert_eq!(
            read_raw_configurations(cx).await.as_deref(),
            Some(newer.as_str()),
            "rolling back to this build must not destroy the newer build's configurations"
        );
    }

    /// A newer build is the case most likely to have reshaped the value around the
    /// version field. It must still read as "newer", not as damaged, or the user is told
    /// to go read a log when what they actually need is to update.
    #[gpui::test]
    async fn workspace_configuration_store_reads_newer_schema_of_unknown_shape(
        cx: &mut gpui::TestAppContext,
    ) {
        clear_raw_configurations(cx).await;
        let newer = format!(
            r#"{{"schema_version":{},"groups":[{{"whatever":true}}],"renamed_everything":null}}"#,
            model::WORKSPACE_CONFIGURATION_SCHEMA_VERSION + 1
        );
        write_raw_configurations(cx, &newer).await;

        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        store.read_with(cx, |store, _| {
            assert_eq!(
                store.blocked(),
                Some(&StoreBlock::WrittenByNewerBuild {
                    schema_version: model::WORKSPACE_CONFIGURATION_SCHEMA_VERSION + 1
                }),
                "a newer value whose body this build cannot parse is newer, not corrupt"
            );
        });

        assert_eq!(
            read_raw_configurations(cx).await.as_deref(),
            Some(newer.as_str()),
            "and it is still left untouched"
        );
    }

    #[gpui::test]
    async fn workspace_configuration_store_refuses_corrupt_value(cx: &mut gpui::TestAppContext) {
        clear_raw_configurations(cx).await;
        let corrupt = "{ this is not a configuration collection";
        write_raw_configurations(cx, corrupt).await;

        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        store.read_with(cx, |store, _| {
            assert_eq!(store.blocked(), Some(&StoreBlock::Corrupt));
        });

        let refused = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Daily".to_string(),
                        members: vec![member(1)],
                        active_member: None,
                    },
                    cx,
                )
            })
            .await;
        assert!(refused.is_err(), "a corrupt value is not overwritten");

        assert_eq!(
            read_raw_configurations(cx).await.as_deref(),
            Some(corrupt),
            "a corrupt value is left intact for recovery rather than replaced"
        );
    }

    /// Two windows checkpointing different configurations at the same time must both
    /// survive. This is the lost-update case: whole-value replacement from a cached copy
    /// would let whichever wrote second erase the other.
    #[gpui::test]
    async fn workspace_configuration_store_orders_mutations_workspace_configuration_concurrency(
        cx: &mut gpui::TestAppContext,
    ) {
        clear_raw_configurations(cx).await;
        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        let daily = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Daily".to_string(),
                        members: vec![member(1)],
                        active_member: Some(WorkspaceId(1)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();

        let release = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Release".to_string(),
                        members: vec![member(2)],
                        active_member: Some(WorkspaceId(2)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();

        // Queue both checkpoints before awaiting either, so they are in flight together.
        let (first, second) = store.update(cx, |store, cx| {
            let first = store.mutate(
                WorkspaceConfigurationMutation::Checkpoint {
                    id: daily.id,
                    members: vec![member(1), member(3)],
                    active_member: Some(WorkspaceId(3)),
                },
                cx,
            );
            let second = store.mutate(
                WorkspaceConfigurationMutation::Checkpoint {
                    id: release.id,
                    members: vec![member(2), member(4)],
                    active_member: Some(WorkspaceId(4)),
                },
                cx,
            );
            (first, second)
        });

        first.await.unwrap();
        second.await.unwrap();

        store.read_with(cx, |store, _| {
            assert_eq!(
                store.configuration(daily.id).unwrap().members,
                vec![member(1), member(3)],
                "the first checkpoint survives the second"
            );
            assert_eq!(
                store.configuration(release.id).unwrap().members,
                vec![member(2), member(4)],
                "the second checkpoint applied to the collection the first committed"
            );
            assert_eq!(store.generation(), 4);
        });

        // The durable value agrees with memory, so a relaunch sees both checkpoints.
        let reloaded = cx.update(|cx| cx.new(|cx| WorkspaceConfigurationStore::load(cx)));
        reloaded.read_with(cx, |store, _| {
            assert_eq!(
                store.configuration(daily.id).unwrap().members,
                vec![member(1), member(3)]
            );
            assert_eq!(
                store.configuration(release.id).unwrap().members,
                vec![member(2), member(4)]
            );
        });
    }

    #[gpui::test]
    async fn workspace_configuration_shutdown_checkpoint_preserves_queued_mutation(
        cx: &mut gpui::TestAppContext,
    ) {
        clear_raw_configurations(cx).await;
        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        let daily = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Daily".to_string(),
                        members: vec![member(1)],
                        active_member: Some(WorkspaceId(1)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();
        let release = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Release".to_string(),
                        members: vec![member(2)],
                        active_member: Some(WorkspaceId(2)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();

        let (resume_sender, resume_receiver) = futures::channel::oneshot::channel();
        cx.update(|cx| {
            let blocker = cx.spawn(async move |_cx| {
                resume_receiver.await.ok();
            });
            cx.global_mut::<WorkspaceConfigurationStore>().queue_tail = Some(blocker);
        });

        let (queued_mutation, shutdown_checkpoint) = cx.update(|cx| {
            let queued_mutation = WorkspaceConfigurationStore::mutate_global(
                WorkspaceConfigurationMutation::Checkpoint {
                    id: daily.id,
                    members: vec![member(1), member(3)],
                    active_member: Some(WorkspaceId(3)),
                },
                cx,
            );
            let shutdown_checkpoint = WorkspaceConfigurationStore::checkpoint_after_for_shutdown(
                Task::ready(Ok(())),
                release.id,
                vec![member(2), member(4)],
                Some(WorkspaceId(4)),
                cx,
            );
            (queued_mutation, shutdown_checkpoint)
        });
        resume_sender.send(()).unwrap();

        queued_mutation
            .await
            .expect("shutdown must not cancel an already queued mutation");
        shutdown_checkpoint.await.unwrap();

        let reloaded = cx.update(|cx| cx.new(|cx| WorkspaceConfigurationStore::load(cx)));
        reloaded.read_with(cx, |store, _| {
            assert_eq!(
                store.configuration(daily.id).unwrap().members,
                vec![member(1), member(3)]
            );
            assert_eq!(
                store.configuration(release.id).unwrap().members,
                vec![member(2), member(4)]
            );
        });
    }

    #[gpui::test]
    async fn failed_shutdown_checkpoint_does_not_leak_into_later_write(
        cx: &mut gpui::TestAppContext,
    ) {
        clear_raw_configurations(cx).await;
        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        let daily = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Daily".to_string(),
                        members: vec![member(1)],
                        active_member: Some(WorkspaceId(1)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();
        let release = store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Release".to_string(),
                        members: vec![member(2)],
                        active_member: Some(WorkspaceId(2)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();

        let (failed_checkpoint, succeeding_checkpoint) = cx.update(|cx| {
            let failed_checkpoint = WorkspaceConfigurationStore::checkpoint_after_for_shutdown(
                Task::ready(Err(anyhow::anyhow!("forced exact checkpoint failure"))),
                daily.id,
                vec![member(1), member(3)],
                Some(WorkspaceId(3)),
                cx,
            );
            let succeeding_checkpoint = WorkspaceConfigurationStore::checkpoint_after_for_shutdown(
                Task::ready(Ok(())),
                release.id,
                vec![member(2), member(4)],
                Some(WorkspaceId(4)),
                cx,
            );
            (failed_checkpoint, succeeding_checkpoint)
        });

        assert!(failed_checkpoint.await.is_err());
        succeeding_checkpoint.await.unwrap();

        let reloaded = cx.update(|cx| cx.new(|cx| WorkspaceConfigurationStore::load(cx)));
        reloaded.read_with(cx, |store, _| {
            assert_eq!(
                store.configuration(daily.id).unwrap().members,
                vec![member(1)],
                "a failed shutdown snapshot never becomes committed state"
            );
            assert_eq!(
                store.configuration(release.id).unwrap().members,
                vec![member(2), member(4)]
            );
        });
    }

    #[gpui::test]
    async fn workspace_configuration_store_reports_referenced_rows(cx: &mut gpui::TestAppContext) {
        clear_raw_configurations(cx).await;
        cx.update(|cx| WorkspaceConfigurationStore::init(cx));
        let store = WorkspaceConfigurationStoreTestHandle;

        store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Daily".to_string(),
                        members: vec![member(1), member(2)],
                        active_member: Some(WorkspaceId(1)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();

        store
            .update(cx, |store, cx| {
                store.mutate(
                    WorkspaceConfigurationMutation::Create {
                        name: "Release".to_string(),
                        members: vec![member(2), member(3)],
                        active_member: Some(WorkspaceId(3)),
                    },
                    cx,
                )
            })
            .await
            .unwrap();

        store.read_with(cx, |store, _| {
            let referenced = store
                .referenced_workspace_ids()
                .expect("a readable store can report referenced rows");
            // Workspace 2 belongs to both, and is reported once.
            assert_eq!(referenced.len(), 3);
            for id in [WorkspaceId(1), WorkspaceId(2), WorkspaceId(3)] {
                assert!(
                    referenced.contains(&id),
                    "{id:?} is referenced and must survive Recent cleanup"
                );
            }
        });
    }

    #[gpui::test]
    async fn workspace_configuration_store_reserves_rows_until_publication_finishes(
        cx: &mut gpui::TestAppContext,
    ) {
        clear_raw_configurations(cx).await;
        cx.update(WorkspaceConfigurationStore::init);

        let reservation_id = cx.update(|cx| {
            WorkspaceConfigurationStore::reserve_workspace_rows(
                [WorkspaceId(41), WorkspaceId(42)],
                cx,
            )
        });
        cx.update(|cx| {
            let referenced = WorkspaceConfigurationStore::global(cx)
                .referenced_workspace_ids()
                .expect("a readable store can report reserved rows");
            assert_eq!(
                referenced,
                [WorkspaceId(41), WorkspaceId(42)]
                    .into_iter()
                    .collect::<HashSet<_>>()
            );
        });

        cx.update(|cx| WorkspaceConfigurationStore::release_workspace_rows(reservation_id, cx));
        cx.update(|cx| {
            assert_eq!(
                WorkspaceConfigurationStore::global(cx).referenced_workspace_ids(),
                Some(HashSet::default())
            );
        });
    }

    #[gpui::test]
    async fn workspace_row_deletion_rechecks_reservations_on_the_store_queue(
        cx: &mut gpui::TestAppContext,
    ) {
        clear_raw_configurations(cx).await;
        cx.update(WorkspaceConfigurationStore::init);
        let db = WorkspaceDb::open_test_db(
            "workspace_row_deletion_rechecks_reservations_on_the_store_queue",
        )
        .await;
        let workspace_id = WorkspaceId(41);
        db.save_workspace(workspace_with(
            41,
            &[Path::new("/reserved")],
            empty_pane_group(),
            None,
        ))
        .await;

        let reservation_id =
            cx.update(|cx| WorkspaceConfigurationStore::reserve_workspace_rows([workspace_id], cx));
        let delete_task = cx.update(|cx| {
            WorkspaceConfigurationStore::delete_unreferenced_workspace_rows_global(
                db.clone(),
                vec![workspace_id],
                cx,
            )
        });
        assert!(delete_task.await.unwrap().is_empty());
        assert!(db.workspace_for_id(workspace_id).is_some());

        cx.update(|cx| WorkspaceConfigurationStore::release_workspace_rows(reservation_id, cx));
        let delete_task = cx.update(|cx| {
            WorkspaceConfigurationStore::delete_unreferenced_workspace_rows_global(
                db.clone(),
                vec![workspace_id],
                cx,
            )
        });
        assert_eq!(delete_task.await.unwrap(), vec![workspace_id]);
        assert!(db.workspace_for_id(workspace_id).is_none());
    }
}
