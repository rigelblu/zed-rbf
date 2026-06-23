mod dev_container_suggest;
pub mod disconnected_overlay;
mod remote_connections;
mod remote_servers;
pub mod sidebar_recent_projects;
mod ssh_config;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};

use fs::Fs;

#[cfg(target_os = "windows")]
mod wsl_picker;

use remote::RemoteConnectionOptions;
pub use remote_connection::{RemoteConnectionModal, connect};
pub use remote_connections::{navigate_to_positions, open_remote_project};

use disconnected_overlay::DisconnectedOverlay;
use fuzzy_nucleo::{StringMatch, StringMatchCandidate, match_strings};
use gpui::{
    Action, AnyElement, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    Subscription, Task, TaskExt, WeakEntity, Window, actions, px,
};

use picker::{
    Picker, PickerDelegate, ScrollBehavior,
    highlighted_match_with_paths::{HighlightedMatch, HighlightedMatchWithPaths},
};
use project::{Worktree, git_store::Repository};
pub use remote_connections::RemoteSettings;
pub use remote_servers::RemoteServerProjects;
use settings::{DefaultOpenBehavior, Settings, WorktreeId, update_settings_file_with_completion};
use ui_input::ErasedEditor;
use workspace::ProjectGroupKey;

use dev_container::{DevContainerContext, find_devcontainer_configs};
use ui::{
    ButtonLike, ContextMenu, Divider, HighlightedLabel, KeyBinding, ListItem, ListItemSpacing,
    ListSubHeader, PopoverMenu, PopoverMenuHandle, TintColor, Tooltip, prelude::*,
};
use util::{ResultExt, paths::PathExt};
use workspace::{
    HistoryManager, ModalView, MultiWorkspace, OpenMode, OpenOptions, OpenVisible, PathList,
    RecentWorkspace, SerializedWorkspaceLocation, Toast, Workspace, WorkspaceDb, WorkspaceId,
    WorkspaceSettings,
    notifications::{DetachAndPromptErr, NotificationId},
    with_active_or_new_workspace,
};
use zed_actions::{OpenDevContainer, OpenRecent, OpenRemote};

actions!(
    recent_projects,
    [
        ToggleActionsMenu,
        RemoveSelected,
        AddToWorkspace,
        PinSelectedRecentProject,
        PinCurrentProject,
        UnpinCurrentProject,
        MovePinnedProjectUp,
        MovePinnedProjectDown,
    ]
);

#[derive(Clone, Debug)]
pub struct RecentProjectEntry {
    pub name: SharedString,
    pub full_path: SharedString,
    pub paths: Vec<PathBuf>,
    pub workspace_id: WorkspaceId,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug)]
struct OpenFolderEntry {
    worktree_id: WorktreeId,
    name: SharedString,
    path: PathBuf,
    branch: Option<SharedString>,
    is_active: bool,
    connection_options: Option<RemoteConnectionOptions>,
}

#[derive(Clone, Debug)]
struct PinnedProjectEntry {
    setting_path: String,
    path: PathBuf,
}

#[derive(Clone, Copy, Debug)]
enum PendingPinnedProjectSelection {
    PinnedProject(usize),
    FirstSelectable,
}

#[derive(Clone, Debug)]
enum ProjectPickerEntry {
    Header(SharedString),
    /// A currently open folder from the active workspace's "Current Folders" section.
    ///
    /// `index` points into `RecentProjectsDelegate::open_folders`, and `positions` stores the
    /// fuzzy-match highlight positions for rendering the folder name.
    OpenFolder {
        index: usize,
        positions: Vec<usize>,
    },
    /// A project group from the current window's "This Window" section.
    ///
    /// These entries come from `RecentProjectsDelegate::window_project_groups`, not from the
    /// recent-project database. Empty queries list every project group known to the current
    /// window; non-empty queries list matching project groups. Confirming one activates or loads
    /// that project group in the current window, while secondary confirm can move local project
    /// groups to a new window when multiple groups are available.
    ProjectGroup(StringMatch),
    /// A local project from the user-managed pinned project settings list.
    ///
    /// The match's `candidate_id` indexes into `RecentProjectsDelegate::pinned_projects`.
    PinnedProject(StringMatch),
    /// A workspace from the recent-project database's "Recent Projects" section.
    ///
    /// The match's `candidate_id` indexes into `RecentProjectsDelegate::workspaces`. Confirming
    /// one opens that recent workspace in either the current window or a new window, depending on
    /// whether the picker was invoked for new-window behavior and whether this was a primary or
    /// secondary confirm.
    RecentProject(StringMatch),
}

fn is_selectable_entry(entry: &ProjectPickerEntry) -> bool {
    matches!(
        entry,
        ProjectPickerEntry::OpenFolder { .. }
            | ProjectPickerEntry::ProjectGroup(_)
            | ProjectPickerEntry::PinnedProject(_)
            | ProjectPickerEntry::RecentProject(_)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectPickerStyle {
    Modal,
    Popover,
}

#[derive(Clone, Copy)]
enum PinnedProjectMoveDirection {
    Up,
    Down,
}

const PINNED_PROJECT_REQUIRES_LOCAL_SINGLE_ROOT_TOAST: &str =
    "pinned-project-requires-local-single-root";

fn expand_pinned_project_path(path: &str) -> PathBuf {
    let env_expanded =
        shellexpand::env_with_context_no_errors(path, |variable| std::env::var(variable).ok());
    let tilde_expanded = shellexpand::tilde(env_expanded.as_ref());
    PathBuf::from(tilde_expanded.as_ref())
}

fn pinned_project_entry(setting_path: String) -> PinnedProjectEntry {
    let path = expand_pinned_project_path(&setting_path);
    PinnedProjectEntry { setting_path, path }
}

fn pinned_project_entries(cx: &App) -> Vec<PinnedProjectEntry> {
    WorkspaceSettings::get_global(cx)
        .pinned_projects
        .iter()
        .cloned()
        .map(pinned_project_entry)
        .collect()
}

fn pinned_project_setting_matches(setting_path: &str, project_path: &Path) -> bool {
    expand_pinned_project_path(setting_path) == project_path
}

fn path_list_matches_pinned_project(paths: &PathList, pinned_path: &Path) -> bool {
    let mut paths = paths.paths().iter();
    matches!((paths.next(), paths.next()), (Some(path), None) if path == pinned_path)
}

fn recent_workspace_pinnable_project_path(workspace: &RecentWorkspace) -> Option<PathBuf> {
    if !matches!(workspace.location, SerializedWorkspaceLocation::Local) {
        return None;
    }

    let mut paths = workspace.identity_paths.paths().iter();
    let path = paths.next()?;
    if paths.next().is_some() {
        return None;
    }

    Some(path.clone())
}

fn set_pinned_project_settings(
    settings: &mut settings::SettingsContent,
    pinned_projects: Vec<String>,
) {
    settings.workspace.pinned_projects = (!pinned_projects.is_empty()).then_some(pinned_projects);
}

fn update_pinned_project_settings(
    fs: Arc<dyn Fs>,
    window: &mut Window,
    cx: &mut App,
    update: impl 'static + Send + FnOnce(&mut settings::SettingsContent, &App),
) {
    let update = update_settings_file_with_completion(fs, cx, update);
    window
        .spawn(cx, async move |_| match update.await {
            Ok(Ok(())) => anyhow::Ok(()),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(anyhow::anyhow!("settings update task was canceled")),
        })
        .detach_and_prompt_err("Failed to update pinned projects", window, cx, |_, _, _| {
            None
        });
}

fn pin_project_path(pinned_projects: &mut Vec<String>, project_path: &Path) -> bool {
    if pinned_projects
        .iter()
        .any(|setting_path| pinned_project_setting_matches(setting_path, project_path))
    {
        return false;
    }

    pinned_projects.push(project_path.compact().to_string_lossy().to_string());
    true
}

fn unpin_project_path(pinned_projects: &mut Vec<String>, project_path: &Path) -> bool {
    let original_len = pinned_projects.len();
    pinned_projects.retain(|setting_path| {
        !pinned_project_setting_matches(setting_path.as_str(), project_path)
    });
    pinned_projects.len() != original_len
}

fn move_pinned_project(
    pinned_projects: &mut [String],
    index: usize,
    direction: PinnedProjectMoveDirection,
) -> bool {
    if index >= pinned_projects.len() {
        return false;
    }

    let Some(target_index) = (match direction {
        PinnedProjectMoveDirection::Up => index.checked_sub(1),
        PinnedProjectMoveDirection::Down => {
            let next_index = index.saturating_add(1);
            (next_index < pinned_projects.len()).then_some(next_index)
        }
    }) else {
        return false;
    };

    pinned_projects.swap(index, target_index);
    true
}

fn current_local_single_root_project_path(workspace: &Workspace, cx: &App) -> Option<PathBuf> {
    let project = workspace.project().read(cx);
    if !project.is_local() {
        return None;
    }

    let mut worktrees = project.visible_worktrees(cx);
    let worktree = worktrees.next()?;
    if worktrees.next().is_some() {
        return None;
    }

    Some(worktree.read(cx).abs_path().to_path_buf())
}

fn show_pinned_project_requires_local_single_root_toast(
    workspace: &mut Workspace,
    cx: &mut Context<Workspace>,
) {
    workspace.show_toast(
        Toast::new(
            NotificationId::named(PINNED_PROJECT_REQUIRES_LOCAL_SINGLE_ROOT_TOAST.into()),
            "Pinned projects require a local single-folder project",
        )
        .autohide(),
        cx,
    );
}

pub async fn get_recent_projects(
    current_workspace_id: Option<WorkspaceId>,
    limit: Option<usize>,
    fs: Arc<dyn fs::Fs>,
    db: &WorkspaceDb,
) -> Vec<RecentProjectEntry> {
    let workspaces = db
        .recent_project_workspaces(fs.as_ref())
        .await
        .unwrap_or_default();

    let filtered: Vec<_> = workspaces
        .into_iter()
        .filter(|workspace| Some(workspace.workspace_id) != current_workspace_id)
        .filter(|workspace| matches!(workspace.location, SerializedWorkspaceLocation::Local))
        .collect();

    let mut all_paths: Vec<PathBuf> = filtered
        .iter()
        .flat_map(|workspace| workspace.identity_paths.paths().iter().cloned())
        .collect();
    all_paths.sort_unstable();
    all_paths.dedup();
    let path_details =
        util::disambiguate::compute_disambiguation_details(&all_paths, |path, detail| {
            project::path_suffix(path, detail)
        });
    let path_detail_map: std::collections::HashMap<PathBuf, usize> =
        all_paths.into_iter().zip(path_details).collect();

    let entries: Vec<RecentProjectEntry> = filtered
        .into_iter()
        .map(|workspace| {
            let paths: Vec<PathBuf> = workspace.paths.paths().to_vec();
            let ordered_paths: Vec<&PathBuf> = workspace.identity_paths.ordered_paths().collect();

            let name = ordered_paths
                .iter()
                .map(|p| {
                    let detail = path_detail_map.get(*p).copied().unwrap_or(0);
                    project::path_suffix(p, detail)
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ");

            let full_path = ordered_paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n");

            RecentProjectEntry {
                name: SharedString::from(name),
                full_path: SharedString::from(full_path),
                paths,
                workspace_id: workspace.workspace_id,
                timestamp: workspace.timestamp,
            }
        })
        .collect();

    match limit {
        Some(n) => entries.into_iter().take(n).collect(),
        None => entries,
    }
}

pub async fn delete_recent_project(workspace_id: WorkspaceId, db: &WorkspaceDb) {
    let _ = db.delete_workspace_by_id(workspace_id).await;
}

fn get_open_folders(workspace: &Workspace, cx: &App) -> Vec<OpenFolderEntry> {
    let project = workspace.project().read(cx);
    let connection_options = project.remote_connection_options(cx);
    let visible_worktrees: Vec<_> = project.visible_worktrees(cx).collect();

    if visible_worktrees.len() <= 1 {
        return Vec::new();
    }

    let active_worktree_id = if let Some(repo) = project.active_repository(cx) {
        let repo = repo.read(cx);
        let repo_path = &repo.work_directory_abs_path;
        project.visible_worktrees(cx).find_map(|worktree| {
            let worktree_path = worktree.read(cx).abs_path();
            (worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref()))
                .then(|| worktree.read(cx).id())
        })
    } else {
        project
            .visible_worktrees(cx)
            .next()
            .map(|wt| wt.read(cx).id())
    };

    let mut all_paths: Vec<PathBuf> = visible_worktrees
        .iter()
        .map(|wt| wt.read(cx).abs_path().to_path_buf())
        .collect();
    all_paths.sort_unstable();
    all_paths.dedup();
    let path_details =
        util::disambiguate::compute_disambiguation_details(&all_paths, |path, detail| {
            project::path_suffix(path, detail)
        });
    let path_detail_map: std::collections::HashMap<PathBuf, usize> =
        all_paths.into_iter().zip(path_details).collect();

    let git_store = project.git_store().read(cx);
    let repositories: Vec<_> = git_store.repositories().values().cloned().collect();

    let mut entries: Vec<OpenFolderEntry> = visible_worktrees
        .into_iter()
        .map(|worktree| {
            let worktree_ref = worktree.read(cx);
            let worktree_id = worktree_ref.id();
            let path = worktree_ref.abs_path().to_path_buf();
            let detail = path_detail_map.get(&path).copied().unwrap_or(0);
            let name = SharedString::from(project::path_suffix(&path, detail));
            let branch = get_branch_for_worktree(worktree_ref, &repositories, cx);
            let is_active = active_worktree_id == Some(worktree_id);
            OpenFolderEntry {
                worktree_id,
                name,
                path,
                branch,
                is_active,
                connection_options: connection_options.clone(),
            }
        })
        .collect();

    entries.sort_by_key(|entry| entry.name.to_lowercase());
    entries
}

fn get_branch_for_worktree(
    worktree: &Worktree,
    repositories: &[Entity<Repository>],
    cx: &App,
) -> Option<SharedString> {
    let worktree_abs_path = worktree.abs_path();
    repositories
        .iter()
        .filter(|repo| {
            let repo_path = &repo.read(cx).work_directory_abs_path;
            *repo_path == worktree_abs_path || worktree_abs_path.starts_with(repo_path.as_ref())
        })
        .max_by_key(|repo| repo.read(cx).work_directory_abs_path.as_os_str().len())
        .and_then(|repo| {
            repo.read(cx)
                .branch
                .as_ref()
                .map(|branch| SharedString::from(branch.name().to_string()))
        })
}

pub(crate) fn default_open_in_new_window(cx: &App) -> bool {
    matches!(
        workspace::WorkspaceSettings::get_global(cx).default_open_behavior,
        DefaultOpenBehavior::NewWindow
    )
}

pub fn init(cx: &mut App) {
    #[cfg(target_os = "windows")]
    cx.on_action(|open_wsl: &zed_actions::wsl_actions::OpenFolderInWsl, cx| {
        let create_new_window = open_wsl
            .create_new_window
            .unwrap_or_else(|| default_open_in_new_window(cx));
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            use gpui::PathPromptOptions;
            use project::DirectoryLister;

            let paths = workspace.prompt_for_open_path(
                PathPromptOptions {
                    files: true,
                    directories: true,
                    multiple: false,
                    prompt: None,
                },
                DirectoryLister::Local(
                    workspace.project().clone(),
                    workspace.app_state().fs.clone(),
                ),
                window,
                cx,
            );

            let app_state = workspace.app_state().clone();
            let window_handle = window.window_handle().downcast::<MultiWorkspace>();

            cx.spawn_in(window, async move |workspace, cx| {
                use util::paths::SanitizedPath;

                let Some(paths) = paths.await.log_err().flatten() else {
                    return;
                };

                let wsl_path = paths
                    .iter()
                    .find_map(util::paths::WslPath::from_path);

                if let Some(util::paths::WslPath { distro, path }) = wsl_path {
                    use remote::WslConnectionOptions;

                    let connection_options = RemoteConnectionOptions::Wsl(WslConnectionOptions {
                        distro_name: distro.to_string(),
                        user: None,
                    });

                    let requesting_window = match create_new_window {
                        false => window_handle,
                        true => None,
                    };

                    let open_options = workspace::OpenOptions {
                        requesting_window,
                        ..Default::default()
                    };

                    open_remote_project(connection_options, vec![path.into()], app_state, open_options, cx).await.log_err();
                    return;
                }

                let paths = paths
                    .into_iter()
                    .filter_map(|path| SanitizedPath::new(&path).local_to_wsl())
                    .collect::<Vec<_>>();

                if paths.is_empty() {
                    let message = indoc::indoc! { r#"
                        Invalid path specified when trying to open a folder inside WSL.

                        Please note that Zed currently does not support opening network share folders inside wsl.
                    "#};

                    let _ = cx.prompt(gpui::PromptLevel::Critical, "Invalid path", Some(&message), &["OK"]).await;
                    return;
                }

                workspace.update_in(cx, |workspace, window, cx| {
                    workspace.toggle_modal(window, cx, |window, cx| {
                        crate::wsl_picker::WslOpenModal::new(paths, create_new_window, window, cx)
                    });
                }).log_err();
            })
            .detach();
        });
    });

    #[cfg(target_os = "windows")]
    cx.on_action(|open_wsl: &zed_actions::wsl_actions::OpenWsl, cx| {
        let create_new_window = open_wsl
            .create_new_window
            .unwrap_or_else(|| default_open_in_new_window(cx));
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            let handle = cx.entity().downgrade();
            let fs = workspace.project().read(cx).fs().clone();
            workspace.toggle_modal(window, cx, |window, cx| {
                RemoteServerProjects::wsl(create_new_window, fs, window, handle, cx)
            });
        });
    });

    #[cfg(target_os = "windows")]
    cx.on_action(|open_wsl: &remote::OpenWslPath, cx| {
        let open_wsl = open_wsl.clone();
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            let fs = workspace.project().read(cx).fs().clone();
            add_wsl_distro(fs, &open_wsl.distro, cx);
            let requesting_window =
                match workspace::WorkspaceSettings::get_global(cx).default_open_behavior {
                    DefaultOpenBehavior::ExistingWindow => {
                        window.window_handle().downcast::<MultiWorkspace>()
                    }
                    DefaultOpenBehavior::NewWindow => None,
                };
            let open_options = OpenOptions {
                requesting_window,
                ..Default::default()
            };

            let app_state = workspace.app_state().clone();

            cx.spawn_in(window, async move |_, cx| {
                open_remote_project(
                    RemoteConnectionOptions::Wsl(open_wsl.distro.clone()),
                    open_wsl.paths,
                    app_state,
                    open_options,
                    cx,
                )
                .await
            })
            .detach();
        });
    });

    cx.on_action(|open_recent: &OpenRecent, cx| {
        let create_new_window = open_recent.create_new_window;

        match cx
            .active_window()
            .and_then(|w| w.downcast::<MultiWorkspace>())
        {
            Some(multi_workspace) => {
                cx.defer(move |cx| {
                    multi_workspace
                        .update(cx, |multi_workspace, window, cx| {
                            let window_project_groups: Vec<ProjectGroupKey> =
                                multi_workspace.project_group_keys();

                            let workspace = multi_workspace.workspace().clone();
                            workspace.update(cx, |workspace, cx| {
                                let Some(recent_projects) =
                                    workspace.active_modal::<RecentProjects>(cx)
                                else {
                                    let focus_handle = workspace.focus_handle(cx);
                                    RecentProjects::open(
                                        workspace,
                                        create_new_window,
                                        window_project_groups,
                                        window,
                                        focus_handle,
                                        cx,
                                    );
                                    return;
                                };

                                recent_projects.update(cx, |recent_projects, cx| {
                                    recent_projects
                                        .picker
                                        .update(cx, |picker, cx| picker.cycle_selection(window, cx))
                                });
                            });
                        })
                        .log_err();
                });
            }
            None => {
                with_active_or_new_workspace(cx, move |workspace, window, cx| {
                    let Some(recent_projects) = workspace.active_modal::<RecentProjects>(cx) else {
                        let focus_handle = workspace.focus_handle(cx);
                        RecentProjects::open(
                            workspace,
                            create_new_window,
                            Vec::new(),
                            window,
                            focus_handle,
                            cx,
                        );
                        return;
                    };

                    recent_projects.update(cx, |recent_projects, cx| {
                        recent_projects
                            .picker
                            .update(cx, |picker, cx| picker.cycle_selection(window, cx))
                    });
                });
            }
        }
    });
    cx.on_action(|open_remote: &OpenRemote, cx| {
        let from_existing_connection = open_remote.from_existing_connection;
        let create_new_window = open_remote
            .create_new_window
            .unwrap_or_else(|| default_open_in_new_window(cx));
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            if from_existing_connection {
                cx.propagate();
                return;
            }
            let handle = cx.entity().downgrade();
            let fs = workspace.project().read(cx).fs().clone();
            workspace.toggle_modal(window, cx, |window, cx| {
                RemoteServerProjects::new(create_new_window, fs, window, handle, cx)
            })
        });
    });

    cx.observe_new(DisconnectedOverlay::register).detach();

    cx.on_action(|_: &PinCurrentProject, cx| {
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            let Some(project_path) = current_local_single_root_project_path(workspace, cx) else {
                show_pinned_project_requires_local_single_root_toast(workspace, cx);
                return;
            };
            let fs = workspace.app_state().fs.clone();
            update_pinned_project_settings(fs, window, cx, move |settings, _| {
                let mut pinned_projects = settings
                    .workspace
                    .pinned_projects
                    .clone()
                    .unwrap_or_default();
                pin_project_path(&mut pinned_projects, &project_path);
                set_pinned_project_settings(settings, pinned_projects);
            });
        });
    });

    cx.on_action(|_: &UnpinCurrentProject, cx| {
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            let Some(project_path) = current_local_single_root_project_path(workspace, cx) else {
                show_pinned_project_requires_local_single_root_toast(workspace, cx);
                return;
            };
            let fs = workspace.app_state().fs.clone();
            update_pinned_project_settings(fs, window, cx, move |settings, _| {
                let mut pinned_projects = settings
                    .workspace
                    .pinned_projects
                    .clone()
                    .unwrap_or_default();
                unpin_project_path(&mut pinned_projects, &project_path);
                set_pinned_project_settings(settings, pinned_projects);
            });
        });
    });

    cx.on_action(|_: &OpenDevContainer, cx| {
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            if !workspace.project().read(cx).is_local() {
                cx.spawn_in(window, async move |_, cx| {
                    cx.prompt(
                        gpui::PromptLevel::Critical,
                        "Cannot open Dev Container from remote project",
                        None,
                        &["OK"],
                    )
                    .await
                    .ok();
                })
                .detach();
                return;
            }

            let fs = workspace.project().read(cx).fs().clone();
            let configs = find_devcontainer_configs(workspace, cx);
            let app_state = workspace.app_state().clone();
            let dev_container_context = DevContainerContext::from_workspace(workspace, cx);
            let handle = cx.entity().downgrade();
            workspace.toggle_modal(window, cx, |window, cx| {
                RemoteServerProjects::new_dev_container(
                    fs,
                    configs,
                    app_state,
                    dev_container_context,
                    window,
                    handle,
                    cx,
                )
            });
        });
    });

    // Subscribe to worktree additions to suggest opening the project in a dev container
    cx.observe_new(
        |workspace: &mut Workspace, window: Option<&mut Window>, cx: &mut Context<Workspace>| {
            let Some(window) = window else {
                return;
            };
            cx.subscribe_in(
                workspace.project(),
                window,
                move |workspace, project, event, window, cx| {
                    if let project::Event::WorktreeUpdatedEntries(worktree_id, updated_entries) =
                        event
                    {
                        dev_container_suggest::suggest_on_worktree_updated(
                            workspace,
                            *worktree_id,
                            updated_entries,
                            project,
                            window,
                            cx,
                        );
                    }
                },
            )
            .detach();
        },
    )
    .detach();
}

