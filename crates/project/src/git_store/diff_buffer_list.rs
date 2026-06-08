use anyhow::Result;
use buffer_diff::BufferDiff;
use collections::{HashMap, HashSet};
use futures::StreamExt;
use git::{
    repository::RepoPath,
    status::{DiffTreeType, FileStatus, StatusCode, TrackedStatus, TreeDiff, TreeDiffStatus},
};
use gpui::{
    App, AsyncApp, Context, Entity, EventEmitter, SharedString, Subscription, Task, WeakEntity,
};

use language::Buffer;
use sum_tree::SumTree;
use text::BufferId;
use util::ResultExt;
use ztracing::instrument;

use crate::{
    ConflictSet,
    git_store::{
        GitStore, GitStoreEvent, Repository, RepositoryEvent, RepositorySnapshot, StatusEntry,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DiffBase {
    Head,
    Index,
    Staged,
    Merge { base_ref: SharedString },
    Since { base_ref: SharedString },
}

impl DiffBase {
    pub fn is_merge_base(&self) -> bool {
        matches!(self, DiffBase::Merge { .. })
    }

    pub fn requires_tree_diff(&self) -> bool {
        matches!(self, DiffBase::Merge { .. } | DiffBase::Since { .. })
    }

    /// The committed and worktree tree diffs backing this base, if it needs any.
    ///
    /// `Merge` resolves both halves through the merge base of `base_ref` and `head`;
    /// `Since` compares against `base_ref` exactly, so the two never collide even
    /// when they name the same ref.
    fn tree_diff_types(&self, head: SharedString) -> Option<(DiffTreeType, DiffTreeType)> {
        match self {
            DiffBase::Head | DiffBase::Index | DiffBase::Staged => None,
            DiffBase::Merge { base_ref } => Some((
                DiffTreeType::MergeBase {
                    base: base_ref.clone(),
                    head,
                },
                DiffTreeType::MergeBaseWithWorktree {
                    base: base_ref.clone(),
                },
            )),
            DiffBase::Since { base_ref } => Some((
                DiffTreeType::Since {
                    base: base_ref.clone(),
                    head,
                },
                DiffTreeType::SinceWithWorktree {
                    base: base_ref.clone(),
                },
            )),
        }
    }
}

pub struct DiffBufferList {
    diff_base: DiffBase,
    repo: Option<Entity<Repository>>,
    git_store: WeakEntity<GitStore>,
    committed_tree_diff: Option<TreeDiff>,
    tree_diff: Option<TreeDiff>,
    statuses_by_path: Option<SumTree<StatusEntry>>,
    tree_diff_base_task: Option<Task<()>>,
    _subscription: Subscription,
    update_needed: postage::watch::Sender<()>,
    _task: Task<()>,
}

pub enum BranchDiffEvent {
    FileListChanged,
    DiffBaseChanged,
}

impl EventEmitter<BranchDiffEvent> for DiffBufferList {}

impl DiffBufferList {
    pub fn new(
        source: DiffBase,
        git_store: Entity<GitStore>,
        repo: Option<Entity<Repository>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let repo = repo.or_else(|| git_store.read(cx).active_repository());
        let git_store_subscription = cx.subscribe(&git_store, move |this, _, event, cx| {
            let should_update = match event {
                GitStoreEvent::ActiveRepositoryChanged(new_repo_id) => {
                    this.repo.is_none() && new_repo_id.is_some()
                }
                GitStoreEvent::RepositoryUpdated(
                    event_repo_id,
                    RepositoryEvent::StatusesChanged
                    | RepositoryEvent::HeadChanged
                    | RepositoryEvent::BranchListChanged,
                    _,
                ) => this
                    .repo
                    .as_ref()
                    .is_some_and(|repo| repo.read(cx).snapshot().id == *event_repo_id),
                _ => false,
            };

            if should_update {
                // Tree-diff-backed lists refresh after the tree diff reloads; other
                // bases have no tree diff, so notify consumers immediately.
                if !this.diff_base.requires_tree_diff() {
                    cx.emit(BranchDiffEvent::FileListChanged);
                }
                *this.update_needed.borrow_mut() = ();
            }
        });

        let (send, recv) = postage::watch::channel::<()>();
        let worker =
            cx.spawn(async move |this, cx| Self::handle_status_updates(this, recv, cx).await);

        Self {
            diff_base: source,
            repo,
            git_store: git_store.downgrade(),
            committed_tree_diff: None,
            tree_diff: None,
            statuses_by_path: None,
            tree_diff_base_task: None,
            _subscription: git_store_subscription,
            _task: worker,
            update_needed: send,
        }
    }

    pub fn diff_base(&self) -> &DiffBase {
        &self.diff_base
    }

    pub fn set_repo(&mut self, repo: Option<Entity<Repository>>, cx: &mut Context<Self>) {
        let same_repo = match (self.repo.as_ref(), repo.as_ref()) {
            (Some(current), Some(new)) => current.read(cx).id == new.read(cx).id,
            (None, None) => true,
            _ => false,
        };
        if same_repo {
            return;
        }

        self.repo = repo;
        self.committed_tree_diff = None;
        self.tree_diff = None;
        self.statuses_by_path = None;
        self.tree_diff_base_task = None;
        cx.emit(BranchDiffEvent::FileListChanged);
        *self.update_needed.borrow_mut() = ();
    }

    pub fn set_diff_base(&mut self, diff_base: DiffBase, cx: &mut Context<Self>) {
        if self.diff_base == diff_base {
            *self.update_needed.borrow_mut() = ();
            return;
        }

        self.committed_tree_diff = None;
        self.tree_diff = None;
        self.statuses_by_path = None;
        self.tree_diff_base_task = None;
        self.diff_base = diff_base;

        cx.emit(BranchDiffEvent::DiffBaseChanged);
        *self.update_needed.borrow_mut() = ();
    }

    pub async fn handle_status_updates(
        this: WeakEntity<Self>,
        mut recv: postage::watch::Receiver<()>,
        cx: &mut AsyncApp,
    ) {
        this.update(cx, |this, cx| this.spawn_reload_tree_diff(cx))
            .log_err();
        while recv.next().await.is_some() {
            let Ok(()) = this.update(cx, |this, cx| {
                if this.repo.is_none() {
                    this.repo = this
                        .git_store
                        .upgrade()
                        .and_then(|git_store| git_store.read(cx).active_repository());
                }
                this.spawn_reload_tree_diff(cx);
            }) else {
                return;
            };
        }
    }

    pub fn status_for_buffer_id(&self, buffer_id: BufferId, cx: &App) -> Option<FileStatus> {
        let git_store = self.git_store.upgrade()?;
        let (repo, path) = git_store
            .read(cx)
            .repository_and_path_for_buffer_id(buffer_id, cx)?;
        (self.repo() == Some(&repo))
            .then(|| self.status_for_path(&path, cx))
            .flatten()
    }

    pub fn status_for_path(&self, path: &RepoPath, cx: &App) -> Option<FileStatus> {
        let repo_status = self
            .repo
            .as_ref()
            .and_then(|repo| repo.read(cx).status_for_path(path))
            .map(|entry| entry.status);
        if repo_status.is_some_and(|status| {
            status_overrides_tree(
                status,
                self.committed_tree_diff
                    .as_ref()
                    .and_then(|diff| diff.entries.get(path)),
            )
        }) {
            return repo_status;
        }
        self.tree_diff
            .as_ref()
            .and_then(|diff| diff.entries.get(path))
            .map(diff_status_to_file_status)
    }

    pub fn statuses_by_path(&self) -> Option<SumTree<StatusEntry>> {
        self.statuses_by_path.clone()
    }

    pub fn base_oid_for_path(&self, path: &RepoPath) -> Option<Option<git::Oid>> {
        let status = self
            .tree_diff
            .as_ref()
            .and_then(|diff| diff.entries.get(path))
            .or_else(|| {
                self.committed_tree_diff
                    .as_ref()
                    .and_then(|diff| diff.entries.get(path))
            })?;
        Some(match status {
            TreeDiffStatus::Added => None,
            TreeDiffStatus::Modified { old } | TreeDiffStatus::Deleted { old } => Some(*old),
        })
    }

    fn spawn_reload_tree_diff(&mut self, cx: &mut Context<Self>) {
        if !self.diff_base.requires_tree_diff() {
            return;
        }

        let task = cx.spawn(async move |this, cx| {
            Self::reload_tree_diff(this, cx).await.log_err();
        });

        self.tree_diff_base_task = Some(task);
        cx.notify();
    }

    pub fn is_tree_base_loading(&self) -> bool {
        self.tree_diff_base_task
            .as_ref()
            .is_some_and(|task| !task.is_ready())
    }

    pub async fn reload_tree_diff(this: WeakEntity<Self>, cx: &mut AsyncApp) -> Result<()> {
        let tasks = this.update(cx, |this, cx| {
            let Some((committed_type, worktree_type)) =
                this.diff_base.tree_diff_types("HEAD".into())
            else {
                return None;
            };
            let Some(repo) = this.repo.as_ref() else {
                this.committed_tree_diff.take();
                this.tree_diff.take();
                this.statuses_by_path.take();
                return None;
            };
            Some(repo.update(cx, |repo, cx| {
                (
                    repo.diff_tree(committed_type, cx),
                    repo.diff_tree(worktree_type, cx),
                )
            }))
        })?;
        let Some((committed_task, worktree_task)) = tasks else {
            return Ok(());
        };

        let (committed_tree_diff, tree_diff) = futures::try_join!(committed_task, worktree_task)?;
        let committed_tree_diff = committed_tree_diff?;
        let tree_diff = tree_diff?;
        this.update(cx, |this, cx| {
            let statuses_by_path = this.repo.as_ref().map(|repo| {
                build_statuses(&repo.read(cx).snapshot, &committed_tree_diff, &tree_diff)
            });
            this.committed_tree_diff = Some(committed_tree_diff);
            this.tree_diff = Some(tree_diff);
            this.statuses_by_path = statuses_by_path;
            cx.emit(BranchDiffEvent::FileListChanged);
            cx.notify();
        })
    }

    pub fn repo(&self) -> Option<&Entity<Repository>> {
        self.repo.as_ref()
    }

    #[instrument(skip_all)]
    pub fn load_buffers(&mut self, cx: &mut Context<Self>) -> Vec<DiffBuffer> {
        let mut output = Vec::default();
        let Some(repo) = self.repo.clone() else {
            return output;
        };
        if self.diff_base.requires_tree_diff() && self.tree_diff.is_none() {
            return output;
        }

        let Some(git_store) = self.git_store.upgrade() else {
            return output;
        };
        {
            let mut seen = HashSet::default();

            for item in repo.read(cx).cached_status() {
                let status = match self.diff_base {
                    DiffBase::Head => Some(item.status),
                    DiffBase::Index => item.status.staging().has_unstaged().then_some(item.status),
                    DiffBase::Staged => item.status.staging().has_staged().then_some(item.status),
                    DiffBase::Merge { .. } | DiffBase::Since { .. } => status_overrides_tree(
                        item.status,
                        self.committed_tree_diff
                            .as_ref()
                            .and_then(|diff| diff.entries.get(&item.repo_path)),
                    )
                    .then_some(item.status),
                };
                let Some(status) = status.filter(|status| status.has_changes()) else {
                    continue;
                };
                seen.insert(item.repo_path.clone());

                let Some(project_path) =
                    repo.read(cx).repo_path_to_project_path(&item.repo_path, cx)
                else {
                    continue;
                };
                let branch_diff = self
                    .tree_diff
                    .as_ref()
                    .and_then(|tree| tree.entries.get(&item.repo_path))
                    .cloned();
                let task = Self::load_buffer(
                    self.diff_base.clone(),
                    branch_diff,
                    project_path,
                    repo.clone(),
                    git_store.clone(),
                    cx,
                );

                output.push(DiffBuffer {
                    repo_path: item.repo_path.clone(),
                    load: task,
                    file_status: status,
                });
            }
            let Some(tree_diff) = self.tree_diff.as_ref() else {
                output.sort_by(|left, right| left.repo_path.cmp(&right.repo_path));
                return output;
            };

            for (path, branch_diff) in tree_diff.entries.iter() {
                if seen.contains(path) {
                    continue;
                }

                let Some(project_path) = repo.read(cx).repo_path_to_project_path(path, cx) else {
                    continue;
                };
                let task = Self::load_buffer(
                    self.diff_base.clone(),
                    Some(branch_diff.clone()),
                    project_path,
                    repo.clone(),
                    git_store.clone(),
                    cx,
                );

                output.push(DiffBuffer {
                    repo_path: path.clone(),
                    load: task,
                    file_status: diff_status_to_file_status(branch_diff),
                });
            }
        }
        output.sort_by(|left, right| left.repo_path.cmp(&right.repo_path));
        output
    }

    #[instrument(skip_all)]
    fn load_buffer(
        diff_base: DiffBase,
        branch_diff: Option<git::status::TreeDiffStatus>,
        project_path: crate::ProjectPath,
        repo: Entity<Repository>,
        git_store: Entity<GitStore>,
        cx: &Context<Self>,
    ) -> Task<Result<LoadedDiffBuffer>> {
        cx.spawn(async move |_, cx| {
            let buffer = git_store
                .update(cx, |git_store, cx| {
                    git_store.buffer_store.update(cx, |buffer_store, cx| {
                        buffer_store.open_buffer(project_path, cx)
                    })
                })
                .await?;

            let main_buffer = buffer.clone();
            let load_conflict_set = diff_base != DiffBase::Staged;
            let (display_buffer, changes) = match diff_base {
                DiffBase::Head => {
                    let diff = git_store
                        .update(cx, |git_store, cx| {
                            git_store.open_uncommitted_diff(buffer.clone(), cx)
                        })
                        .await?;
                    (buffer, diff)
                }
                DiffBase::Index => {
                    let diff = git_store
                        .update(cx, |git_store, cx| {
                            git_store.open_unstaged_diff(buffer.clone(), cx)
                        })
                        .await?;
                    (buffer, diff)
                }
                DiffBase::Staged => {
                    let (diff, index_buffer) = git_store
                        .update(cx, |git_store, cx| {
                            git_store.open_staged_diff(buffer.clone(), cx)
                        })
                        .await?;
                    (index_buffer, diff)
                }
                DiffBase::Merge { .. } | DiffBase::Since { .. } => {
                    let diff = if let Some(entry) = branch_diff {
                        let oid = match entry {
                            git::status::TreeDiffStatus::Added { .. } => None,
                            git::status::TreeDiffStatus::Modified { old, .. }
                            | git::status::TreeDiffStatus::Deleted { old } => Some(old),
                        };
                        git_store
                            .update(cx, |git_store, cx| {
                                git_store.open_diff_since(oid, buffer.clone(), repo, cx)
                            })
                            .await?
                    } else {
                        git_store
                            .update(cx, |git_store, cx| {
                                git_store.open_uncommitted_diff(buffer.clone(), cx)
                            })
                            .await?
                    };
                    (buffer, diff)
                }
            };
            let conflict_set = if load_conflict_set {
                Some(
                    git_store
                        .update(cx, |git_store, cx| {
                            git_store.open_conflict_set(main_buffer.clone(), cx)
                        })
                        .await,
                )
            } else {
                None
            };
            Ok(LoadedDiffBuffer {
                display_buffer,
                main_buffer,
                diff: changes,
                conflict_set,
            })
        })
    }
}

fn build_statuses(
    snapshot: &RepositorySnapshot,
    committed_tree_diff: &TreeDiff,
    tree_diff: &TreeDiff,
) -> SumTree<StatusEntry> {
    let mut entries = tree_diff
        .entries
        .iter()
        .map(|(repo_path, status)| {
            (
                repo_path.clone(),
                StatusEntry {
                    repo_path: repo_path.clone(),
                    status: diff_status_to_file_status(status),
                    diff_stat: None,
                    staged_diff_stat: None,
                    unstaged_diff_stat: None,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    for entry in snapshot.statuses_by_path.iter() {
        if status_overrides_tree(
            entry.status,
            committed_tree_diff.entries.get(&entry.repo_path),
        ) {
            entries.insert(entry.repo_path.clone(), entry.clone());
        }
    }
    let mut entries = entries.into_values().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.repo_path.cmp(&right.repo_path));
    SumTree::from_iter(entries, ())
}

fn status_overrides_tree(status: FileStatus, committed_status: Option<&TreeDiffStatus>) -> bool {
    status.is_conflicted()
        || status.is_untracked()
            && !matches!(committed_status, Some(TreeDiffStatus::Deleted { .. }))
}

fn diff_status_to_file_status(branch_diff: &git::status::TreeDiffStatus) -> FileStatus {
    let file_status = match branch_diff {
        git::status::TreeDiffStatus::Added { .. } => FileStatus::Tracked(TrackedStatus {
            index_status: StatusCode::Added,
            worktree_status: StatusCode::Added,
        }),
        git::status::TreeDiffStatus::Modified { .. } => FileStatus::Tracked(TrackedStatus {
            index_status: StatusCode::Modified,
            worktree_status: StatusCode::Modified,
        }),
        git::status::TreeDiffStatus::Deleted { .. } => FileStatus::Tracked(TrackedStatus {
            index_status: StatusCode::Deleted,
            worktree_status: StatusCode::Deleted,
        }),
    };
    file_status
}

#[derive(Debug)]
pub struct LoadedDiffBuffer {
    pub display_buffer: Entity<Buffer>,
    pub main_buffer: Entity<Buffer>,
    pub diff: Entity<BufferDiff>,
    pub conflict_set: Option<Entity<ConflictSet>>,
}

#[derive(Debug)]
pub struct DiffBuffer {
    pub repo_path: RepoPath,
    pub file_status: FileStatus,
    pub load: Task<Result<LoadedDiffBuffer>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_base_tree_diff_types_distinguish_merge_base_from_exact_since() {
        let merge = DiffBase::Merge {
            base_ref: "main".into(),
        }
        .tree_diff_types("HEAD".into());
        let since = DiffBase::Since {
            base_ref: "abc123".into(),
        }
        .tree_diff_types("HEAD".into());

        match merge {
            Some((
                DiffTreeType::MergeBase { base, head },
                DiffTreeType::MergeBaseWithWorktree {
                    base: worktree_base,
                },
            )) => {
                assert_eq!(base.as_ref(), "main");
                assert_eq!(head.as_ref(), "HEAD");
                assert_eq!(worktree_base.as_ref(), "main");
            }
            _ => panic!("merge base diff should use the merge-base tree diff pair"),
        }

        match since {
            Some((
                DiffTreeType::Since { base, head },
                DiffTreeType::SinceWithWorktree {
                    base: worktree_base,
                },
            )) => {
                assert_eq!(base.as_ref(), "abc123");
                assert_eq!(head.as_ref(), "HEAD");
                assert_eq!(worktree_base.as_ref(), "abc123");
            }
            _ => panic!("exact since diff should use the exact tree diff pair"),
        }

        for base in [DiffBase::Head, DiffBase::Index, DiffBase::Staged] {
            assert!(base.tree_diff_types("HEAD".into()).is_none());
        }
    }

    #[test]
    fn diff_base_requires_tree_diff_only_for_tree_backed_bases() {
        assert!(
            DiffBase::Merge {
                base_ref: "main".into()
            }
            .requires_tree_diff()
        );
        assert!(
            DiffBase::Since {
                base_ref: "abc123".into()
            }
            .requires_tree_diff()
        );
        assert!(!DiffBase::Head.requires_tree_diff());
        assert!(!DiffBase::Index.requires_tree_diff());
        assert!(!DiffBase::Staged.requires_tree_diff());
    }

    // `DiffBase` is persisted as JSON in the project diff database, so the
    // externally tagged representation of the existing variants must not shift
    // when a new one is added.
    #[test]
    fn diff_base_serde_representation_is_stable() {
        assert_eq!(serde_json::to_string(&DiffBase::Head).unwrap(), r#""Head""#);
        assert_eq!(
            serde_json::to_string(&DiffBase::Merge {
                base_ref: "main".into()
            })
            .unwrap(),
            r#"{"Merge":{"base_ref":"main"}}"#
        );
        assert_eq!(
            serde_json::to_string(&DiffBase::Since {
                base_ref: "abc123".into()
            })
            .unwrap(),
            r#"{"Since":{"base_ref":"abc123"}}"#
        );

        let since = DiffBase::Since {
            base_ref: "abc123".into(),
        };
        let round_tripped: DiffBase =
            serde_json::from_str(&serde_json::to_string(&since).unwrap()).unwrap();
        assert_eq!(round_tripped, since);
    }
}
