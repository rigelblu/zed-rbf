//! `#zed-76`: the one `Locate…` flow behind both entry points — the Manage modal and the
//! switch-time recovery prompt — so the dogfooded name-mismatch confirm and the collision
//! rules can't drift between them.
//!
//! pick → name-mismatch confirm → collision check → `RelocateWorkspace` (or, for
//! Collision Policy 2a, `RelinkWorkspace`) → [`LocateOutcome`].

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use gpui::{
    App, AppContext as _, AsyncWindowContext, PathPromptOptions, PromptLevel, SharedString, Task,
    WeakEntity, WindowId,
};
use util::path_list::PathList;

use crate::{
    MultiWorkspace, WorkspaceId,
    persistence::{
        WorkspaceConfigurationMutation, WorkspaceConfigurationStore,
        model::{WorkspaceConfiguration, WorkspaceConfigurationId},
    },
};

/// One missing folder of one configuration member.
#[derive(Clone, Debug)]
pub(crate) struct UnavailableWorkspaceMember {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) old_root: PathBuf,
    /// The missing folder's name, as `Locate “<label>”…` shows it.
    pub(crate) label: String,
}

impl UnavailableWorkspaceMember {
    pub(crate) fn new(workspace_id: WorkspaceId, old_root: PathBuf) -> Self {
        let label = old_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| old_root.to_string_lossy().into_owned());
        Self {
            workspace_id,
            old_root,
            label,
        }
    }
}

/// Which entry point is locating, because Collision Policy 2a differs between them.
pub(crate) enum LocateEntry {
    /// A pick open in this window is rejected: there's no switch to carry it into.
    ManageModal,
    /// A pick that makes the member's full folder set equal a workspace open in the
    /// switching window relinks the member to that workspace. `member_folders` is the
    /// member's saved folder set, which still contains the missing folder.
    Switch { member_folders: Vec<PathBuf> },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LocateOutcome {
    /// The relocate (or relink) was saved. `new_root` is the folder the user picked.
    Relocated { new_root: PathBuf },
    /// The collision check refused the pick. The message names the conflict and reads
    /// after `Cannot relocate: `.
    Collision(String),
    /// The picker was dismissed, or the name-mismatch confirm was canceled.
    Canceled,
}

pub(crate) const CHOOSE_ANOTHER_FOLDER: &str = "Choose Another Folder…";

/// The name-mismatch confirm's buttons. `Choose Another Folder…` comes first so it is the
/// default (Return) button: a reflexive confirm must not commit the folder being asked
/// about.
pub(crate) fn name_mismatch_answers(selected_name: &str) -> [String; 3] {
    [
        CHOOSE_ANOTHER_FOLDER.to_string(),
        format!("Use “{selected_name}”"),
        "Cancel".to_string(),
    ]
}

/// Runs the whole locate flow for `member` in the window `cx` belongs to.
///
/// An `Err` is a failure the caller reports: a picker error, a store write that failed,
/// or a configuration that no longer exists. A relocate that finds the member already
/// moved fails with `WorkspaceMemberAlreadyMoved` (see `workspace_member_was_already_moved`).
pub(crate) async fn locate_workspace_folder(
    multi_workspace: WeakEntity<MultiWorkspace>,
    configuration_id: WorkspaceConfigurationId,
    member: &UnavailableWorkspaceMember,
    entry: &LocateEntry,
    cx: &mut AsyncWindowContext,
) -> Result<LocateOutcome> {
    let Some(selected_path) = pick_workspace_folder(member, cx).await? else {
        return Ok(LocateOutcome::Canceled);
    };
    let window_id = cx.window_handle().window_id();
    let relocate = cx.update(|_window, cx| {
        relocate_to_located_folder(
            &multi_workspace,
            window_id,
            configuration_id,
            member,
            entry,
            selected_path,
            cx,
        )
    })?;
    relocate.await
}

/// The folder picker, then — when the picked folder's name differs from the missing
/// one's — the confirm. `None` means the user backed out.
async fn pick_workspace_folder(
    member: &UnavailableWorkspaceMember,
    cx: &mut AsyncWindowContext,
) -> Result<Option<PathBuf>> {
    loop {
        let prompt = cx.update(|_window, cx| {
            cx.prompt_for_paths(PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some(SharedString::from(format!("Locate “{}”", member.label))),
            })
        })?;
        let selected_path = match prompt.await.context("the folder picker closed")?? {
            Some(mut paths) if !paths.is_empty() => paths.remove(0),
            _ => return Ok(None),
        };

        // The picker opens wherever macOS last left it, and confirming with nothing
        // selected returns that folder. A differently named folder is the signature of
        // that one-click accident, so make the user confirm it before any write.
        if selected_path.file_name() == member.old_root.file_name() {
            return Ok(Some(selected_path));
        }
        let selected_name = selected_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| selected_path.display().to_string());
        let answers = name_mismatch_answers(&selected_name);
        let confirm = cx.update(|window, cx| {
            let answers = answers.iter().map(String::as_str).collect::<Vec<_>>();
            window.prompt(
                PromptLevel::Warning,
                &format!("Relocate “{}” to “{selected_name}”?", member.label),
                Some(&format!(
                    "The folder names don’t match.\n\nFrom: {}\nTo: {}",
                    member.old_root.display(),
                    selected_path.display()
                )),
                &answers,
                cx,
            )
        })?;
        match confirm.await {
            Ok(0) => continue,
            Ok(1) => return Ok(Some(selected_path)),
            _ => return Ok(None),
        }
    }
}