#[cfg(target_os = "windows")]
pub fn add_wsl_distro(
    fs: Arc<dyn project::Fs>,
    connection_options: &remote::WslConnectionOptions,
    cx: &App,
) {
    use gpui::ReadGlobal;
    use settings::SettingsStore;

    let distro_name = connection_options.distro_name.clone();
    let user = connection_options.user.clone();
    SettingsStore::global(cx).update_settings_file(fs, move |setting, _| {
        let connections = setting
            .remote
            .wsl_connections
            .get_or_insert(Default::default());

        if !connections
            .iter()
            .any(|conn| conn.distro_name == distro_name && conn.user == user)
        {
            use std::collections::BTreeSet;

            connections.push(settings::WslConnection {
                distro_name,
                user,
                projects: BTreeSet::new(),
            })
        }
    });
}

pub struct RecentProjects {
    pub picker: Entity<Picker<RecentProjectsDelegate>>,
    rem_width: f32,
    _subscriptions: Vec<Subscription>,
}

impl ModalView for RecentProjects {
    fn on_before_dismiss(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> workspace::DismissDecision {
        let submenu_focused = self.picker.update(cx, |picker, cx| {
            picker.delegate.actions_menu_handle.is_focused(window, cx)
        });
        workspace::DismissDecision::Dismiss(!submenu_focused)
    }
}

impl RecentProjects {
    fn new(
        delegate: RecentProjectsDelegate,
        fs: Option<Arc<dyn Fs>>,
        rem_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let style = delegate.style;
        let picker = cx.new(|cx| {
            Picker::list(delegate, window, cx)
                .list_measure_all()
                .show_scrollbar(true)
        });

        let picker_focus_handle = picker.focus_handle(cx);
        picker.update(cx, |picker, _| {
            picker.delegate.focus_handle = picker_focus_handle;
        });

        let mut subscriptions = vec![cx.subscribe(&picker, |_, _, _, cx| cx.emit(DismissEvent))];

        if style == ProjectPickerStyle::Popover {
            let picker_focus = picker.focus_handle(cx);
            subscriptions.push(
                cx.on_focus_out(&picker_focus, window, |this, _, window, cx| {
                    let submenu_focused = this.picker.update(cx, |picker, cx| {
                        picker.delegate.actions_menu_handle.is_focused(window, cx)
                    });
                    if !submenu_focused {
                        cx.emit(DismissEvent);
                    }
                }),
            );
        }
        // We do not want to block the UI on a potentially lengthy call to DB, so we're gonna swap
        // out workspace locations once the future runs to completion.
        let db = WorkspaceDb::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(fs) = fs else { return };
            let workspaces = db
                .recent_project_workspaces(fs.as_ref())
                .await
                .log_err()
                .unwrap_or_default();
            this.update_in(cx, move |this, window, cx| {
                this.picker.update(cx, move |picker, cx| {
                    picker.delegate.set_workspaces(workspaces);
                    picker.update_matches(picker.query(cx), window, cx)
                })
            })
            .ok();
        })
        .detach();
        Self {
            picker,
            rem_width,
            _subscriptions: subscriptions,
        }
    }

    pub fn open(
        workspace: &mut Workspace,
        create_new_window: Option<bool>,
        window_project_groups: Vec<ProjectGroupKey>,
        window: &mut Window,
        focus_handle: FocusHandle,
        cx: &mut Context<Workspace>,
    ) {
        let weak = cx.entity().downgrade();
        let open_folders = get_open_folders(workspace, cx);
        let fs = Some(workspace.app_state().fs.clone());

        let create_new_window = create_new_window.unwrap_or_else(|| default_open_in_new_window(cx));

        workspace.toggle_modal(window, cx, |window, cx| {
            let delegate = RecentProjectsDelegate::new(
                weak,
                create_new_window,
                focus_handle,
                open_folders,
                window_project_groups,
                ProjectPickerStyle::Modal,
                cx,
            );

            Self::new(delegate, fs, 34., window, cx)
        })
    }

    pub fn popover(
        workspace: WeakEntity<Workspace>,
        window_project_groups: Vec<ProjectGroupKey>,
        create_new_window: Option<bool>,
        focus_handle: FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let (open_folders, fs) = workspace
            .upgrade()
            .map(|workspace| {
                let workspace = workspace.read(cx);
                (
                    get_open_folders(workspace, cx),
                    Some(workspace.app_state().fs.clone()),
                )
            })
            .unwrap_or_else(|| (Vec::new(), None));

        let create_new_window = create_new_window.unwrap_or_else(|| default_open_in_new_window(cx));

        cx.new(|cx| {
            let delegate = RecentProjectsDelegate::new(
                workspace,
                create_new_window,
                focus_handle,
                open_folders,
                window_project_groups,
                ProjectPickerStyle::Popover,
                cx,
            );
            let list = Self::new(delegate, fs, 20., window, cx);
            list.picker.focus_handle(cx).focus(window, cx);
            list
        })
    }

    fn handle_toggle_open_menu(
        &mut self,
        _: &ToggleActionsMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let menu_handle = &picker.delegate.actions_menu_handle;
            if menu_handle.is_deployed() {
                menu_handle.hide(cx);
            } else {
                menu_handle.show(window, cx);
            }
        });
    }

    fn handle_remove_selected(
        &mut self,
        _: &RemoveSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let ix = picker.delegate.selected_index;
            let selected_entry = picker.delegate.filtered_entries.get(ix).cloned();

            match selected_entry {
                Some(ProjectPickerEntry::OpenFolder { index, .. }) => {
                    if let Some(folder) = picker.delegate.open_folders.get(index) {
                        let worktree_id = folder.worktree_id;
                        let Some(workspace) = picker.delegate.workspace.upgrade() else {
                            return;
                        };
                        workspace.update(cx, |workspace, cx| {
                            let project = workspace.project().clone();
                            project.update(cx, |project, cx| {
                                project.remove_worktree(worktree_id, cx);
                            });
                        });
                        picker.delegate.open_folders = get_open_folders(workspace.read(cx), cx);
                        let query = picker.query(cx);
                        picker.update_matches(query, window, cx);
                    }
                }
                Some(ProjectPickerEntry::ProjectGroup(hit)) => {
                    if let Some(key) = picker
                        .delegate
                        .window_project_groups
                        .get(hit.candidate_id)
                        .cloned()
                    {
                        if picker.delegate.is_active_project_group(&key, cx) {
                            return;
                        }
                        picker.delegate.remove_project_group(key, window, cx);
                        let query = picker.query(cx);
                        picker.update_matches(query, window, cx);
                    }
                }
                Some(ProjectPickerEntry::PinnedProject(hit)) => {
                    picker
                        .delegate
                        .remove_pinned_project(hit.candidate_id, window, cx);
                    let query = picker.query(cx);
                    picker.update_matches(query, window, cx);
                }
                Some(ProjectPickerEntry::RecentProject(_)) => {
                    picker.delegate.delete_recent_project(ix, window, cx);
                }
                _ => {}
            }
        });
    }

    fn handle_add_to_workspace(
        &mut self,
        _: &AddToWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let ix = picker.delegate.selected_index;

            if let Some(ProjectPickerEntry::RecentProject(hit)) =
                picker.delegate.filtered_entries.get(ix)
            {
                if let Some(workspace) = picker.delegate.workspaces.get(hit.candidate_id) {
                    if matches!(workspace.location, SerializedWorkspaceLocation::Local) {
                        let paths_to_add = workspace.paths.paths().to_vec();
                        picker
                            .delegate
                            .add_paths_to_project(paths_to_add, window, cx);
                    }
                }
            }
        });
    }

    fn handle_pin_selected_recent_project(
        &mut self,
        _: &PinSelectedRecentProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let ix = picker.delegate.selected_index;
            let Some(ProjectPickerEntry::RecentProject(hit)) =
                picker.delegate.filtered_entries.get(ix).cloned()
            else {
                return;
            };

            picker
                .delegate
                .pin_recent_project(hit.candidate_id, window, cx);
            let query = picker.query(cx);
            picker.update_matches(query, window, cx);
        });
    }

    fn handle_move_pinned_project_up(
        &mut self,
        _: &MovePinnedProjectUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_move_selected_pinned_project(PinnedProjectMoveDirection::Up, window, cx);
    }

    fn handle_move_pinned_project_down(
        &mut self,
        _: &MovePinnedProjectDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_move_selected_pinned_project(PinnedProjectMoveDirection::Down, window, cx);
    }

    fn handle_move_selected_pinned_project(
        &mut self,
        direction: PinnedProjectMoveDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let ix = picker.delegate.selected_index;
            let Some(ProjectPickerEntry::PinnedProject(hit)) =
                picker.delegate.filtered_entries.get(ix).cloned()
            else {
                return;
            };

            picker
                .delegate
                .move_pinned_project(hit.candidate_id, direction, window, cx);
            let query = picker.query(cx);
            picker.update_matches(query, window, cx);
        });
    }
}

impl EventEmitter<DismissEvent> for RecentProjects {}

impl Focusable for RecentProjects {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for RecentProjects {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("RecentProjects")
            .on_action(cx.listener(Self::handle_toggle_open_menu))
            .on_action(cx.listener(Self::handle_remove_selected))
            .on_action(cx.listener(Self::handle_add_to_workspace))
            .on_action(cx.listener(Self::handle_pin_selected_recent_project))
            .on_action(cx.listener(Self::handle_move_pinned_project_up))
            .on_action(cx.listener(Self::handle_move_pinned_project_down))
            .w(rems(self.rem_width))
            .child(self.picker.clone())
    }
}

pub struct RecentProjectsDelegate {
    workspace: WeakEntity<Workspace>,
    open_folders: Vec<OpenFolderEntry>,
    window_project_groups: Vec<ProjectGroupKey>,
    pinned_projects: Vec<PinnedProjectEntry>,
    workspaces: Vec<RecentWorkspace>,
    filtered_entries: Vec<ProjectPickerEntry>,
    selected_index: usize,
    render_paths: bool,
    create_new_window: bool,
    snap_selection_to_first_non_header_match: bool,
    pending_pinned_project_selection: Option<PendingPinnedProjectSelection>,
    focus_handle: FocusHandle,
    style: ProjectPickerStyle,
    actions_menu_handle: PopoverMenuHandle<ContextMenu>,
}

impl RecentProjectsDelegate {
    fn new(
        workspace: WeakEntity<Workspace>,
        create_new_window: bool,
        focus_handle: FocusHandle,
        open_folders: Vec<OpenFolderEntry>,
        window_project_groups: Vec<ProjectGroupKey>,
        style: ProjectPickerStyle,
        cx: &App,
    ) -> Self {
        let render_paths = style == ProjectPickerStyle::Modal;
        Self {
            workspace,
            open_folders,
            window_project_groups,
            pinned_projects: pinned_project_entries(cx),
            workspaces: Vec::new(),
            filtered_entries: Vec::new(),
            selected_index: 0,
            create_new_window,
            render_paths,
            snap_selection_to_first_non_header_match: true,
            pending_pinned_project_selection: None,
            focus_handle,
            style,
            actions_menu_handle: PopoverMenuHandle::default(),
        }
    }

    pub fn set_workspaces(&mut self, workspaces: Vec<RecentWorkspace>) {
        self.workspaces = workspaces;
    }

    fn filtered_entries_include_remote_project(&self) -> bool {
        self.filtered_entries
            .iter()
            .any(|entry| self.entry_is_remote_project(entry))
    }