/// The collision check and the durable write for a folder the user already picked.
pub(crate) fn relocate_to_located_folder(
    multi_workspace: &WeakEntity<MultiWorkspace>,
    window_id: WindowId,
    configuration_id: WorkspaceConfigurationId,
    member: &UnavailableWorkspaceMember,
    entry: &LocateEntry,
    selected_path: PathBuf,
    cx: &mut App,
) -> Task<Result<LocateOutcome>> {
    let Some(configuration) = WorkspaceConfigurationStore::try_global(cx)
        .and_then(|store| store.configuration(configuration_id))
    else {
        return Task::ready(Err(anyhow::anyhow!(
            "that workspace configuration no longer exists"
        )));
    };
    let relink_to = match check_located_folder(
        multi_workspace,
        window_id,
        configuration,
        member,
        entry,
        &selected_path,
        cx,
    ) {
        LocatedFolderCheck::Collision(message) => {
            return Task::ready(Ok(LocateOutcome::Collision(message)));
        }
        LocatedFolderCheck::Relocate => None,
        LocatedFolderCheck::RelinkTo(open_workspace_id) => Some(open_workspace_id),
    };

    let mutation = match relink_to {
        None => WorkspaceConfigurationMutation::RelocateWorkspace {
            id: configuration_id,
            workspace_id: member.workspace_id,
            old_root: member.old_root.clone(),
            new_root: selected_path.clone(),
        },
        Some(open_workspace_id) => WorkspaceConfigurationMutation::RelinkWorkspace {
            id: configuration_id,
            workspace_id: member.workspace_id,
            old_root: member.old_root.clone(),
            new_root: selected_path.clone(),
            open_workspace_id,
        },
    };
    let write = WorkspaceConfigurationStore::mutate_global(mutation, cx);
    cx.background_spawn(async move {
        write.await?;
        Ok(LocateOutcome::Relocated {
            new_root: selected_path,
        })
    })
}

enum LocatedFolderCheck {
    Relocate,
    /// Collision Policy 2a: relink to this workspace, open in the switching window.
    RelinkTo(WorkspaceId),
    Collision(String),
}

fn check_located_folder(
    multi_workspace: &WeakEntity<MultiWorkspace>,
    window_id: WindowId,
    configuration: &WorkspaceConfiguration,
    member: &UnavailableWorkspaceMember,
    entry: &LocateEntry,
    selected_path: &Path,
    cx: &App,
) -> LocatedFolderCheck {
    let in_configuration = configuration.members.iter().any(|other| {
        other.workspace_id != member.workspace_id
            && other
                .identity_paths
                .iter()
                .any(|path| path == selected_path)
    });
    if in_configuration {
        return LocatedFolderCheck::Collision(format!(
            "“{}” is already a member of this configuration.",
            selected_path.display()
        ));
    }

    let this_window = multi_workspace.upgrade();
    let this_window_workspaces = this_window
        .as_ref()
        .map(|multi_workspace| {
            multi_workspace
                .read(cx)
                .workspaces()
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Collision Policy 2a, switch time: only an exact match of the member's full folder
    // set after the relocate. A partial match, or a pick that is one folder of an open
    // workspace, falls through to the rejection below.
    let relink_to = match entry {
        LocateEntry::ManageModal => None,
        LocateEntry::Switch { member_folders } => {
            let relocated = PathList::new(
                &member_folders
                    .iter()
                    .map(|folder| {
                        if folder == &member.old_root {
                            selected_path.to_path_buf()
                        } else {
                            folder.clone()
                        }
                    })
                    .collect::<Vec<_>>(),
            );
            this_window_workspaces.iter().find_map(|workspace| {
                let workspace = workspace.read(cx);
                let folders = workspace.project().read(cx).worktree_paths(cx);
                (folders.folder_path_list().paths() == relocated.paths())
                    .then(|| workspace.database_id())
                    .flatten()
            })
        }
    };

    for workspace in &this_window_workspaces {
        let workspace = workspace.read(cx);
        if workspace.database_id() == Some(member.workspace_id) {
            return LocatedFolderCheck::Collision(
                "this workspace is currently open in this window. Close it before relocating."
                    .to_string(),
            );
        }
        if relink_to.is_none()
            && workspace
                .project_group_key(cx)
                .path_list()
                .paths()
                .iter()
                .any(|path| path == selected_path)
        {
            return LocatedFolderCheck::Collision(format!(
                "“{}” is currently open in this window.",
                selected_path.display()
            ));
        }
    }

    let this_window_entity = this_window.as_ref().map(|entity| entity.entity_id());
    let other_windows = cx
        .windows()
        .into_iter()
        .filter_map(|window| window.downcast::<MultiWorkspace>())
        .filter(|window| window.window_id() != window_id)
        .filter_map(|window| window.entity(cx).ok())
        .filter(|entity| Some(entity.entity_id()) != this_window_entity);
    for other_window in other_windows {
        for workspace in other_window.read(cx).workspaces() {
            let workspace = workspace.read(cx);
            if workspace.database_id() == Some(member.workspace_id) {
                return LocatedFolderCheck::Collision(
                    "this workspace is currently open in another window. Close it before relocating."
                        .to_string(),
                );
            }
            if workspace
                .project_group_key(cx)
                .path_list()
                .paths()
                .iter()
                .any(|path| path == selected_path)
            {
                return LocatedFolderCheck::Collision(format!(
                    "“{}” is currently open in another window.",
                    selected_path.display()
                ));
            }
        }
    }

    match relink_to {
        Some(open_workspace_id) => LocatedFolderCheck::RelinkTo(open_workspace_id),
        None => LocatedFolderCheck::Relocate,
    }
}