    fn entry_is_remote_project(&self, entry: &ProjectPickerEntry) -> bool {
        match entry {
            ProjectPickerEntry::Header(_) => false,
            ProjectPickerEntry::OpenFolder { index, .. } => self
                .open_folders
                .get(*index)
                .is_some_and(|folder| folder.connection_options.is_some()),
            ProjectPickerEntry::PinnedProject(_) => false,
            ProjectPickerEntry::ProjectGroup(hit) => self
                .window_project_groups
                .get(hit.candidate_id)
                .is_some_and(|key| key.host().is_some()),
            ProjectPickerEntry::RecentProject(hit) => self
                .workspaces
                .get(hit.candidate_id)
                .is_some_and(|workspace| {
                    matches!(workspace.location, SerializedWorkspaceLocation::Remote(_))
                }),
        }
    }
}
impl EventEmitter<DismissEvent> for RecentProjectsDelegate {}
impl PickerDelegate for RecentProjectsDelegate {
    type ListItem = AnyElement;

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Search projects…".into()
    }

    fn render_editor(
        &self,
        editor: &Arc<dyn ErasedEditor>,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Div {
        h_flex()
            .flex_none()
            .h_9()
            .px_2p5()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(editor.render(window, cx))
    }

    fn match_count(&self) -> usize {
        self.filtered_entries.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    fn can_select(&self, ix: usize, _window: &mut Window, _cx: &mut Context<Picker<Self>>) -> bool {
        matches!(
            self.filtered_entries.get(ix),
            Some(
                ProjectPickerEntry::OpenFolder { .. }
                    | ProjectPickerEntry::ProjectGroup(_)
                    | ProjectPickerEntry::PinnedProject(_)
                    | ProjectPickerEntry::RecentProject(_)
            )
        )
    }

    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> gpui::Task<()> {
        let query = query.trim_start();
        let case = fuzzy_nucleo::Case::smart_if_uppercase_in(query);
        let is_empty_query = query.is_empty();

        let folder_matches = if self.open_folders.is_empty() {
            Vec::new()
        } else {
            let candidates: Vec<_> = self
                .open_folders
                .iter()
                .enumerate()
                .map(|(id, folder)| StringMatchCandidate::new(id, folder.name.as_ref()))
                .collect();

            match_strings(
                &candidates,
                query,
                case,
                fuzzy_nucleo::LengthPenalty::On,
                100,
            )
        };

        let project_group_candidates: Vec<_> = self
            .window_project_groups
            .iter()
            .enumerate()
            .map(|(id, key)| {
                let combined_string = key
                    .path_list()
                    .ordered_paths()
                    .map(|path| path.compact().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .concat();
                StringMatchCandidate::new(id, &combined_string)
            })
            .collect();

        let project_group_matches = match_strings(
            &project_group_candidates,
            query,
            case,
            fuzzy_nucleo::LengthPenalty::On,
            100,
        );

        let pinned_project_candidates: Vec<_> = self
            .pinned_projects
            .iter()
            .enumerate()
            .map(|(id, pinned_project)| {
                let path = pinned_project.path.compact().to_string_lossy().into_owned();
                StringMatchCandidate::new(id, &path)
            })
            .collect();

        let mut pinned_project_matches = match_strings(
            &pinned_project_candidates,
            query,
            case,
            fuzzy_nucleo::LengthPenalty::On,
            100,
        );
        pinned_project_matches.sort_by_key(|pinned_match| pinned_match.candidate_id);

        // Build candidates for recent projects (not current, not sibling, not open folder)
        let recent_candidates: Vec<_> = self
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, workspace)| self.is_valid_recent_candidate(workspace, cx))
            .map(|(id, workspace)| {
                let combined_string = workspace
                    .identity_paths
                    .ordered_paths()
                    .map(|path| path.compact().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .concat();
                StringMatchCandidate::new(id, &combined_string)
            })
            .collect();

        let recent_matches = match_strings(
            &recent_candidates,
            query,
            case,
            fuzzy_nucleo::LengthPenalty::On,
            100,
        );

        let mut entries = Vec::new();

        let has_pinned_projects_to_show = if is_empty_query {
            !pinned_project_candidates.is_empty()
        } else {
            !pinned_project_matches.is_empty()
        };

        if has_pinned_projects_to_show {
            entries.push(ProjectPickerEntry::Header("Pinned Projects".into()));

            if is_empty_query {
                for id in 0..self.pinned_projects.len() {
                    entries.push(ProjectPickerEntry::PinnedProject(StringMatch {
                        candidate_id: id,
                        score: 0.0,
                        positions: Vec::new(),
                        string: Default::default(),
                    }));
                }
            } else {
                for pinned_match in pinned_project_matches {
                    entries.push(ProjectPickerEntry::PinnedProject(pinned_match));
                }
            }
        }

        if !self.open_folders.is_empty() {
            let matched_folders: Vec<_> = if is_empty_query {
                (0..self.open_folders.len())
                    .map(|i| (i, Vec::new()))
                    .collect()
            } else {
                folder_matches
                    .iter()
                    .map(|m| (m.candidate_id, m.positions.clone()))
                    .collect()
            };

            if !matched_folders.is_empty() {
                entries.push(ProjectPickerEntry::Header("Current Folders".into()));
                for (index, positions) in matched_folders {
                    entries.push(ProjectPickerEntry::OpenFolder { index, positions });
                }
            }
        }

        let has_projects_to_show = if is_empty_query {
            !project_group_candidates.is_empty()
        } else {
            !project_group_matches.is_empty()
        };

        if has_projects_to_show {
            entries.push(ProjectPickerEntry::Header("This Window".into()));

            if is_empty_query {
                for id in 0..self.window_project_groups.len() {
                    entries.push(ProjectPickerEntry::ProjectGroup(StringMatch {
                        candidate_id: id,
                        score: 0.0,
                        positions: Vec::new(),
                        string: Default::default(),
                    }));
                }
            } else {
                for m in project_group_matches {
                    entries.push(ProjectPickerEntry::ProjectGroup(m));
                }
            }
        }

        let has_recent_to_show = if is_empty_query {
            !recent_candidates.is_empty()
        } else {
            !recent_matches.is_empty()
        };

        if has_recent_to_show {
            entries.push(ProjectPickerEntry::Header("Recent Projects".into()));

            if is_empty_query {
                for (id, workspace) in self.workspaces.iter().enumerate() {
                    if self.is_valid_recent_candidate(workspace, cx) {
                        entries.push(ProjectPickerEntry::RecentProject(StringMatch {
                            candidate_id: id,
                            score: 0.0,
                            positions: Vec::new(),
                            string: Default::default(),
                        }));
                    }
                }
            } else {
                for m in recent_matches {
                    entries.push(ProjectPickerEntry::RecentProject(m));
                }
            }
        }

        self.filtered_entries = entries;

        if let Some(selection) = self.pending_pinned_project_selection.take() {
            self.selected_index = match selection {
                PendingPinnedProjectSelection::PinnedProject(candidate_id) => self
                    .filtered_entries
                    .iter()
                    .position(|entry| {
                        matches!(
                            entry,
                            ProjectPickerEntry::PinnedProject(hit)
                                if hit.candidate_id == candidate_id
                        )
                    })
                    .or_else(|| self.first_selectable_entry_index())
                    .unwrap_or(0),
                PendingPinnedProjectSelection::FirstSelectable => {
                    self.first_selectable_entry_index().unwrap_or(0)
                }
            };
        } else if self.snap_selection_to_first_non_header_match {
            self.selected_index = self.first_selectable_entry_index().unwrap_or(0);
        }
        self.snap_selection_to_first_non_header_match = true;
        Task::ready(())
    }

    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        match self.filtered_entries.get(self.selected_index) {
            Some(ProjectPickerEntry::OpenFolder { index, .. }) => {
                let Some(folder) = self.open_folders.get(*index) else {
                    return;
                };
                let worktree_id = folder.worktree_id;
                if let Some(workspace) = self.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        let git_store = workspace.project().read(cx).git_store().clone();
                        git_store.update(cx, |git_store, cx| {
                            git_store.set_active_repo_for_worktree(worktree_id, cx);
                        });
                    });
                }
                cx.emit(DismissEvent);
            }
            Some(ProjectPickerEntry::ProjectGroup(selected_match)) => {
                let Some(key) = self.window_project_groups.get(selected_match.candidate_id) else {
                    return;
                };

                if secondary && key.host().is_none() && self.window_project_groups.len() >= 2 {
                    move_project_group_to_new_window(key, window, cx);
                    cx.emit(DismissEvent);
                    return;
                }

                let key = key.clone();
                if let Some(handle) = window.window_handle().downcast::<MultiWorkspace>() {
                    cx.defer(move |cx| {
                        // Try to activate an existing workspace for this project group
                        // first, so we preserve the actual worktree paths (which may
                        // differ from the main git worktree paths stored in the key).
                        if let Some(workspace) = handle
                            .update(cx, |multi_workspace, _window, cx| {
                                multi_workspace.last_active_workspace_for_group(&key, cx)
                            })
                            .log_err()
                            .flatten()
                        {
                            handle
                                .update(cx, |multi_workspace, window, cx| {
                                    multi_workspace.activate(workspace, None, window, cx);
                                })
                                .log_err();
                        } else {
                            let path_list = key.path_list().clone();
                            if let Some(task) = handle
                                .update(cx, |multi_workspace, window, cx| {
                                    multi_workspace.find_or_create_local_workspace(
                                        path_list,
                                        Some(key.clone()),
                                        &[],
                                        None,
                                        OpenMode::Activate,
                                        window,
                                        cx,
                                    )
                                })
                                .log_err()
                            {
                                task.detach_and_log_err(cx);
                            }
                        }
                    });
                }
                cx.emit(DismissEvent);
            }
            Some(ProjectPickerEntry::PinnedProject(selected_match)) => {
                let candidate_id = selected_match.candidate_id;
                self.open_pinned_project(candidate_id, secondary, window, cx);
            }
            Some(ProjectPickerEntry::RecentProject(selected_match)) => {
                let candidate_id = selected_match.candidate_id;
                self.open_recent_projects(candidate_id, secondary, window, cx);
            }
            _ => {}
        }
    }

    fn dismissed(&mut self, _window: &mut Window, _: &mut Context<Picker<Self>>) {}

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        let text = if self.workspaces.is_empty() && self.open_folders.is_empty() {
            "Recently opened projects will show up here".into()
        } else {
            "No matches".into()
        };
        Some(text)
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        match self.filtered_entries.get(ix)? {
            ProjectPickerEntry::Header(title) => Some(
                v_flex()
                    .w_full()
                    .gap_1()
                    .when(ix > 0, |this| this.mt_1().child(Divider::horizontal()))
                    .child(ListSubHeader::new(title.clone()).inset(true))
                    .into_any_element(),
            ),
            ProjectPickerEntry::OpenFolder { index, positions } => {
                let folder = self.open_folders.get(*index)?;
                let name = folder.name.clone();
                let path = folder.path.compact();
                let branch = folder.branch.clone();
                let is_active = folder.is_active;
                let worktree_id = folder.worktree_id;
                let positions = positions.clone();
                let show_path = self.style == ProjectPickerStyle::Modal;

                let secondary_actions = h_flex()
                    .gap_1()
                    .child(
                        IconButton::new(("remove-folder", worktree_id.to_usize()), IconName::Close)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Remove Folder from Project"))
                            .on_click(cx.listener(move |picker, _, window, cx| {
                                let Some(workspace) = picker.delegate.workspace.upgrade() else {
                                    return;
                                };
                                workspace.update(cx, |workspace, cx| {
                                    let project = workspace.project().clone();
                                    project.update(cx, |project, cx| {
                                        project.remove_worktree(worktree_id, cx);
                                    });
                                });
                                picker.delegate.open_folders =
                                    get_open_folders(workspace.read(cx), cx);
                                let query = picker.query(cx);
                                picker.update_matches(query, window, cx);
                            })),
                    )
                    .into_any_element();

                let icon = icon_for_remote_connection(folder.connection_options.as_ref());
                let show_icon = self.filtered_entries_include_remote_project();

                let tooltip_path: SharedString = path.to_string_lossy().to_string().into();
                let tooltip_branch = branch.clone();

                Some(
                    ListItem::new(ix)
                        .toggle_state(selected)
                        .inset(true)
                        .spacing(ListItemSpacing::Sparse)
                        .child(
                            h_flex()
                                .id("open_folder_item")
                                .w_full()
                                .min_w_0()
                                .gap_2p5()
                                .when(show_icon, |this| {
                                    this.child(Icon::new(icon).color(Color::Muted))
                                })
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(HighlightedLabel::new(
                                                    name.to_string(),
                                                    positions,
                                                ))
                                                .when_some(branch, |this, branch| {
                                                    this.child(
                                                        Label::new(branch)
                                                            .color(Color::Muted)
                                                            .truncate(),
                                                    )
                                                })
                                                .when(is_active, |this| {
                                                    this.child(
                                                        Icon::new(IconName::Check)
                                                            .size(IconSize::Small)
                                                            .color(Color::Accent),
                                                    )
                                                }),
                                        )
                                        .when(show_path, |this| {
                                            this.child(
                                                Label::new(path.to_string_lossy().to_string())
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted),
                                            )
                                        }),
                                )
                                .when(!show_path, |this| {
                                    this.tooltip(move |_, cx| {
                                        if let Some(branch) = tooltip_branch.clone() {
                                            Tooltip::with_meta(
                                                format!("{}/{}", name, branch),
                                                None,
                                                tooltip_path.clone(),
                                                cx,
                                            )
                                        } else {
                                            Tooltip::simple(tooltip_path.clone(), cx)
                                        }
                                    })
                                }),
                        )
                        .end_slot(secondary_actions)
                        .show_end_slot_on_hover()
                        .into_any_element(),
                )
            }
            ProjectPickerEntry::ProjectGroup(hit) => {
                let key = self.window_project_groups.get(hit.candidate_id)?;
                let is_active = self.is_active_project_group(key, cx);
                let paths = key.path_list();
                let ordered_paths: Vec<_> = paths
                    .ordered_paths()
                    .map(|p| p.compact().to_string_lossy().to_string())
                    .collect();
                let tooltip_path: SharedString = ordered_paths.join("\n").into();
                let icon = icon_for_project_group(key);
                let show_icon = self.filtered_entries_include_remote_project();

                let mut path_start_offset = 0;
                let (match_labels, path_highlights): (Vec<_>, Vec<_>) = paths
                    .ordered_paths()
                    .map(|p| p.compact())
                    .map(|path| {
                        let highlighted_text =
                            highlights_for_path(path.as_ref(), &hit.positions, path_start_offset);
                        path_start_offset += highlighted_text.1.text.len();
                        highlighted_text
                    })
                    .unzip();

                let highlighted_match = HighlightedMatchWithPaths {
                    prefix: None,
                    match_label: HighlightedMatch::join(match_labels.into_iter().flatten(), ", "),
                    paths: path_highlights,
                    active: is_active,
                };

                let project_group_key = key.clone();
                let is_local = key.host().is_none();
                let has_multiple_groups = self.window_project_groups.len() >= 2;
                let secondary_actions = h_flex()
                    .gap_0p5()
                    .when(is_local && has_multiple_groups, |this| {
                        this.child(
                            IconButton::new("move_to_new_window", IconName::ArrowUpRight)
                                .icon_size(IconSize::Small)
                                .tooltip({
                                    let focus_handle = self.focus_handle.clone();
                                    move |_, cx| {
                                        Tooltip::for_action_in(
                                            "Open in New Window",
                                            &menu::SecondaryConfirm,
                                            &focus_handle,
                                            cx,
                                        )
                                    }
                                })
                                .on_click({
                                    let project_group_key = project_group_key.clone();
                                    cx.listener(move |_picker, _, window, cx| {
                                        cx.stop_propagation();
                                        window.prevent_default();
                                        move_project_group_to_new_window(
                                            &project_group_key,
                                            window,
                                            cx,
                                        );
                                        cx.emit(DismissEvent);
                                    })
                                }),
                        )
                    })
                    .when(!is_active, |this| {
                        this.child(
                            IconButton::new("remove_open_project", IconName::Close)
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text("Remove Project from Window"))
                                .on_click({
                                    let project_group_key = project_group_key.clone();
                                    cx.listener(move |picker, _, window, cx| {
                                        cx.stop_propagation();
                                        window.prevent_default();
                                        picker.delegate.remove_project_group(
                                            project_group_key.clone(),
                                            window,
                                            cx,
                                        );
                                        let query = picker.query(cx);
                                        picker.update_matches(query, window, cx);
                                    })
                                }),
                        )
                    })
                    .into_any_element();

                Some(
                    ListItem::new(ix)
                        .inset(true)
                        .toggle_state(selected)
                        .spacing(ListItemSpacing::Sparse)
                        .child(
                            h_flex()
                                .id("open_project_info_container")
                                .w_full()
                                .min_w_0()
                                .gap_2p5()
                                .when(show_icon, |this| {
                                    this.child(Icon::new(icon).color(Color::Muted))
                                })
                                .child({
                                    let mut highlighted = highlighted_match;
                                    if !self.render_paths {
                                        highlighted.paths.clear();
                                    }
                                    highlighted.render(window, cx)
                                })
                                .tooltip(Tooltip::text(tooltip_path)),
                        )
                        .end_slot(secondary_actions)
                        .show_end_slot_on_hover()
                        .into_any_element(),
                )
            }
            ProjectPickerEntry::PinnedProject(hit) => {
                let pinned_project = self.pinned_projects.get(hit.candidate_id)?;
                let path = pinned_project.path.compact();
                let tooltip_path: SharedString = path.to_string_lossy().to_string().into();
                let (match_label, path_highlight) =
                    highlights_for_path(path.as_ref(), &hit.positions, 0);
                let highlighted_match = HighlightedMatchWithPaths {
                    prefix: None,
                    match_label: HighlightedMatch::join(match_label.into_iter(), ", "),
                    paths: vec![path_highlight],
                    active: false,
                };

                let focus_handle = self.focus_handle.clone();
                let candidate_id = hit.candidate_id;
                let secondary_actions = h_flex()
                    .gap_px()
                    .child(
                        IconButton::new(
                            ("move_pinned_project_up", candidate_id),
                            IconName::ArrowUp,
                        )
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text("Move Up"))
                        .on_click(cx.listener(
                            move |picker, _event, window, cx| {
                                cx.stop_propagation();
                                window.prevent_default();
                                picker.delegate.move_pinned_project(
                                    candidate_id,
                                    PinnedProjectMoveDirection::Up,
                                    window,
                                    cx,
                                );
                                let query = picker.query(cx);
                                picker.update_matches(query, window, cx);
                            },
                        )),
                    )
                    .child(
                        IconButton::new(
                            ("move_pinned_project_down", candidate_id),
                            IconName::ArrowDown,
                        )
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text("Move Down"))
                        .on_click(cx.listener(
                            move |picker, _event, window, cx| {
                                cx.stop_propagation();
                                window.prevent_default();
                                picker.delegate.move_pinned_project(
                                    candidate_id,
                                    PinnedProjectMoveDirection::Down,
                                    window,
                                    cx,
                                );
                                let query = picker.query(cx);
                                picker.update_matches(query, window, cx);
                            },
                        )),
                    )
                    .child(
                        IconButton::new(
                            ("open_pinned_new_window", candidate_id),
                            IconName::ArrowUpRight,
                        )
                        .icon_size(IconSize::Small)
                        .tooltip({
                            move |_, cx| {
                                Tooltip::for_action_in(
                                    "Open Project in New Window",
                                    &menu::SecondaryConfirm,
                                    &focus_handle,
                                    cx,
                                )
                            }
                        })
                        .on_click(cx.listener(
                            move |picker, _event, window, cx| {
                                cx.stop_propagation();
                                window.prevent_default();
                                picker.delegate.set_selected_index(ix, window, cx);
                                picker.delegate.confirm(true, window, cx);
                            },
                        )),
                    )
                    .child(
                        IconButton::new(("unpin_project", candidate_id), IconName::Close)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Unpin Project"))
                            .on_click(cx.listener(move |picker, _event, window, cx| {
                                cx.stop_propagation();
                                window.prevent_default();
                                picker
                                    .delegate
                                    .remove_pinned_project(candidate_id, window, cx);
                                let query = picker.query(cx);
                                picker.update_matches(query, window, cx);
                            })),
                    )
                    .into_any_element();

                Some(
                    ListItem::new(ix)
                        .toggle_state(selected)
                        .inset(true)
                        .spacing(ListItemSpacing::Sparse)
                        .child(
                            h_flex()
                                .id("pinned_project_info_container")
                                .w_full()
                                .min_w_0()
                                .gap_2p5()
                                .flex_grow_1()
                                .child(Icon::new(IconName::Pin).color(Color::Muted))
                                .child({
                                    let mut highlighted = highlighted_match;
                                    if !self.render_paths {
                                        highlighted.paths.clear();
                                    }
                                    highlighted.render(window, cx)
                                })
                                .tooltip(move |_, cx| {
                                    Tooltip::with_meta(
                                        "Open Pinned Project in This Window",
                                        None,
                                        tooltip_path.clone(),
                                        cx,
                                    )
                                }),
                        )
                        .end_slot(secondary_actions)
                        .show_end_slot_on_hover()
                        .into_any_element(),
                )
            }
            ProjectPickerEntry::RecentProject(hit) => {
                let workspace = self.workspaces.get(hit.candidate_id)?;
                let location = &workspace.location;
                let raw_paths = &workspace.paths;
                let identity_paths = &workspace.identity_paths;
                let is_local = matches!(location, SerializedWorkspaceLocation::Local);
                let can_pin_project = recent_workspace_pinnable_project_path(workspace).is_some();
                let paths_to_add = raw_paths.paths().to_vec();
                let ordered_paths: Vec<_> = identity_paths
                    .ordered_paths()
                    .map(|p| p.compact().to_string_lossy().to_string())
                    .collect();
                let tooltip_path: SharedString = match &location {
                    SerializedWorkspaceLocation::Remote(options) => {
                        let host = options.display_name();
                        if ordered_paths.len() == 1 {
                            format!("{} ({})", ordered_paths[0], host).into()
                        } else {
                            format!("{}\n({})", ordered_paths.join("\n"), host).into()
                        }
                    }
                    _ => ordered_paths.join("\n").into(),
                };

                let mut path_start_offset = 0;
                let (match_labels, paths): (Vec<_>, Vec<_>) = identity_paths
                    .ordered_paths()
                    .map(|p| p.compact())
                    .map(|path| {
                        let highlighted_text =
                            highlights_for_path(path.as_ref(), &hit.positions, path_start_offset);
                        path_start_offset += highlighted_text.1.text.len();
                        highlighted_text
                    })
                    .unzip();

                let tooltip_title = if paths.len() > 1 {
                    "Add Folders to this Project"
                } else {
                    "Add Folder to this Project"
                };

                let prefix = match &location {
                    SerializedWorkspaceLocation::Remote(options) => {
                        Some(SharedString::from(options.display_name()))
                    }
                    _ => None,
                };

                let highlighted_match = HighlightedMatchWithPaths {
                    prefix,
                    match_label: HighlightedMatch::join(match_labels.into_iter().flatten(), ", "),
                    paths,
                    active: false,
                };

                let focus_handle = self.focus_handle.clone();
                let candidate_id = hit.candidate_id;
                let secondary_confirm_tooltip = if self.create_new_window {
                    "Open Project in This Window"
                } else {
                    "Open Project in New Window"
                };
                let primary_confirm_tooltip = if self.create_new_window {
                    "Open Project in New Window"
                } else {
                    "Open Project in This Window"
                };
                let secondary_confirm_icon = if self.create_new_window {
                    IconName::ThisWindow
                } else {
                    IconName::ArrowUpRight
                };

                let secondary_actions = h_flex()
                    .gap_px()
                    .when(can_pin_project, |this| {
                        this.child(
                            IconButton::new(("pin_project", candidate_id), IconName::Pin)
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text("Pin Project"))
                                .on_click(cx.listener(move |picker, _event, window, cx| {
                                    cx.stop_propagation();
                                    window.prevent_default();
                                    picker.delegate.pin_recent_project(candidate_id, window, cx);
                                    let query = picker.query(cx);
                                    picker.update_matches(query, window, cx);
                                })),
                        )
                    })
                    .when(is_local, |this| {
                        this.child(
                            IconButton::new("add_to_workspace", IconName::FolderOpenAdd)
                                .icon_size(IconSize::Small)
                                .tooltip(move |_, cx| {
                                    Tooltip::with_meta(
                                        tooltip_title,
                                        None,
                                        "As a multi-root folder",
                                        cx,
                                    )
                                })
                                .on_click({
                                    let paths_to_add = paths_to_add.clone();
                                    cx.listener(move |picker, _event, window, cx| {
                                        cx.stop_propagation();
                                        window.prevent_default();
                                        picker.delegate.add_paths_to_project(
                                            paths_to_add.clone(),
                                            window,
                                            cx,
                                        );
                                    })
                                }),
                        )
                    })
                    .child(
                        IconButton::new("alternate_open", secondary_confirm_icon)
                            .icon_size(IconSize::Small)
                            .tooltip({
                                move |_, cx| {
                                    Tooltip::for_action_in(
                                        secondary_confirm_tooltip,
                                        &menu::SecondaryConfirm,
                                        &focus_handle,
                                        cx,
                                    )
                                }
                            })
                            .on_click(cx.listener(move |this, _event, window, cx| {
                                cx.stop_propagation();
                                window.prevent_default();
                                this.delegate.set_selected_index(ix, window, cx);
                                this.delegate.confirm(true, window, cx);
                            })),
                    )
                    .child(
                        IconButton::new("delete", IconName::Close)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Remove from Recent Projects"))
                            .on_click(cx.listener(move |this, _event, window, cx| {
                                cx.stop_propagation();
                                window.prevent_default();
                                this.delegate.delete_recent_project(ix, window, cx)
                            })),
                    )
                    .into_any_element();

                let icon = icon_for_remote_connection(match location {
                    SerializedWorkspaceLocation::Local => None,
                    SerializedWorkspaceLocation::Remote(options) => Some(options),
                });
                let show_icon = self.filtered_entries_include_remote_project();

                Some(
                    ListItem::new(ix)
                        .toggle_state(selected)
                        .inset(true)
                        .spacing(ListItemSpacing::Sparse)
                        .child(
                            h_flex()
                                .id("project_info_container")
                                .w_full()
                                .min_w_0()
                                .gap_2p5()
                                .flex_grow_1()
                                .when(show_icon, |this| {
                                    this.child(Icon::new(icon).color(Color::Muted))
                                })
                                .child({
                                    let mut highlighted = highlighted_match;
                                    if !self.render_paths {
                                        highlighted.paths.clear();
                                    }
                                    highlighted.render(window, cx)
                                })
                                .tooltip(move |_, cx| {
                                    Tooltip::with_meta(
                                        primary_confirm_tooltip,
                                        None,
                                        tooltip_path.clone(),
                                        cx,
                                    )
                                }),
                        )
                        .end_slot(secondary_actions)
                        .show_end_slot_on_hover()
                        .into_any_element(),
                )
            }
        }
    }

    fn render_footer(&self, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Option<AnyElement> {
        let focus_handle = self.focus_handle.clone();
        let popover_style = matches!(self.style, ProjectPickerStyle::Popover);

        let is_already_open_entry = matches!(
            self.filtered_entries.get(self.selected_index),
            Some(ProjectPickerEntry::OpenFolder { .. } | ProjectPickerEntry::ProjectGroup(_))
        );

        let show_move_to_new_window = match self.filtered_entries.get(self.selected_index) {
            Some(ProjectPickerEntry::ProjectGroup(hit)) => {
                self.window_project_groups.len() >= 2
                    && self
                        .window_project_groups
                        .get(hit.candidate_id)
                        .is_some_and(|key| key.host().is_none())
            }
            _ => false,
        };

        if popover_style {
            return Some(
                v_flex()
                    .flex_1()
                    .p_1p5()
                    .gap_1()
                    .border_t_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child({
                        ButtonLike::new("open_local_folder")
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_1()
                                    .justify_between()
                                    .child(Label::new("Open Local Folders"))
                                    .child(KeyBinding::for_action_in(
                                        &workspace::Open {
                                            create_new_window: Some(self.create_new_window),
                                        },
                                        &focus_handle,
                                        cx,
                                    )),
                            )
                            .on_click({
                                let workspace = self.workspace.clone();
                                let create_new_window = self.create_new_window;
                                move |_, window, cx| {
                                    open_local_project(
                                        workspace.clone(),
                                        create_new_window,
                                        window,
                                        cx,
                                    );
                                }
                            })
                    })
                    .child(
                        ButtonLike::new("open_remote_folder")
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_1()
                                    .justify_between()
                                    .child(Label::new("Open Remote Folder"))
                                    .child(KeyBinding::for_action(
                                        &OpenRemote {
                                            from_existing_connection: false,
                                            create_new_window: Some(self.create_new_window),
                                        },
                                        cx,
                                    )),
                            )
                            .on_click({
                                let create_new_window = self.create_new_window;
                                move |_, window, cx| {
                                    window.dispatch_action(
                                        OpenRemote {
                                            from_existing_connection: false,
                                            create_new_window: Some(create_new_window),
                                        }
                                        .boxed_clone(),
                                        cx,
                                    )
                                }
                            }),
                    )
                    .into_any(),
            );
        }

        let selected_entry = self.filtered_entries.get(self.selected_index);

        let is_current_workspace_entry =
            if let Some(ProjectPickerEntry::ProjectGroup(hit)) = selected_entry {
                self.window_project_groups
                    .get(hit.candidate_id)
                    .is_some_and(|key| self.is_active_project_group(key, cx))
            } else {
                false
            };

        let secondary_footer_actions: Option<AnyElement> = match selected_entry {
            Some(ProjectPickerEntry::OpenFolder { .. }) => Some(
                Button::new("remove_selected", "Remove Folder")
                    .key_binding(KeyBinding::for_action_in(
                        &RemoveSelected,
                        &focus_handle,
                        cx,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(RemoveSelected.boxed_clone(), cx)
                    })
                    .into_any_element(),
            ),
            Some(ProjectPickerEntry::ProjectGroup(_)) if !is_current_workspace_entry => Some(
                Button::new("remove_selected", "Remove from Window")
                    .key_binding(KeyBinding::for_action_in(
                        &RemoveSelected,
                        &focus_handle,
                        cx,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(RemoveSelected.boxed_clone(), cx)
                    })
                    .into_any_element(),
            ),
            Some(ProjectPickerEntry::RecentProject(_)) => Some(
                Button::new("delete_recent", "Remove")
                    .key_binding(KeyBinding::for_action_in(
                        &RemoveSelected,
                        &focus_handle,
                        cx,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(RemoveSelected.boxed_clone(), cx)
                    })
                    .into_any_element(),
            ),
            Some(ProjectPickerEntry::PinnedProject(_)) => Some(
                Button::new("unpin_project", "Unpin")
                    .key_binding(KeyBinding::for_action_in(
                        &RemoveSelected,
                        &focus_handle,
                        cx,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(RemoveSelected.boxed_clone(), cx)
                    })
                    .into_any_element(),
            ),
            _ => None,
        };

        Some(
            h_flex()
                .flex_1()
                .p_1p5()
                .gap_1()
                .justify_end()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .when_some(secondary_footer_actions, |this, actions| {
                    this.child(actions)
                })
                .map(|this| {
                    if is_already_open_entry {
                        this.when(show_move_to_new_window, |this| {
                            this.child({
                                let window_project_groups = self.window_project_groups.clone();
                                let selected_index = self.selected_index;
                                let filtered_entries = self.filtered_entries.clone();
                                Button::new("move_to_new_window", "New Window")
                                    .key_binding(KeyBinding::for_action_in(
                                        &menu::SecondaryConfirm,
                                        &focus_handle,
                                        cx,
                                    ))
                                    .on_click(move |_, window, cx| {
                                        let key = match filtered_entries.get(selected_index) {
                                            Some(ProjectPickerEntry::ProjectGroup(hit)) => {
                                                window_project_groups.get(hit.candidate_id).cloned()
                                            }
                                            _ => None,
                                        };
                                        if let Some(key) = key {
                                            move_project_group_to_new_window(&key, window, cx);
                                        }
                                    })
                            })
                        })
                        .child(
                            Button::new("activate", "Activate")
                                .key_binding(KeyBinding::for_action_in(
                                    &menu::Confirm,
                                    &focus_handle,
                                    cx,
                                ))
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(menu::Confirm.boxed_clone(), cx)
                                }),
                        )
                    } else if self.create_new_window {
                        this.child(
                            Button::new("open_here", "This Window")
                                .key_binding(KeyBinding::for_action_in(
                                    &menu::SecondaryConfirm,
                                    &focus_handle,
                                    cx,
                                ))
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(menu::SecondaryConfirm.boxed_clone(), cx)
                                }),
                        )
                        .child(
                            Button::new("open_new_window", "Open")
                                .key_binding(KeyBinding::for_action_in(
                                    &menu::Confirm,
                                    &focus_handle,
                                    cx,
                                ))
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(menu::Confirm.boxed_clone(), cx)
                                }),
                        )
                    } else {
                        this.child(
                            Button::new("open_new_window", "New Window")
                                .key_binding(KeyBinding::for_action_in(
                                    &menu::SecondaryConfirm,
                                    &focus_handle,
                                    cx,
                                ))
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(menu::SecondaryConfirm.boxed_clone(), cx)
                                }),
                        )
                        .child(
                            Button::new("open_here", "Open")
                                .key_binding(KeyBinding::for_action_in(
                                    &menu::Confirm,
                                    &focus_handle,
                                    cx,
                                ))
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(menu::Confirm.boxed_clone(), cx)
                                }),
                        )
                    }
                })
                .child(Divider::vertical())
                .child(
                    PopoverMenu::new("actions-menu-popover")
                        .with_handle(self.actions_menu_handle.clone())
                        .anchor(gpui::Anchor::BottomRight)
                        .offset(gpui::Point {
                            x: px(0.0),
                            y: px(-2.0),
                        })
                        .trigger(
                            Button::new("actions-trigger", "Actions")
                                .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                                .key_binding(KeyBinding::for_action_in(
                                    &ToggleActionsMenu,
                                    &focus_handle,
                                    cx,
                                )),
                        )
                        .menu({
                            let focus_handle = focus_handle.clone();
                            let workspace_handle = self.workspace.clone();
                            let create_new_window = self.create_new_window;
                            let open_action = workspace::Open {
                                create_new_window: Some(create_new_window),
                            };
                            let show_pinned_actions = matches!(
                                selected_entry,
                                Some(ProjectPickerEntry::PinnedProject(_))
                            );
                            let show_pin_recent_project = match selected_entry {
                                Some(ProjectPickerEntry::RecentProject(hit)) => self
                                    .workspaces
                                    .get(hit.candidate_id)
                                    .is_some_and(|workspace| {
                                        recent_workspace_pinnable_project_path(workspace).is_some()
                                    }),
                                _ => false,
                            };
                            let show_add_to_workspace = match selected_entry {
                                Some(ProjectPickerEntry::RecentProject(hit)) => self
                                    .workspaces
                                    .get(hit.candidate_id)
                                    .map(|workspace| {
                                        matches!(
                                            workspace.location,
                                            SerializedWorkspaceLocation::Local
                                        )
                                    })
                                    .unwrap_or(false),
                                _ => false,
                            };

                            move |window, cx| {
                                Some(ContextMenu::build(window, cx, {
                                    let focus_handle = focus_handle.clone();
                                    let workspace_handle = workspace_handle.clone();
                                    let open_action = open_action.clone();
                                    move |menu, _, _| {
                                        menu.context(focus_handle)
                                            .when(show_pinned_actions, |menu| {
                                                menu.action(
                                                    "Move Pinned Project Up",
                                                    MovePinnedProjectUp.boxed_clone(),
                                                )
                                                .action(
                                                    "Move Pinned Project Down",
                                                    MovePinnedProjectDown.boxed_clone(),
                                                )
                                                .action(
                                                    "Unpin Project",
                                                    RemoveSelected.boxed_clone(),
                                                )
                                                .separator()
                                            })
                                            .when(show_pin_recent_project, |menu| {
                                                menu.action(
                                                    "Pin Project",
                                                    PinSelectedRecentProject.boxed_clone(),
                                                )
                                                .separator()
                                            })
                                            .when(show_add_to_workspace, |menu| {
                                                menu.action(
                                                    "Add Folder to this Project",
                                                    AddToWorkspace.boxed_clone(),
                                                )
                                                .separator()
                                            })
                                            .entry(
                                                "Open Local Folders",
                                                Some(open_action.boxed_clone()),
                                                {
                                                    let workspace_handle = workspace_handle.clone();
                                                    move |window, cx| {
                                                        open_local_project(
                                                            workspace_handle.clone(),
                                                            create_new_window,
                                                            window,
                                                            cx,
                                                        );
                                                    }
                                                },
                                            )
                                            .action(
                                                "Open Remote Folder",
                                                OpenRemote {
                                                    from_existing_connection: false,
                                                    create_new_window: Some(create_new_window),
                                                }
                                                .boxed_clone(),
                                            )
                                    }
                                }))
                            }
                        }),
                )
                .into_any(),
        )
    }
}

fn icon_for_project_group(key: &ProjectGroupKey) -> IconName {
    let host = key.host();
    icon_for_remote_connection(host.as_ref())
}

pub(crate) fn icon_for_remote_connection(options: Option<&RemoteConnectionOptions>) -> IconName {
    match options {
        None => IconName::Screen,
        Some(options) => match options {
            RemoteConnectionOptions::Ssh(_) => IconName::Server,
            RemoteConnectionOptions::Wsl(_) => IconName::Linux,
            RemoteConnectionOptions::Docker(_) => IconName::Box,
            #[cfg(any(test, feature = "test-support"))]
            RemoteConnectionOptions::Mock(_) => IconName::Server,
        },
    }
}

// Compute the highlighted text for the name and path
pub(crate) fn highlights_for_path(
    path: &Path,
    match_positions: &Vec<usize>,
    path_start_offset: usize,
) -> (Option<HighlightedMatch>, HighlightedMatch) {
    let path_string = path.to_string_lossy();
    let path_text = path_string.to_string();
    let path_byte_len = path_text.len();
    // Get the subset of match highlight positions that line up with the given path.
    // Also adjusts them to start at the path start
    let path_positions = match_positions
        .iter()
        .copied()
        .skip_while(|position| *position < path_start_offset)
        .take_while(|position| *position < path_start_offset + path_byte_len)
        .map(|position| position - path_start_offset)
        .collect::<Vec<_>>();

    // Again subset the highlight positions to just those that line up with the file_name
    // again adjusted to the start of the file_name
    let file_name_text_and_positions = path.file_name().map(|file_name| {
        let file_name_text = file_name.to_string_lossy().into_owned();
        let file_name_start_byte = path_byte_len - file_name_text.len();
        let highlight_positions = path_positions
            .iter()
            .copied()
            .skip_while(|position| *position < file_name_start_byte)
            .take_while(|position| *position < file_name_start_byte + file_name_text.len())
            .map(|position| position - file_name_start_byte)
            .collect::<Vec<_>>();
        HighlightedMatch {
            text: file_name_text,
            highlight_positions,
            color: Color::Default,
        }
    });

    (
        file_name_text_and_positions,
        HighlightedMatch {
            text: path_text,
            highlight_positions: path_positions,
            color: Color::Default,
        },
    )
}

fn move_project_group_to_new_window(key: &ProjectGroupKey, window: &mut Window, cx: &mut App) {
    if let Some(handle) = window.window_handle().downcast::<MultiWorkspace>() {
        let key = key.clone();
        cx.defer(move |cx| {
            handle
                .update(cx, |multi_workspace, window, cx| {
                    multi_workspace
                        .open_project_group_in_new_window(&key, window, cx)
                        .detach_and_log_err(cx);
                })
                .log_err();
        });
    }
}

fn open_local_project(
    workspace: WeakEntity<Workspace>,
    create_new_window: bool,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui::PathPromptOptions;
    use project::DirectoryLister;

    let Some(workspace) = workspace.upgrade() else {
        return;
    };

    let paths = workspace.update(cx, |workspace, cx| {
        workspace.prompt_for_open_path(
            PathPromptOptions {
                files: true,
                directories: true,
                multiple: true,
                prompt: None,
            },
            DirectoryLister::Local(
                workspace.project().clone(),
                workspace.app_state().fs.clone(),
            ),
            window,
            cx,
        )
    });

    let multi_workspace_handle = window.window_handle().downcast::<MultiWorkspace>();
    window
        .spawn(cx, async move |cx| {
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
            if let Some(task) = workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.open_workspace_for_paths(OpenMode::NewWindow, paths, window, cx)
                })
                .log_err()
            {
                task.await.log_err();
            }
        })
        .detach();
}

impl RecentProjectsDelegate {
    fn first_selectable_entry_index(&self) -> Option<usize> {
        self.filtered_entries.iter().position(is_selectable_entry)
    }

    fn replace_pinned_projects(
        &mut self,
        pinned_projects: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        self.pinned_projects = pinned_projects
            .iter()
            .cloned()
            .map(pinned_project_entry)
            .collect();

        if let Some(workspace) = self.workspace.upgrade() {
            let fs = workspace.read(cx).app_state().fs.clone();
            update_pinned_project_settings(fs, window, cx, move |settings, _| {
                set_pinned_project_settings(settings, pinned_projects);
            });
        }
    }

    fn remove_pinned_project(
        &mut self,
        candidate_id: usize,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        if candidate_id >= self.pinned_projects.len() {
            return;
        }

        let mut pinned_projects = self
            .pinned_projects
            .iter()
            .map(|pinned_project| pinned_project.setting_path.clone())
            .collect::<Vec<_>>();
        pinned_projects.remove(candidate_id);
        self.pending_pinned_project_selection = if pinned_projects.is_empty() {
            Some(PendingPinnedProjectSelection::FirstSelectable)
        } else {
            Some(PendingPinnedProjectSelection::PinnedProject(
                candidate_id.min(pinned_projects.len() - 1),
            ))
        };
        self.replace_pinned_projects(pinned_projects, window, cx);
        self.snap_selection_to_first_non_header_match = false;
    }

    fn pin_recent_project(
        &mut self,
        candidate_id: usize,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let Some(project_path) = self
            .workspaces
            .get(candidate_id)
            .and_then(recent_workspace_pinnable_project_path)
        else {
            return;
        };

        let mut pinned_projects = self
            .pinned_projects
            .iter()
            .map(|pinned_project| pinned_project.setting_path.clone())
            .collect::<Vec<_>>();
        if !pin_project_path(&mut pinned_projects, &project_path) {
            return;
        }

        self.pending_pinned_project_selection = Some(PendingPinnedProjectSelection::PinnedProject(
            pinned_projects.len() - 1,
        ));
        self.replace_pinned_projects(pinned_projects, window, cx);
        self.snap_selection_to_first_non_header_match = false;
    }

    fn move_pinned_project(
        &mut self,
        candidate_id: usize,
        direction: PinnedProjectMoveDirection,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let mut pinned_projects = self
            .pinned_projects
            .iter()
            .map(|pinned_project| pinned_project.setting_path.clone())
            .collect::<Vec<_>>();
        let target_candidate_id = match direction {
            PinnedProjectMoveDirection::Up => candidate_id.checked_sub(1),
            PinnedProjectMoveDirection::Down => {
                let next_candidate_id = candidate_id.saturating_add(1);
                (next_candidate_id < pinned_projects.len()).then_some(next_candidate_id)
            }
        };
        if !move_pinned_project(&mut pinned_projects, candidate_id, direction) {
            return;
        }

        self.pending_pinned_project_selection =
            target_candidate_id.map(PendingPinnedProjectSelection::PinnedProject);
        self.replace_pinned_projects(pinned_projects, window, cx);
        self.snap_selection_to_first_non_header_match = false;
    }

    fn open_pinned_project(
        &mut self,
        candidate_id: usize,
        secondary: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(candidate_project) = self.pinned_projects.get(candidate_id) else {
            return;
        };

        let replace_current_window = self.create_new_window == secondary;
        let paths = vec![candidate_project.path.clone()];

        workspace.update(cx, |workspace, cx| {
            if replace_current_window {
                if let Some(handle) = window.window_handle().downcast::<MultiWorkspace>() {
                    cx.defer(move |cx| {
                        handle
                            .update(cx, |multi_workspace, window, cx| {
                                multi_workspace
                                    .open_project(paths, OpenMode::Activate, window, cx)
                                    .detach_and_prompt_err(
                                        "Failed to open project",
                                        window,
                                        cx,
                                        |_, _, _| None,
                                    );
                            })
                            .log_err();
                    });
                }
            } else {
                workspace
                    .open_workspace_for_paths(OpenMode::NewWindow, paths, window, cx)
                    .detach_and_prompt_err("Failed to open project", window, cx, |_, _, _| None);
            }
        });
        cx.emit(DismissEvent);
    }

    fn open_recent_projects(
        &mut self,
        candidate_id: usize,
        secondary: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(candidate_workspace) = self.workspaces.get(candidate_id) else {
            return;
        };

        let replace_current_window = self.create_new_window == secondary;
        let candidate_workspace_id = candidate_workspace.workspace_id;
        let candidate_workspace_location = candidate_workspace.location.clone();
        let candidate_workspace_paths = candidate_workspace.paths.clone();

        workspace.update(cx, |workspace, cx| {
            if workspace.database_id() == Some(candidate_workspace_id) {
                return;
            }
            match candidate_workspace_location {
                SerializedWorkspaceLocation::Local => {
                    let paths = candidate_workspace_paths.paths().to_vec();
                    if replace_current_window {
                        if let Some(handle) = window.window_handle().downcast::<MultiWorkspace>() {
                            cx.defer(move |cx| {
                                if let Some(task) = handle
                                    .update(cx, |multi_workspace, window, cx| {
                                        multi_workspace.open_project(
                                            paths,
                                            OpenMode::Activate,
                                            window,
                                            cx,
                                        )
                                    })
                                    .log_err()
                                {
                                    task.detach_and_log_err(cx);
                                }
                            });
                        }
                        return;
                    } else {
                        workspace
                            .open_workspace_for_paths(OpenMode::NewWindow, paths, window, cx)
                            .detach_and_prompt_err(
                                "Failed to open project",
                                window,
                                cx,
                                |_, _, _| None,
                            );
                    }
                }
                SerializedWorkspaceLocation::Remote(mut connection) => {
                    let app_state = workspace.app_state().clone();
                    let replace_window = if replace_current_window {
                        window.window_handle().downcast::<MultiWorkspace>()
                    } else {
                        None
                    };
                    let open_options = OpenOptions {
                        requesting_window: replace_window,
                        ..Default::default()
                    };
                    if let RemoteConnectionOptions::Ssh(connection) = &mut connection {
                        RemoteSettings::get_global(cx)
                            .fill_connection_options_from_settings(connection);
                    };
                    let paths = candidate_workspace_paths.paths().to_vec();
                    cx.spawn_in(window, async move |_, cx| {
                        open_remote_project(connection.clone(), paths, app_state, open_options, cx)
                            .await
                    })
                    .detach_and_prompt_err(
                        "Failed to open project",
                        window,
                        cx,
                        |_, _, _| None,
                    );
                }
            }
        });
        cx.emit(DismissEvent);
    }

    fn add_paths_to_project(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let open_paths_task = workspace.update(cx, |workspace, cx| {
            workspace.open_paths(
                paths,
                OpenOptions {
                    visible: Some(OpenVisible::All),
                    ..Default::default()
                },
                None,
                window,
                cx,
            )
        });
        cx.spawn_in(window, async move |picker, cx| {
            let _result = open_paths_task.await;
            picker
                .update_in(cx, |picker, window, cx| {
                    let Some(workspace) = picker.delegate.workspace.upgrade() else {
                        return;
                    };
                    picker.delegate.open_folders = get_open_folders(workspace.read(cx), cx);
                    let query = picker.query(cx);
                    picker.update_matches(query, window, cx);
                })
                .ok();
        })
        .detach();
    }

    /// Returns the new selection index after the entry at `deleted_index`
    /// is removed.
    ///
    /// - Prefers the nearest entry matching `prefer_section` so the user
    ///   stays in the same section they were navigating.
    /// - Falls back to any other selectable entry so the picker doesn't
    ///   land on a header.
    fn replacement_index_after_deletion(
        &self,
        deleted_index: usize,
        prefer_previous: bool,
        prefer_section: fn(&ProjectPickerEntry) -> bool,
    ) -> Option<usize> {
        let replacement_index = |matches_entry: fn(&ProjectPickerEntry) -> bool| {
            let next_index = self
                .filtered_entries
                .iter()
                .enumerate()
                .skip(deleted_index)
                .find_map(|(index, entry)| matches_entry(entry).then_some(index));
            let previous_index = self
                .filtered_entries
                .iter()
                .enumerate()
                .take(deleted_index.min(self.filtered_entries.len()))
                .rev()
                .find_map(|(index, entry)| matches_entry(entry).then_some(index));

            if prefer_previous {
                previous_index.or(next_index)
            } else {
                next_index.or(previous_index)
            }
        };

        replacement_index(prefer_section).or_else(|| replacement_index(is_selectable_entry))
    }

    fn update_picker_after_recent_project_deletion(
        picker: &mut Picker<Self>,
        deleted_index: usize,
        workspaces: Vec<RecentWorkspace>,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let prefer_previous = picker.is_scrolled_to_end() == Some(true);
        picker.delegate.set_workspaces(workspaces);
        picker.delegate.snap_selection_to_first_non_header_match = false;
        picker.update_matches_with_options(
            picker.query(cx),
            ScrollBehavior::PreserveOffset,
            window,
            cx,
        );
        if let Some(replacement_index) = picker.delegate.replacement_index_after_deletion(
            deleted_index,
            prefer_previous,
            |entry| matches!(entry, ProjectPickerEntry::RecentProject(_)),
        ) {
            picker.set_selected_index(replacement_index, None, false, window, cx);
        }
    }

    fn delete_recent_project(
        &self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        if let Some(ProjectPickerEntry::RecentProject(selected_match)) =
            self.filtered_entries.get(ix)
        {
            let Some(recent_workspace) = self.workspaces.get(selected_match.candidate_id).cloned()
            else {
                return;
            };
            let fs = self
                .workspace
                .upgrade()
                .map(|ws| ws.read(cx).app_state().fs.clone());
            let db = WorkspaceDb::global(cx);
            cx.spawn_in(window, async move |this, cx| {
                let Some(fs) = fs else { return };
                let deleted_workspace_ids = db
                    .delete_recent_workspace_group(&recent_workspace)
                    .await
                    .log_err()
                    .unwrap_or_default();
                let workspaces = db
                    .recent_project_workspaces(fs.as_ref())
                    .await
                    .unwrap_or_default();
                this.update_in(cx, move |picker, window, cx| {
                    Self::update_picker_after_recent_project_deletion(
                        picker, ix, workspaces, window, cx,
                    );
                    // After deleting a project, we want to update the history manager to reflect the change.
                    // But we do not emit a update event when user opens a project, because it's handled in `workspace::load_workspace`.
                    if let Some(history_manager) = HistoryManager::global(cx) {
                        history_manager.update(cx, |this, cx| {
                            for workspace_id in &deleted_workspace_ids {
                                this.delete_history(*workspace_id, cx);
                            }
                        });
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    fn remove_project_group(
        &mut self,
        key: ProjectGroupKey,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        if let Some(handle) = window.window_handle().downcast::<MultiWorkspace>() {
            let key_for_remove = key.clone();
            cx.defer(move |cx| {
                handle
                    .update(cx, |multi_workspace, window, cx| {
                        multi_workspace
                            .remove_project_group(&key_for_remove, window, cx)
                            .detach_and_log_err(cx);
                    })
                    .log_err();
            });
        }

        self.window_project_groups.retain(|k| k != &key);
    }

    fn is_current_workspace(
        &self,
        workspace_id: WorkspaceId,
        cx: &mut Context<Picker<Self>>,
    ) -> bool {
        if let Some(workspace) = self.workspace.upgrade() {
            let workspace = workspace.read(cx);
            if Some(workspace_id) == workspace.database_id() {
                return true;
            }
        }

        false
    }

    fn is_active_project_group(&self, key: &ProjectGroupKey, cx: &App) -> bool {
        if let Some(workspace) = self.workspace.upgrade() {
            return workspace.read(cx).project_group_key(cx) == *key;
        }
        false
    }

    fn is_in_current_window_groups(&self, workspace: &RecentWorkspace) -> bool {
        self.window_project_groups
            .iter()
            .any(|key| key.matches(&workspace.project_group_key()))
    }

    fn is_open_folder(&self, paths: &PathList) -> bool {
        if self.open_folders.is_empty() {
            return false;
        }

        for workspace_path in paths.paths() {
            for open_folder in &self.open_folders {
                if workspace_path == &open_folder.path {
                    return true;
                }
            }
        }

        false
    }

    fn is_pinned_project(&self, paths: &PathList) -> bool {
        self.pinned_projects
            .iter()
            .any(|pinned_project| path_list_matches_pinned_project(paths, &pinned_project.path))
    }

    fn is_valid_recent_candidate(
        &self,
        workspace: &RecentWorkspace,
        cx: &mut Context<Picker<Self>>,
    ) -> bool {
        !self.is_current_workspace(workspace.workspace_id, cx)
            && !self.is_in_current_window_groups(workspace)
            && !self.is_open_folder(&workspace.paths)
            && !self.is_pinned_project(&workspace.identity_paths)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, UpdateGlobal, VisualTestContext};

    use project::DisableAiSettings;
    use serde_json::json;
    use settings::SettingsStore;
    use util::path;
    use workspace::{AppState, open_paths};

    use super::*;

    // Test picker for the empty query:
    //
    //   [0] Header("Current Folders")
    //   [1] OpenFolder(0)
    //   [2] OpenFolder(1)
    //   [3] Header("This Window")
    //   [4] ProjectGroup(0)
    //   [5] ProjectGroup(1)
    //   [6] Header("Recent Projects")
    //   [7..=26] RecentProject(0..=19)
    //
    const RECENT_PROJECT_COUNT: usize = 20;
    const FIRST_RECENT_PROJECT: usize = 7;
    const LAST_RECENT_PROJECT: usize = FIRST_RECENT_PROJECT + RECENT_PROJECT_COUNT - 1;

    fn open_folder(index: usize) -> OpenFolderEntry {
        OpenFolderEntry {
            worktree_id: WorktreeId::from_usize(index),
            name: format!("project-folder-{index}").into(),
            path: PathBuf::from(format!("/current/project-folder-{index}")),
            branch: None,
            is_active: false,
            connection_options: None,
        }
    }

    fn project_group(index: usize) -> ProjectGroupKey {
        ProjectGroupKey::new(
            None,
            PathList::new(&[PathBuf::from(format!("/this-window/project-{index}"))]),
        )
    }

    fn remote_project_group(index: usize) -> ProjectGroupKey {
        ProjectGroupKey::new(
            Some(RemoteConnectionOptions::Mock(
                remote::MockConnectionOptions { id: index as u64 },
            )),
            PathList::new(&[PathBuf::from(format!(
                "/this-window/remote-project-{index}"
            ))]),
        )
    }

    fn recent_workspace(index: usize) -> RecentWorkspace {
        let paths = PathList::new(&[PathBuf::from(format!("/recent/project-{index:02}"))]);
        RecentWorkspace {
            workspace_id: WorkspaceId::from_i64(index as i64),
            location: SerializedWorkspaceLocation::Local,
            paths: paths.clone(),
            identity_paths: paths,
            timestamp: Utc::now(),
        }
    }

    fn recent_workspaces() -> Vec<RecentWorkspace> {
        (0..RECENT_PROJECT_COUNT).map(recent_workspace).collect()
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear());
    }

    fn build_picker(
        cx: &mut TestAppContext,
    ) -> (
        Entity<Picker<RecentProjectsDelegate>>,
        &mut VisualTestContext,
    ) {
        init_test(cx);
        let (picker, cx) = cx.add_window_view(|window, cx| {
            let mut delegate = RecentProjectsDelegate::new(
                WeakEntity::new_invalid(),
                false,
                cx.focus_handle(),
                vec![open_folder(0), open_folder(1)],
                vec![project_group(0), project_group(1)],
                ProjectPickerStyle::Modal,
                cx,
            );
            delegate.set_workspaces(recent_workspaces());
            Picker::list(delegate, window, cx)
                .list_measure_all()
                .show_scrollbar(true)
                .max_height(Some(px(240.).into()))
        });
        draw(cx);
        (picker, cx)
    }

    fn scroll_to_and_select(
        picker: &Entity<Picker<RecentProjectsDelegate>>,
        cx: &mut VisualTestContext,
        index: usize,
    ) -> usize {
        picker.update_in(cx, |picker, window, cx| {
            picker.set_selected_index(index, None, true, window, cx);
        });
        draw(cx);
        picker.update(cx, |picker, _| picker.logical_scroll_top_index())
    }

    fn delete_recent_project_in_picker(
        picker: &Entity<Picker<RecentProjectsDelegate>>,
        cx: &mut VisualTestContext,
        index: usize,
    ) {
        picker.update_in(cx, |picker, window, cx| {
            let Some(ProjectPickerEntry::RecentProject(hit)) =
                picker.delegate.filtered_entries.get(index)
            else {
                panic!("expected entry at {index} to be a recent project");
            };
            let mut workspaces = picker.delegate.workspaces.clone();
            workspaces.remove(hit.candidate_id);
            RecentProjectsDelegate::update_picker_after_recent_project_deletion(
                picker, index, workspaces, window, cx,
            );
        });
    }

    #[track_caller]
    fn assert_scroll_top_is(
        picker: &Entity<Picker<RecentProjectsDelegate>>,
        cx: &mut VisualTestContext,
        expected: usize,
        phase: &str,
    ) {
        picker.update(cx, |picker, _| {
            assert_eq!(
                picker.logical_scroll_top_index(),
                expected,
                "scroll top should remain at {expected} ({phase})"
            );
            assert_selected_entry_is_recent_project(picker);
        });
    }

    #[track_caller]
    fn assert_pinned_to_bottom(
        picker: &Entity<Picker<RecentProjectsDelegate>>,
        cx: &mut VisualTestContext,
        phase: &str,
    ) {
        picker.update(cx, |picker, _| {
            assert_eq!(
                picker.is_scrolled_to_end(),
                Some(true),
                "picker should remain pinned to the bottom ({phase})"
            );
            assert!(
                picker.logical_scroll_top_index() > 0,
                "picker should not jump to the top while pinned to the bottom ({phase})"
            );
            assert_selected_entry_is_recent_project(picker);
        });
    }

    #[track_caller]
    fn assert_selected_entry_is_recent_project(picker: &Picker<RecentProjectsDelegate>) {
        assert!(matches!(
            picker
                .delegate
                .filtered_entries
                .get(picker.delegate.selected_index),
            Some(ProjectPickerEntry::RecentProject(_))
        ));
    }

    #[test]
    fn pinned_project_setting_mutations_are_idempotent_and_bounded() {
        let project_a = Path::new("/project/a");
        let project_b = Path::new("/project/b");
        let mut pinned_projects = vec![project_b.to_string_lossy().to_string()];

        assert!(pin_project_path(&mut pinned_projects, project_a));
        assert!(!pin_project_path(&mut pinned_projects, project_a));
        assert_eq!(
            pinned_projects,
            vec![
                project_b.to_string_lossy().to_string(),
                project_a.to_string_lossy().to_string()
            ]
        );

        assert!(move_pinned_project(
            &mut pinned_projects,
            1,
            PinnedProjectMoveDirection::Up
        ));
        assert_eq!(
            pinned_projects,
            vec![
                project_a.to_string_lossy().to_string(),
                project_b.to_string_lossy().to_string()
            ]
        );

        assert!(!move_pinned_project(
            &mut pinned_projects,
            0,
            PinnedProjectMoveDirection::Up
        ));
        assert!(!move_pinned_project(
            &mut pinned_projects,
            1,
            PinnedProjectMoveDirection::Down
        ));
        assert!(!move_pinned_project(
            &mut pinned_projects,
            99,
            PinnedProjectMoveDirection::Up
        ));
        assert!(!move_pinned_project(
            &mut pinned_projects,
            99,
            PinnedProjectMoveDirection::Down
        ));

        assert!(unpin_project_path(&mut pinned_projects, project_a));
        assert!(!unpin_project_path(&mut pinned_projects, project_a));
        assert_eq!(
            pinned_projects,
            vec![project_b.to_string_lossy().to_string()]
        );
    }

    #[test]
    fn pin_recent_project_eligibility_requires_local_single_folder() {
        let local_single_folder = recent_workspace(1);
        assert_eq!(
            recent_workspace_pinnable_project_path(&local_single_folder),
            Some(PathBuf::from("/recent/project-01"))
        );

        let remote_paths = PathList::new(&[PathBuf::from("/recent/remote-project")]);
        let remote_workspace = RecentWorkspace {
            workspace_id: WorkspaceId::from_i64(100),
            location: SerializedWorkspaceLocation::Remote(RemoteConnectionOptions::Mock(
                remote::MockConnectionOptions { id: 100 },
            )),
            paths: remote_paths.clone(),
            identity_paths: remote_paths,
            timestamp: Utc::now(),
        };
        assert!(recent_workspace_pinnable_project_path(&remote_workspace).is_none());

        let multi_root_paths = PathList::new(&[
            PathBuf::from("/recent/multi-root-a"),
            PathBuf::from("/recent/multi-root-b"),
        ]);
        let multi_root_workspace = RecentWorkspace {
            workspace_id: WorkspaceId::from_i64(101),
            location: SerializedWorkspaceLocation::Local,
            paths: multi_root_paths.clone(),
            identity_paths: multi_root_paths,
            timestamp: Utc::now(),
        };
        assert!(recent_workspace_pinnable_project_path(&multi_root_workspace).is_none());
    }

    #[gpui::test]
    fn pin_recent_project_row_moves_it_to_pinned_projects(cx: &mut TestAppContext) {
        init_test(cx);

        let (picker, cx) = cx.add_window_view(|window, cx| {
            let mut delegate = RecentProjectsDelegate::new(
                WeakEntity::new_invalid(),
                false,
                cx.focus_handle(),
                Vec::new(),
                Vec::new(),
                ProjectPickerStyle::Modal,
                cx,
            );
            delegate.set_workspaces(recent_workspaces());
            Picker::list(delegate, window, cx)
                .list_measure_all()
                .show_scrollbar(true)
        });

        const RECENT_CANDIDATE_ID: usize = 2;
        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches(String::new(), window, cx);
            picker
                .delegate
                .pin_recent_project(RECENT_CANDIDATE_ID, window, cx);
            picker.update_matches(String::new(), window, cx);
        });

        picker.update(cx, |picker, _| {
            let entries = &picker.delegate.filtered_entries;
            assert!(matches!(
                entries.first(),
                Some(ProjectPickerEntry::Header(title)) if title.as_ref() == "Pinned Projects"
            ));

            let Some(ProjectPickerEntry::PinnedProject(hit)) = entries.get(1) else {
                panic!("expected newly pinned recent project to render first");
            };
            assert_eq!(
                picker.delegate.pinned_projects[hit.candidate_id].setting_path,
                "/recent/project-02"
            );
            assert_eq!(
                picker.delegate.selected_index, 1,
                "newly pinned project should remain selected after matches refresh"
            );
            assert!(
                !entries.iter().any(|entry| matches!(
                    entry,
                    ProjectPickerEntry::RecentProject(hit)
                        if hit.candidate_id == RECENT_CANDIDATE_ID
                )),
                "newly pinned project should not also render as a recent project"
            );
        });
    }

    #[gpui::test]
    fn pinned_projects_render_before_recents_and_dedup(cx: &mut TestAppContext) {
        init_test(cx);

        cx.update(|cx| {
            SettingsStore::update_global(cx, |store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.workspace.pinned_projects = Some(vec![
                        "/recent/project-03".to_string(),
                        "/pinned/project-only".to_string(),
                    ]);
                });
            });
        });

        let (picker, cx) = cx.add_window_view(|window, cx| {
            let mut delegate = RecentProjectsDelegate::new(
                WeakEntity::new_invalid(),
                false,
                cx.focus_handle(),
                Vec::new(),
                Vec::new(),
                ProjectPickerStyle::Modal,
                cx,
            );
            delegate.set_workspaces(recent_workspaces());
            Picker::list(delegate, window, cx)
                .list_measure_all()
                .show_scrollbar(true)
        });
        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches(String::new(), window, cx);
        });

        picker.update(cx, |picker, _| {
            let entries = &picker.delegate.filtered_entries;
            assert!(matches!(
                entries.first(),
                Some(ProjectPickerEntry::Header(title)) if title.as_ref() == "Pinned Projects"
            ));
            assert!(matches!(
                entries.get(1),
                Some(ProjectPickerEntry::PinnedProject(hit)) if hit.candidate_id == 0
            ));
            assert!(matches!(
                entries.get(2),
                Some(ProjectPickerEntry::PinnedProject(hit)) if hit.candidate_id == 1
            ));
            assert!(
                !entries.iter().any(|entry| matches!(
                    entry,
                    ProjectPickerEntry::RecentProject(hit) if hit.candidate_id == 3
                )),
                "pinned recent project should not also render as a recent"
            );
        });

        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches("project".to_string(), window, cx);
        });

        picker.update(cx, |picker, _| {
            let entries = &picker.delegate.filtered_entries;
            assert!(matches!(
                entries.first(),
                Some(ProjectPickerEntry::Header(title)) if title.as_ref() == "Pinned Projects"
            ));
            assert!(matches!(
                entries.get(1),
                Some(ProjectPickerEntry::PinnedProject(hit)) if hit.candidate_id == 0
            ));
            assert!(matches!(
                entries.get(2),
                Some(ProjectPickerEntry::PinnedProject(hit)) if hit.candidate_id == 1
            ));
            assert!(
                !entries.iter().any(|entry| matches!(
                    entry,
                    ProjectPickerEntry::RecentProject(hit) if hit.candidate_id == 3
                )),
                "pinned recent project should not also render as a recent while filtering"
            );
        });

        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches(String::new(), window, cx);
            picker.set_selected_index(1, None, false, window, cx);
            picker
                .delegate
                .move_pinned_project(0, PinnedProjectMoveDirection::Down, window, cx);
            picker.update_matches(String::new(), window, cx);
        });

        picker.update(cx, |picker, _| {
            let Some(ProjectPickerEntry::PinnedProject(hit)) = picker
                .delegate
                .filtered_entries
                .get(picker.delegate.selected_index)
            else {
                panic!("expected moved pinned project to remain selected");
            };
            assert_eq!(
                picker.delegate.pinned_projects[hit.candidate_id].setting_path,
                "/recent/project-03"
            );
        });

        picker.update_in(cx, |picker, window, cx| {
            let candidate_id = match picker
                .delegate
                .filtered_entries
                .get(picker.delegate.selected_index)
            {
                Some(ProjectPickerEntry::PinnedProject(hit)) => hit.candidate_id,
                _ => panic!("expected pinned project selection before removal"),
            };
            picker
                .delegate
                .remove_pinned_project(candidate_id, window, cx);
            picker.update_matches(String::new(), window, cx);
        });

        picker.update(cx, |picker, _| {
            let Some(ProjectPickerEntry::PinnedProject(hit)) = picker
                .delegate
                .filtered_entries
                .get(picker.delegate.selected_index)
            else {
                panic!("expected replacement pinned project to remain selected");
            };
            assert_eq!(
                picker.delegate.pinned_projects[hit.candidate_id].setting_path,
                "/pinned/project-only"
            );
        });
    }

    #[gpui::test]
    fn this_window_project_icons_use_each_project_group_host(cx: &mut TestAppContext) {
        init_test(cx);

        let mut delegate = cx.update(|cx| {
            RecentProjectsDelegate::new(
                WeakEntity::new_invalid(),
                false,
                cx.focus_handle(),
                Vec::new(),
                vec![project_group(0), remote_project_group(1)],
                ProjectPickerStyle::Modal,
                cx,
            )
        });
        delegate.filtered_entries = vec![
            ProjectPickerEntry::ProjectGroup(StringMatch {
                candidate_id: 0,
                score: 0.0,
                positions: Vec::new(),
                string: Default::default(),
            }),
            ProjectPickerEntry::ProjectGroup(StringMatch {
                candidate_id: 1,
                score: 0.0,
                positions: Vec::new(),
                string: Default::default(),
            }),
        ];

        assert!(!delegate.entry_is_remote_project(&delegate.filtered_entries[0]));
        assert!(delegate.entry_is_remote_project(&delegate.filtered_entries[1]));
        assert!(delegate.filtered_entries_include_remote_project());
        assert_eq!(
            icon_for_project_group(&delegate.window_project_groups[0]),
            IconName::Screen
        );
        assert_eq!(
            icon_for_project_group(&delegate.window_project_groups[1]),
            IconName::Server
        );
    }

    #[gpui::test]
    fn deleting_top_recent_project_preserves_scroll_position(cx: &mut TestAppContext) {
        let target = FIRST_RECENT_PROJECT;
        let (picker, cx) = build_picker(cx);
        let scroll_top = scroll_to_and_select(&picker, cx, target);
        assert!(
            scroll_top > 0,
            "test should start scrolled away from the top"
        );

        delete_recent_project_in_picker(&picker, cx, target);
        assert_scroll_top_is(&picker, cx, scroll_top, "after delete");

        // The picker re-runs layout on the next frame; the scroll position
        // must still be preserved after that redraw.
        draw(cx);
        assert_scroll_top_is(&picker, cx, scroll_top, "after redraw");
    }

    #[gpui::test]
    fn deleting_middle_recent_project_preserves_scroll_position(cx: &mut TestAppContext) {
        let target = FIRST_RECENT_PROJECT + RECENT_PROJECT_COUNT / 2;
        let (picker, cx) = build_picker(cx);
        let scroll_top = scroll_to_and_select(&picker, cx, target);
        assert!(
            scroll_top > 0,
            "test should start scrolled away from the top"
        );

        delete_recent_project_in_picker(&picker, cx, target);
        assert_scroll_top_is(&picker, cx, scroll_top, "after delete");

        draw(cx);
        assert_scroll_top_is(&picker, cx, scroll_top, "after redraw");
    }

    #[gpui::test]
    fn deleting_last_recent_project_preserves_scroll_position(cx: &mut TestAppContext) {
        let target = LAST_RECENT_PROJECT;
        let (picker, cx) = build_picker(cx);
        scroll_to_and_select(&picker, cx, target);

        picker.update(cx, |picker, _| {
            assert_eq!(
                picker.is_scrolled_to_end(),
                Some(true),
                "selecting the last entry should leave the picker pinned to the bottom"
            );
        });

        delete_recent_project_in_picker(&picker, cx, target);
        assert_pinned_to_bottom(&picker, cx, "after delete");

        draw(cx);
        assert_pinned_to_bottom(&picker, cx, "after redraw");
    }

    #[gpui::test]
    async fn test_open_dev_container_action_with_single_config(cx: &mut TestAppContext) {
        let app_state = init_test(cx);

        app_state
            .fs
            .as_fake()
            .insert_tree(
                path!("/project"),
                json!({
                    ".devcontainer": {
                        "devcontainer.json": "{}"
                    },
                    "src": {
                        "main.rs": "fn main() {}"
                    }
                }),
            )
            .await;

        // Open a file path (not a directory) so that the worktree root is a
        // file. This means `active_project_directory` returns `None`, which
        // causes `DevContainerContext::from_workspace` to return `None`,
        // preventing `open_dev_container` from spawning real I/O (docker
        // commands, shell environment loading) that is incompatible with the
        // test scheduler. The modal is still created and the re-entrancy
        // guard that this test validates is still exercised.
        cx.update(|cx| {
            open_paths(
                &[PathBuf::from(path!("/project/src/main.rs"))],
                app_state,
                workspace::OpenOptions::default(),
                cx,
            )
        })
        .await
        .unwrap();

        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        let multi_workspace = cx.update(|cx| cx.windows()[0].downcast::<MultiWorkspace>().unwrap());

        cx.run_until_parked();

        // This dispatch triggers with_active_or_new_workspace -> MultiWorkspace::update
        // -> Workspace::update -> toggle_modal -> new_dev_container.
        // Before the fix, this panicked with "cannot read workspace::Workspace while
        // it is already being updated" because new_dev_container and open_dev_container
        // tried to read the Workspace entity through a WeakEntity handle while it was
        // already leased by the outer update.
        cx.dispatch_action(*multi_workspace, OpenDevContainer);

        multi_workspace
            .update(cx, |multi_workspace, _, cx| {
                let modal = multi_workspace
                    .workspace()
                    .read(cx)
                    .active_modal::<RemoteServerProjects>(cx);
                assert!(
                    modal.is_some(),
                    "Dev container modal should be open after dispatching OpenDevContainer"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    async fn test_open_dev_container_action_with_multiple_configs(cx: &mut TestAppContext) {
        let app_state = init_test(cx);

        app_state
            .fs
            .as_fake()
            .insert_tree(
                path!("/project"),
                json!({
                    ".devcontainer": {
                        "rust": {
                            "devcontainer.json": "{}"
                        },
                        "python": {
                            "devcontainer.json": "{}"
                        }
                    },
                    "src": {
                        "main.rs": "fn main() {}"
                    }
                }),
            )
            .await;

        cx.update(|cx| {
            open_paths(
                &[PathBuf::from(path!("/project"))],
                app_state,
                workspace::OpenOptions::default(),
                cx,
            )
        })
        .await
        .unwrap();

        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        let multi_workspace = cx.update(|cx| cx.windows()[0].downcast::<MultiWorkspace>().unwrap());

        cx.run_until_parked();

        cx.dispatch_action(*multi_workspace, OpenDevContainer);

        multi_workspace
            .update(cx, |multi_workspace, _, cx| {
                let modal = multi_workspace
                    .workspace()
                    .read(cx)
                    .active_modal::<RemoteServerProjects>(cx);
                assert!(
                    modal.is_some(),
                    "Dev container modal should be open after dispatching OpenDevContainer with multiple configs"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    async fn test_open_local_project_reuses_multi_workspace_window(cx: &mut TestAppContext) {
        let app_state = init_test(cx);

        // Disable system path prompts so the injected mock is used.
        cx.update(|cx| {
            SettingsStore::update_global(cx, |store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.workspace.use_system_path_prompts = Some(false);
                });
            });
            DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
            assert!(DisableAiSettings::get_global(cx).disable_ai);
        });

        app_state
            .fs
            .as_fake()
            .insert_tree(
                path!("/initial-project"),
                json!({ "src": { "main.rs": "" } }),
            )
            .await;
        app_state
            .fs
            .as_fake()
            .insert_tree(path!("/new-project"), json!({ "lib": { "mod.rs": "" } }))
            .await;

        cx.update(|cx| {
            open_paths(
                &[PathBuf::from(path!("/initial-project"))],
                app_state.clone(),
                workspace::OpenOptions::default(),
                cx,
            )
        })
        .await
        .unwrap();

        let initial_window_count = cx.update(|cx| cx.windows().len());
        assert_eq!(initial_window_count, 1);

        let multi_workspace = cx.update(|cx| cx.windows()[0].downcast::<MultiWorkspace>().unwrap());
        cx.run_until_parked();

        let workspace = multi_workspace
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        // Set up the prompt mock to return the new project path.
        workspace.update(cx, |workspace, _cx| {
            workspace.set_prompt_for_open_path(Box::new(|_, _, _, _| {
                let (tx, rx) = futures::channel::oneshot::channel();
                tx.send(Some(vec![PathBuf::from(path!("/new-project"))]))
                    .ok();
                rx
            }));
        });

        // Call open_local_project with create_new_window: false.
        let weak_workspace = workspace.downgrade();
        multi_workspace
            .update(cx, |_, window, cx| {
                open_local_project(weak_workspace, false, window, cx);
            })
            .unwrap();

        cx.run_until_parked();

        // Should NOT have opened a new window.
        let final_window_count = cx.update(|cx| cx.windows().len());
        assert_eq!(
            final_window_count, initial_window_count,
            "open_local_project with create_new_window=false should reuse the current multi-workspace window"
        );

        multi_workspace
            .read_with(cx, |mw, cx| {
                assert!(!mw.sidebar_ui_enabled(cx));
                assert!(!mw.sidebar_open());
                assert_eq!(mw.workspaces().count(), 2);
                assert_eq!(mw.project_group_keys().len(), 2);
            })
            .unwrap();
    }

    #[gpui::test]
    async fn test_open_local_project_new_window_creates_new_window(cx: &mut TestAppContext) {
        let app_state = init_test(cx);

        // Disable system path prompts so the injected mock is used.
        cx.update(|cx| {
            SettingsStore::update_global(cx, |store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.workspace.use_system_path_prompts = Some(false);
                });
            });
        });

        app_state
            .fs
            .as_fake()
            .insert_tree(
                path!("/initial-project"),
                json!({ "src": { "main.rs": "" } }),
            )
            .await;
        app_state
            .fs
            .as_fake()
            .insert_tree(path!("/new-project"), json!({ "lib": { "mod.rs": "" } }))
            .await;

        cx.update(|cx| {
            open_paths(
                &[PathBuf::from(path!("/initial-project"))],
                app_state.clone(),
                workspace::OpenOptions::default(),
                cx,
            )
        })
        .await
        .unwrap();

        let initial_window_count = cx.update(|cx| cx.windows().len());
        assert_eq!(initial_window_count, 1);

        let multi_workspace = cx.update(|cx| cx.windows()[0].downcast::<MultiWorkspace>().unwrap());
        cx.run_until_parked();

        let workspace = multi_workspace
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        // Set up the prompt mock to return the new project path.
        workspace.update(cx, |workspace, _cx| {
            workspace.set_prompt_for_open_path(Box::new(|_, _, _, _| {
                let (tx, rx) = futures::channel::oneshot::channel();
                tx.send(Some(vec![PathBuf::from(path!("/new-project"))]))
                    .ok();
                rx
            }));
        });

        // Call open_local_project with create_new_window: true.
        let weak_workspace = workspace.downgrade();
        multi_workspace
            .update(cx, |_, window, cx| {
                open_local_project(weak_workspace, true, window, cx);
            })
            .unwrap();

        cx.run_until_parked();

        // Should have opened a new window.
        let final_window_count = cx.update(|cx| cx.windows().len());
        assert_eq!(
            final_window_count,
            initial_window_count + 1,
            "open_local_project with create_new_window=true should open a new window"
        );
    }

    fn init_test(cx: &mut TestAppContext) -> Arc<AppState> {
        cx.update(|cx| {
            let state = AppState::test(cx);
            crate::init(cx);
            editor::init(cx);
            DisableAiSettings::register(cx);
            state
        })
    }
}
