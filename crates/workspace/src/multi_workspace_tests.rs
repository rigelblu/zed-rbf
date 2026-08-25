use std::{any::Any, path::PathBuf, sync::Arc};

use super::*;
use crate::item::test::TestItem;
use crate::multi_workspace::{
    ManageWorkspaceConfigurations, SelectNextWorkspaceConfiguration,
    SelectPreviousWorkspaceConfiguration, WorkspaceConfigurationSwitchTestStage,
};
use crate::persistence::WorkspaceConfigurationStore;
use agent_settings::AgentSettings;
use client::proto;
use db::kvp::KeyValueStore;
use fs::{FakeFs, Fs};
use gpui::{
    IntoElement, KeyBinding, MouseButton, TestAppContext, VisualTestContext, WindowId, div,
};
use project::DisableAiSettings;
use serde_json::json;
use settings::{Settings, SettingsStore};
use ui::utils::platform_title_bar_height;
use util::path;

struct TestInputEditor {
    text: String,
    focus_handle: gpui::FocusHandle,
}

#[derive(Clone)]
struct TestErasedEditor(Entity<TestInputEditor>);

impl ui_input::ErasedEditor for TestErasedEditor {
    fn text(&self, cx: &App) -> String {
        self.0.read(cx).text.clone()
    }

    fn set_text(&self, text: &str, _window: &mut Window, cx: &mut App) {
        self.0.update(cx, |editor, cx| {
            editor.text = text.to_string();
            cx.notify();
        });
    }

    fn clear(&self, window: &mut Window, cx: &mut App) {
        self.set_text("", window, cx);
    }

    fn set_placeholder_text(&self, _text: &str, _window: &mut Window, _cx: &mut App) {}

    fn move_selection_to_end(&self, _window: &mut Window, _cx: &mut App) {}

    fn select_all(&self, _window: &mut Window, _cx: &mut App) {}

    fn set_masked(&self, _masked: bool, _window: &mut Window, _cx: &mut App) {}

    fn set_read_only(&self, _read_only: bool, _cx: &mut App) {}

    fn set_multiline(&self, _max_lines: Option<usize>, _window: &mut Window, _cx: &mut App) {}

    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.0.read(cx).focus_handle.clone()
    }

    fn subscribe(
        &self,
        _callback: Box<dyn FnMut(ui_input::ErasedEditorEvent, &mut Window, &mut App) + 'static>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> gpui::Subscription {
        gpui::Subscription::new(|| {})
    }

    fn render(&self, _window: &mut Window, cx: &App) -> ui::AnyElement {
        div().child(self.text(cx)).into_any_element()
    }

    fn as_any(&self) -> &dyn Any {
        &self.0
    }
}

fn test_erased_editor_factory(
    _window: &mut Window,
    cx: &mut App,
) -> Arc<dyn ui_input::ErasedEditor> {
    Arc::new(TestErasedEditor(cx.new(|cx| TestInputEditor {
        text: String::new(),
        focus_handle: cx.focus_handle(),
    })))
}

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        ui_input::ERASED_EDITOR_FACTORY.get_or_init(|| test_erased_editor_factory);
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        DisableAiSettings::register(cx);
        WorkspaceConfigurationStore::init(cx);
        cx.bind_keys([
            KeyBinding::new(
                "tab",
                menu::SelectNext,
                Some("ManageWorkspaceConfigurations"),
            ),
            KeyBinding::new(
                "shift-tab",
                menu::SelectPrevious,
                Some("ManageWorkspaceConfigurations"),
            ),
            KeyBinding::new(
                "enter",
                menu::Confirm,
                Some("ManageWorkspaceConfigurations"),
            ),
            KeyBinding::new(
                "escape",
                menu::Cancel,
                Some("ManageWorkspaceConfigurations"),
            ),
            KeyBinding::new(
                "down",
                SelectNextWorkspaceConfiguration,
                Some("ManageWorkspaceConfigurations"),
            ),
            KeyBinding::new(
                "up",
                SelectPreviousWorkspaceConfiguration,
                Some("ManageWorkspaceConfigurations"),
            ),
        ]);
    });
}

async fn reset_workspace_configuration_store(cx: &mut TestAppContext) {
    let kvp = cx.update(|cx| KeyValueStore::global(cx));
    if let Err(error) = kvp
        .scoped("workspace_configurations")
        .delete("collection".to_string())
        .await
    {
        panic!("failed to reset workspace configurations: {error:#}");
    }
    cx.update(WorkspaceConfigurationStore::reload_for_tests);
}

async fn set_configuration_kvp_query_only(kvp: &KeyValueStore, query_only: bool) {
    let query = if query_only {
        "PRAGMA query_only = ON"
    } else {
        "PRAGMA query_only = OFF"
    };
    if let Err(error) = kvp
        .write(move |connection| connection.exec(query).and_then(|mut statement| statement()))
        .await
    {
        panic!("failed to change configuration KVP query_only state: {error:#}");
    }
}

fn setup_multi_workspace<'a>(
    projects: &[Entity<Project>],
    cx: &'a mut TestAppContext,
) -> (Entity<MultiWorkspace>, &'a mut VisualTestContext) {
    let mut iterator = projects.iter();
    let project = iterator
        .next()
        .expect("At least one project should be provided")
        .clone();

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));

    for project in iterator {
        multi_workspace.update_in(cx, |multi_workspace, window, cx| {
            multi_workspace.test_add_workspace(project.clone(), window, cx);
        })
    }

    // Opening the sidebar retains the workspaces and establishes their project groups.
    multi_workspace.update(cx, |multi_workspace, cx| multi_workspace.open_sidebar(cx));
    cx.run_until_parked();

    (multi_workspace, cx)
}

fn workspace_configuration(
    id: crate::persistence::model::WorkspaceConfigurationId,
    cx: &mut VisualTestContext,
) -> Option<crate::persistence::model::WorkspaceConfiguration> {
    cx.update(|_window, cx| {
        WorkspaceConfigurationStore::global(cx)
            .configuration(id)
            .cloned()
    })
}

fn workspace_configuration_store_is_empty(cx: &mut VisualTestContext) -> bool {
    cx.update(|_window, cx| {
        WorkspaceConfigurationStore::global(cx)
            .configurations()
            .is_empty()
    })
}

fn strict_restore_fixture(
    workspace_id: WorkspaceId,
    path: &str,
    center_group: crate::persistence::model::SerializedPaneGroup,
) -> crate::persistence::model::SerializedWorkspace {
    crate::persistence::model::SerializedWorkspace {
        id: workspace_id,
        paths: PathList::new(&[PathBuf::from(path)]),
        identity_paths: Some(PathList::new(&[PathBuf::from(path)])),
        location: crate::persistence::model::SerializedWorkspaceLocation::Local,
        center_group,
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

async fn seed_workspace_configuration(
    name: &str,
    members: &[(WorkspaceId, &str)],
    active_member: WorkspaceId,
    cx: &mut VisualTestContext,
) -> crate::persistence::model::WorkspaceConfigurationId {
    let db = cx.update(|_window, cx| WorkspaceDb::global(cx));
    for (workspace_id, path) in members {
        db.save_workspace_checked(strict_restore_fixture(
            *workspace_id,
            path,
            crate::persistence::model::SerializedPaneGroup::Pane(
                crate::persistence::model::SerializedPane::new(Vec::new(), true, 0),
            ),
        ))
        .await
        .expect("failed to seed workspace configuration member");
    }
    cx.update(|_window, cx| {
        WorkspaceConfigurationStore::mutate_global(
            crate::persistence::WorkspaceConfigurationMutation::Create {
                name: name.to_string(),
                members: members
                    .iter()
                    .map(|(workspace_id, path)| {
                        crate::persistence::model::WorkspaceConfigurationMember {
                            workspace_id: *workspace_id,
                            identity_paths: vec![PathBuf::from(path)],
                        }
                    })
                    .collect(),
                active_member: Some(active_member),
            },
            cx,
        )
    })
    .await
    .expect("failed to seed workspace configuration")
    .id
}

#[gpui::test]
async fn workspace_configuration_strict_restore_stays_detached(cx: &mut TestAppContext) {
    init_test(cx);
    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project-a"), json!({})).await;
    fs.insert_tree(path!("/project-b"), json!({})).await;

    let open = cx
        .update(|cx| {
            Workspace::new_local(
                vec![PathBuf::from(path!("/project-a"))],
                app_state.clone(),
                None,
                None,
                None,
                OpenMode::Activate,
                cx,
            )
        })
        .await;
    let open = match open {
        Ok(open) => open,
        Err(error) => panic!("failed to open the outgoing workspace: {error:#}"),
    };
    let window = open.window;
    let outgoing_workspace = open.workspace;
    let target_id = WorkspaceId(9001);
    let serialized = strict_restore_fixture(
        target_id,
        path!("/project-b"),
        crate::persistence::model::SerializedPaneGroup::Pane(
            crate::persistence::model::SerializedPane::new(Vec::new(), true, 0),
        ),
    );

    let prepared = cx
        .update(|cx| Workspace::prepare_local_strict(serialized, app_state.clone(), window, cx))
        .await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => panic!("strict target preparation failed: {error:#}"),
    };

    window
        .read_with(cx, |multi_workspace, _cx| {
            assert_eq!(multi_workspace.workspaces().count(), 1);
            assert_eq!(multi_workspace.workspace(), &outgoing_workspace);
            assert!(
                multi_workspace
                    .workspaces()
                    .all(|workspace| workspace != &prepared)
            );
        })
        .expect("the outgoing window closed during target preparation");
    prepared.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.database_id(), Some(target_id));
        assert_eq!(
            PathList::new(&workspace.root_paths(cx)),
            PathList::new(&[path!("/project-b")])
        );
        assert_eq!(workspace.panes().len(), 1);
    });
    let db = cx.update(|cx| WorkspaceDb::global(cx));
    assert!(
        db.workspace_for_id(target_id).is_none(),
        "detached preparation must not serialize or create history"
    );
}

#[gpui::test]
async fn workspace_configuration_strict_restore_failure_changes_nothing(cx: &mut TestAppContext) {
    init_test(cx);
    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project-a"), json!({})).await;
    fs.insert_tree(path!("/project-b"), json!({})).await;

    let open = cx
        .update(|cx| {
            Workspace::new_local(
                vec![PathBuf::from(path!("/project-a"))],
                app_state.clone(),
                None,
                None,
                None,
                OpenMode::Activate,
                cx,
            )
        })
        .await;
    let open = match open {
        Ok(open) => open,
        Err(error) => panic!("failed to open the outgoing workspace: {error:#}"),
    };
    let window = open.window;
    let outgoing_workspace = open.workspace;
    let target_id = WorkspaceId(9002);
    let serialized = strict_restore_fixture(
        target_id,
        path!("/project-b"),
        crate::persistence::model::SerializedPaneGroup::Pane(
            crate::persistence::model::SerializedPane::new(
                vec![crate::persistence::model::SerializedItem::new(
                    "Missing Strict Restore Descriptor",
                    1,
                    true,
                    false,
                )],
                true,
                0,
            ),
        ),
    );

    let result = cx
        .update(|cx| Workspace::prepare_local_strict(serialized, app_state, window, cx))
        .await;
    let error = match result {
        Ok(_) => panic!("strict restoration unexpectedly ignored a missing item descriptor"),
        Err(error) => error,
    };
    assert!(format!("{error:#}").contains("cannot deserialize"));
    window
        .read_with(cx, |multi_workspace, _cx| {
            assert_eq!(multi_workspace.workspaces().count(), 1);
            assert_eq!(multi_workspace.workspace(), &outgoing_workspace);
        })
        .expect("the outgoing window closed after failed target preparation");
    let db = cx.update(|cx| WorkspaceDb::global(cx));
    assert!(db.workspace_for_id(target_id).is_none());
}

#[gpui::test]
async fn test_switch_workspace_configuration_restores_exact_state(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    for path in ["/project-a", "/project-b", "/project-c", "/project-d"] {
        fs.insert_tree(path, json!({})).await;
    }
    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/project-b".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project_a, project_b], cx);
    let outgoing_ids = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace
            .ordered_workspaces(cx)
            .into_iter()
            .map(|workspace| {
                workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
                workspace.read(cx).database_id().expect("workspace id")
            })
            .collect::<Vec<_>>()
    });
    let outgoing_active = multi_workspace
        .read_with(cx, |multi_workspace, cx| {
            multi_workspace.workspace().read(cx).database_id()
        })
        .expect("outgoing active workspace id");
    let session_id = multi_workspace
        .read_with(cx, |multi_workspace, cx| {
            multi_workspace.workspace().read(cx).session_id()
        })
        .expect("outgoing workspace session id");
    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Outgoing".to_string(), cx)
    });
    let outgoing_configuration_id = match save.await {
        Ok(id) => id,
        Err(error) => panic!("failed to save outgoing configuration: {error:#}"),
    };

    let target_c = WorkspaceId(9101);
    let target_d = WorkspaceId(9102);
    let db = cx.update(|_window, cx| WorkspaceDb::global(cx));
    for (id, path) in [(target_c, "/project-c"), (target_d, "/project-d")] {
        if let Err(error) = db
            .save_workspace_checked(strict_restore_fixture(
                id,
                path,
                crate::persistence::model::SerializedPaneGroup::Pane(
                    crate::persistence::model::SerializedPane::new(Vec::new(), true, 0),
                ),
            ))
            .await
        {
            panic!("failed to seed target workspace: {error:#}");
        }
    }
    let create_target = cx.update(|_window, cx| {
        WorkspaceConfigurationStore::mutate_global(
            crate::persistence::WorkspaceConfigurationMutation::Create {
                name: "Target".to_string(),
                members: vec![
                    crate::persistence::model::WorkspaceConfigurationMember {
                        workspace_id: target_d,
                        identity_paths: vec![PathBuf::from("/project-d")],
                    },
                    crate::persistence::model::WorkspaceConfigurationMember {
                        workspace_id: target_c,
                        identity_paths: vec![PathBuf::from("/project-c")],
                    },
                ],
                active_member: Some(target_c),
            },
            cx,
        )
    });
    let target_configuration_id = match create_target.await {
        Ok(commit) => commit.id,
        Err(error) => panic!("failed to create target configuration: {error:#}"),
    };
    let window_id =
        multi_workspace.read_with(cx, |multi_workspace, _cx| multi_workspace.test_window_id());

    let switch = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.switch_workspace_configuration(target_configuration_id, window, cx)
    });
    if let Err(error) = switch.await {
        panic!("workspace configuration switch failed: {error:#}");
    }
    let removal_tasks = multi_workspace.update(cx, |multi_workspace, _cx| {
        multi_workspace.take_pending_removal_tasks()
    });
    futures::future::join_all(removal_tasks).await;

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(multi_workspace.test_window_id(), window_id);
        assert_eq!(
            multi_workspace
                .ordered_workspaces(cx)
                .iter()
                .map(|workspace| workspace.read(cx).database_id().expect("target id"))
                .collect::<Vec<_>>(),
            vec![target_d, target_c]
        );
        assert_eq!(
            multi_workspace.workspace().read(cx).database_id(),
            Some(target_c)
        );
        assert_eq!(
            multi_workspace.active_configuration_id(),
            Some(target_configuration_id)
        );
    });
    let outgoing_configuration = workspace_configuration(outgoing_configuration_id, cx)
        .expect("outgoing configuration disappeared");
    assert_eq!(
        outgoing_configuration
            .members
            .iter()
            .map(|member| member.workspace_id)
            .collect::<Vec<_>>(),
        outgoing_ids
    );
    assert_eq!(outgoing_configuration.active_member, Some(outgoing_active));
    let restored_session = db
        .last_session_workspace_locations(&session_id, None, fs.as_ref())
        .await
        .expect("failed to query the outgoing session after switching");
    assert!(
        restored_session
            .iter()
            .all(|workspace| !outgoing_ids.contains(&workspace.workspace_id)),
        "outgoing workspaces must not reappear on ordinary relaunch"
    );
}

#[gpui::test]
async fn test_switch_workspace_configuration_rolls_back(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    for path in ["/project-a", "/project-b", "/project-c"] {
        fs.insert_tree(path, json!({})).await;
    }
    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project_a, project_b], cx);
    multi_workspace.update(cx, |multi_workspace, cx| {
        for workspace in multi_workspace.workspaces() {
            workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
        }
    });
    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Outgoing".to_string(), cx)
    });
    let outgoing_configuration_id = match save.await {
        Ok(id) => id,
        Err(error) => panic!("failed to save outgoing configuration: {error:#}"),
    };
    let before = multi_workspace.read_with(cx, |multi_workspace, cx| {
        (
            multi_workspace
                .ordered_workspaces(cx)
                .iter()
                .map(Entity::entity_id)
                .collect::<Vec<_>>(),
            multi_workspace.workspace().entity_id(),
        )
    });

    let missing_id = WorkspaceId(9201);
    let valid_id = WorkspaceId(9202);
    let db = cx.update(|_window, cx| WorkspaceDb::global(cx));
    for (id, path) in [(missing_id, "/missing-project"), (valid_id, "/project-c")] {
        if let Err(error) = db
            .save_workspace_checked(strict_restore_fixture(
                id,
                path,
                crate::persistence::model::SerializedPaneGroup::Pane(
                    crate::persistence::model::SerializedPane::new(Vec::new(), true, 0),
                ),
            ))
            .await
        {
            panic!("failed to seed rollback target: {error:#}");
        }
    }
    let create_configuration =
        |name: &str, workspace_id, path: &str, cx: &mut VisualTestContext| {
            cx.update(|_window, cx| {
                WorkspaceConfigurationStore::mutate_global(
                    crate::persistence::WorkspaceConfigurationMutation::Create {
                        name: name.to_string(),
                        members: vec![crate::persistence::model::WorkspaceConfigurationMember {
                            workspace_id,
                            identity_paths: vec![PathBuf::from(path)],
                        }],
                        active_member: Some(workspace_id),
                    },
                    cx,
                )
            })
        };
    let missing_configuration_id =
        create_configuration("Missing", missing_id, "/missing-project", cx)
            .await
            .expect("missing target configuration fixture")
            .id;
    let valid_configuration_id = create_configuration("Valid", valid_id, "/project-c", cx)
        .await
        .expect("valid target configuration fixture")
        .id;

    let missing_switch = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.switch_workspace_configuration(missing_configuration_id, window, cx)
    });
    assert!(missing_switch.await.is_err());
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace
                .ordered_workspaces(cx)
                .iter()
                .map(Entity::entity_id)
                .collect::<Vec<_>>(),
            before.0
        );
        assert_eq!(multi_workspace.workspace().entity_id(), before.1);
        assert_eq!(
            multi_workspace.active_configuration_id(),
            Some(outgoing_configuration_id)
        );
    });

    cx.update(|_window, cx| WorkspaceConfigurationStore::set_write_failure_for_tests(true, cx));
    let failed_commit = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.switch_workspace_configuration(valid_configuration_id, window, cx)
    });
    let failed_commit_error = match failed_commit.await {
        Ok(()) => panic!("forced configuration write failure unexpectedly switched"),
        Err(error) => error,
    };
    assert!(
        format!("{failed_commit_error:#}").contains("forced workspace configuration write failure"),
        "unexpected switch failure: {failed_commit_error:#}"
    );
    cx.update(|_window, cx| WorkspaceConfigurationStore::set_write_failure_for_tests(false, cx));
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace
                .ordered_workspaces(cx)
                .iter()
                .map(Entity::entity_id)
                .collect::<Vec<_>>(),
            before.0
        );
        assert_eq!(multi_workspace.workspace().entity_id(), before.1);
        assert_eq!(
            multi_workspace.active_configuration_id(),
            Some(outgoing_configuration_id)
        );
        assert!(multi_workspace.configuration_checkpoint_error().is_some());
    });
}

#[gpui::test]
async fn workspace_configuration_concurrency_restarts_after_outgoing_stage_drift(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    for path in ["/project-a", "/project-b", "/project-target"] {
        fs.insert_tree(path, json!({})).await;
    }
    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project_a], cx);
    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace
            .workspace()
            .update(cx, |workspace, _cx| workspace.set_random_database_id());
    });
    let outgoing_configuration_id = multi_workspace
        .update(cx, |multi_workspace, cx| {
            multi_workspace.save_configuration_as("Outgoing".to_string(), cx)
        })
        .await
        .expect("failed to save outgoing configuration");
    let target_workspace_id = WorkspaceId(9401);
    let target_configuration_id = seed_workspace_configuration(
        "Target",
        &[(target_workspace_id, "/project-target")],
        target_workspace_id,
        cx,
    )
    .await;
    let (target_staged, resume_switch) = multi_workspace.update(cx, |multi_workspace, _cx| {
        multi_workspace.pause_configuration_switch_for_test(
            WorkspaceConfigurationSwitchTestStage::TargetStaged,
        )
    });

    let switch = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.switch_workspace_configuration(target_configuration_id, window, cx)
    });
    target_staged
        .await
        .expect("switch ended before the staged-target barrier");
    let added_workspace_id = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        let workspace = multi_workspace.test_add_workspace(project_b, window, cx);
        workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
        workspace
            .read(cx)
            .database_id()
            .expect("added workspace id")
    });
    cx.run_until_parked();
    resume_switch
        .send(())
        .expect("switch dropped the staged-target barrier");
    switch
        .await
        .expect("switch did not restart after outgoing stage drift");

    let outgoing_configuration = workspace_configuration(outgoing_configuration_id, cx)
        .expect("outgoing configuration disappeared");
    assert!(
        outgoing_configuration
            .members
            .iter()
            .any(|member| member.workspace_id == added_workspace_id),
        "the restarted outgoing snapshot must include the workspace added during staging"
    );
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.workspace().read(cx).database_id(),
            Some(target_workspace_id)
        );
        assert_eq!(
            multi_workspace.active_configuration_id(),
            Some(target_configuration_id)
        );
    });
}

#[gpui::test]
async fn workspace_configuration_concurrency_ignores_unrelated_store_change_before_commit(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    for path in ["/project-a", "/project-target"] {
        fs.insert_tree(path, json!({})).await;
    }
    let project = Project::test(fs, ["/project-a".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project], cx);
    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace
            .workspace()
            .update(cx, |workspace, _cx| workspace.set_random_database_id());
    });
    let outgoing_configuration_id = multi_workspace
        .update(cx, |multi_workspace, cx| {
            multi_workspace.save_configuration_as("Outgoing".to_string(), cx)
        })
        .await
        .expect("failed to save outgoing configuration");
    let before_workspace = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });
    let target_workspace_id = WorkspaceId(9402);
    let target_configuration_id = seed_workspace_configuration(
        "Target",
        &[(target_workspace_id, "/project-target")],
        target_workspace_id,
        cx,
    )
    .await;
    let (before_commit, resume_switch) = multi_workspace.update(cx, |multi_workspace, _cx| {
        multi_workspace.pause_configuration_switch_for_test(
            WorkspaceConfigurationSwitchTestStage::BeforeCommit,
        )
    });

    let switch = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.switch_workspace_configuration(target_configuration_id, window, cx)
    });
    before_commit
        .await
        .expect("switch ended before the final-commit barrier");
    let concurrent_mutation = cx.update(|_window, cx| {
        WorkspaceConfigurationStore::mutate_global(
            crate::persistence::WorkspaceConfigurationMutation::Create {
                name: "Concurrent".to_string(),
                members: Vec::new(),
                active_member: None,
            },
            cx,
        )
    });
    concurrent_mutation
        .await
        .expect("failed to mutate the configuration store at the barrier");
    resume_switch
        .send(())
        .expect("switch dropped the final-commit barrier");
    switch
        .await
        .expect("an unrelated configuration mutation must not abort the target switch");
    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_ne!(multi_workspace.workspace(), &before_workspace);
        assert_eq!(
            multi_workspace.active_configuration_id(),
            Some(target_configuration_id)
        );
        assert!(!multi_workspace.configuration_switch_in_progress());
    });
    let configurations = cx.update(|_window, cx| {
        WorkspaceConfigurationStore::global(cx)
            .configurations()
            .to_vec()
    });
    assert!(
        configurations
            .iter()
            .any(|configuration| configuration.name == "Concurrent")
    );
    assert!(
        configurations
            .iter()
            .any(|configuration| configuration.id == outgoing_configuration_id)
    );
}

#[gpui::test]
async fn workspace_configuration_final_gate_blocks_workspace_keyboard_actions(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    for path in ["/project-a", "/project-b", "/project-target"] {
        fs.insert_tree(path, json!({})).await;
    }
    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project_a, project_b], cx);
    multi_workspace.update(cx, |multi_workspace, cx| {
        for workspace in multi_workspace.workspaces() {
            workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
        }
    });
    multi_workspace
        .update(cx, |multi_workspace, cx| {
            multi_workspace.save_configuration_as("Outgoing".to_string(), cx)
        })
        .await
        .expect("failed to save outgoing configuration");
    let target_workspace_id = WorkspaceId(9403);
    let target_configuration_id = seed_workspace_configuration(
        "Target",
        &[(target_workspace_id, "/project-target")],
        target_workspace_id,
        cx,
    )
    .await;
    let outgoing_active = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().entity_id()
    });
    let (before_commit, resume_switch) = multi_workspace.update(cx, |multi_workspace, _cx| {
        multi_workspace.pause_configuration_switch_for_test(
            WorkspaceConfigurationSwitchTestStage::BeforeCommit,
        )
    });

    let switch = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.switch_workspace_configuration(target_configuration_id, window, cx)
    });
    before_commit
        .await
        .expect("switch ended before the final input gate");

    cx.dispatch_action(NextProject);
    cx.dispatch_action(PreviousProject);
    cx.dispatch_action(SaveWorkspaceConfigurationAs);
    cx.dispatch_action(SwitchWorkspaceConfiguration);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspace().entity_id(), outgoing_active);
        assert!(!multi_workspace.test_workspace_configuration_menu_is_deployed());
    });

    resume_switch
        .send(())
        .expect("switch dropped the final-commit barrier");
    switch
        .await
        .expect("switch failed after releasing its gate");
}

#[gpui::test]
async fn workspace_configuration_concurrency_aborts_when_foreign_window_acquires_target(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    for path in ["/project-a", "/project-foreign", "/project-target"] {
        fs.insert_tree(path, json!({})).await;
    }
    let source_project = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let foreign_project = Project::test(fs, ["/project-foreign".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;
    let foreign_multi_workspace = {
        let (foreign_multi_workspace, _foreign_cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(foreign_project, window, cx));
        foreign_multi_workspace
    };
    let (multi_workspace, cx) = setup_multi_workspace(&[source_project], cx);
    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace
            .workspace()
            .update(cx, |workspace, _cx| workspace.set_random_database_id());
    });
    let outgoing_configuration_id = multi_workspace
        .update(cx, |multi_workspace, cx| {
            multi_workspace.save_configuration_as("Outgoing".to_string(), cx)
        })
        .await
        .expect("failed to save outgoing configuration");
    let before_workspace = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });
    let target_workspace_id = WorkspaceId(9403);
    let target_configuration_id = seed_workspace_configuration(
        "Target",
        &[(target_workspace_id, "/project-target")],
        target_workspace_id,
        cx,
    )
    .await;
    let (before_commit, resume_switch) = multi_workspace.update(cx, |multi_workspace, _cx| {
        multi_workspace.pause_configuration_switch_for_test(
            WorkspaceConfigurationSwitchTestStage::BeforeCommit,
        )
    });

    let switch = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.switch_workspace_configuration(target_configuration_id, window, cx)
    });
    before_commit
        .await
        .expect("switch ended before the final-commit barrier");
    foreign_multi_workspace.update(cx, |foreign_multi_workspace, cx| {
        foreign_multi_workspace
            .workspace()
            .update(cx, |workspace, _cx| {
                workspace.set_database_id(target_workspace_id)
            });
    });
    resume_switch
        .send(())
        .expect("switch dropped the final-commit barrier");
    let error = switch
        .await
        .expect_err("switch stole a target acquired by another window");
    assert!(format!("{error:#}").contains("open in another window"));
    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspace(), &before_workspace);
        assert_eq!(
            multi_workspace.active_configuration_id(),
            Some(outgoing_configuration_id)
        );
        assert!(!multi_workspace.configuration_switch_in_progress());
    });
}

#[gpui::test]
async fn workspace_configuration_ui_prompts_before_leaving_meaningful_unnamed_set(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    for path in ["/project-a", "/project-c"] {
        fs.insert_tree(path, json!({})).await;
    }
    let project = Project::test(fs, ["/project-a".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;
    let (multi_workspace, cx) = setup_multi_workspace(&[project], cx);
    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace
            .workspace()
            .update(cx, |workspace, _| workspace.set_random_database_id());
    });
    let outgoing_workspace =
        multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());

    let target_id = WorkspaceId(9301);
    let db = cx.update(|_window, cx| WorkspaceDb::global(cx));
    db.save_workspace_checked(strict_restore_fixture(
        target_id,
        "/project-c",
        crate::persistence::model::SerializedPaneGroup::Pane(
            crate::persistence::model::SerializedPane::new(Vec::new(), true, 0),
        ),
    ))
    .await
    .expect("failed to seed the unnamed-switch target workspace");
    let target_configuration_id = cx
        .update(|_window, cx| {
            WorkspaceConfigurationStore::mutate_global(
                crate::persistence::WorkspaceConfigurationMutation::Create {
                    name: "Target".to_string(),
                    members: vec![crate::persistence::model::WorkspaceConfigurationMember {
                        workspace_id: target_id,
                        identity_paths: vec![PathBuf::from("/project-c")],
                    }],
                    active_member: Some(target_id),
                },
                cx,
            )
        })
        .await
        .expect("failed to seed the unnamed-switch target configuration")
        .id;

    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.switch_workspace_configuration_from_ui(
            target_configuration_id,
            "Target".to_string(),
            window,
            cx,
        );
    });
    assert!(cx.has_pending_prompt());
    let (message, detail) = cx.pending_prompt().expect("unnamed switch prompt");
    assert_eq!(message, "Save Current Workspace Configuration?");
    assert!(detail.contains("Target"));

    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, _| {
        assert_eq!(multi_workspace.workspace(), &outgoing_workspace);
        assert_eq!(multi_workspace.active_configuration_id(), None);
        assert!(!multi_workspace.configuration_switch_in_progress());
    });
}

#[gpui::test]
async fn test_sidebar_disabled_when_disable_ai_is_enabled(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));

    multi_workspace.read_with(cx, |mw, cx| {
        assert!(mw.retention_enabled(cx));
        assert!(mw.sidebar_ui_enabled(cx));
    });

    multi_workspace.update_in(cx, |mw, _window, cx| {
        mw.open_sidebar(cx);
        assert!(mw.sidebar_open());
    });

    cx.update(|_window, cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |mw, cx| {
        assert!(
            !mw.sidebar_open(),
            "Sidebar should be closed when disable_ai is true"
        );
        assert!(
            mw.retention_enabled(cx),
            "Workspace retention should stay enabled when disable_ai is true"
        );
        assert!(
            !mw.sidebar_ui_enabled(cx),
            "Sidebar UI should be disabled when disable_ai is true"
        );
    });

    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.toggle_sidebar(window, cx);
    });
    multi_workspace.read_with(cx, |mw, _cx| {
        assert!(
            !mw.sidebar_open(),
            "Sidebar should remain closed when toggled with disable_ai true"
        );
    });

    cx.update(|_window, cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: false }, cx);
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |mw, cx| {
        assert!(
            mw.retention_enabled(cx),
            "Workspace retention should remain enabled after re-enabling AI"
        );
        assert!(
            mw.sidebar_ui_enabled(cx),
            "Sidebar UI should be enabled after re-enabling AI"
        );
        assert!(
            !mw.sidebar_open(),
            "Sidebar should still be closed after re-enabling AI (not auto-opened)"
        );
    });

    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.toggle_sidebar(window, cx);
    });
    multi_workspace.read_with(cx, |mw, _cx| {
        assert!(
            mw.sidebar_open(),
            "Sidebar should open when toggled after re-enabling AI"
        );
    });
}

#[gpui::test]
async fn test_multi_workspace_retains_when_agent_is_disabled(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));

    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(multi_workspace.retention_enabled(cx));
        assert!(multi_workspace.sidebar_ui_enabled(cx));
        assert_eq!(multi_workspace.workspaces().count(), 2);
    });
    multi_workspace.update_in(cx, |multi_workspace, _window, cx| {
        multi_workspace.open_sidebar(cx);
        assert!(multi_workspace.sidebar_open());
    });

    cx.update(|_window, cx| {
        let mut settings = AgentSettings::get_global(cx).clone();
        settings.enabled = false;
        AgentSettings::override_global(settings, cx);
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(multi_workspace.retention_enabled(cx));
        assert!(!multi_workspace.sidebar_ui_enabled(cx));
        assert!(!multi_workspace.sidebar_open());
        assert_eq!(multi_workspace.workspaces().count(), 2);
        assert_eq!(multi_workspace.project_group_keys().len(), 2);
    });
}

#[gpui::test]
async fn test_next_previous_project_cycle_workspace_tabs(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let workspace_a = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });

    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    cx.run_until_parked();

    let workspace_b = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspaces().count(), 2);
        assert_ne!(multi_workspace.workspace(), &workspace_a);
        multi_workspace.workspace().clone()
    });

    cx.dispatch_action(NextProject);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(multi_workspace.sidebar_ui_enabled(cx));
        assert_eq!(multi_workspace.workspace(), &workspace_a);
    });

    cx.dispatch_action(PreviousProject);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspace(), &workspace_b);
    });

    cx.update(|_window, cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    cx.dispatch_action(NextProject);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(!multi_workspace.sidebar_ui_enabled(cx));
        assert_eq!(multi_workspace.workspace(), &workspace_a);
    });

    cx.dispatch_action(PreviousProject);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspace(), &workspace_b);
    });
}

#[gpui::test]
async fn test_next_project_cycles_visible_workspace_tab_order(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_c", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/root_b".as_ref()], cx).await;
    let project_c = Project::test(fs, ["/root_c".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let workspace_a = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
        multi_workspace.test_add_workspace(project_c, window, cx);
    });
    cx.run_until_parked();

    let (workspace_b, workspace_c, key_c) = multi_workspace.read_with(cx, |multi_workspace, cx| {
        let ordered_workspaces = multi_workspace.ordered_workspaces(cx);
        assert_eq!(ordered_workspaces.len(), 3);
        (
            ordered_workspaces[1].clone(),
            ordered_workspaces[0].clone(),
            multi_workspace.project_group_key_for_workspace(&ordered_workspaces[0], cx),
        )
    });

    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.activate(workspace_c.clone(), None, window, cx);
        assert!(!multi_workspace.move_project_group_to_index(&key_c, 0, cx));
    });
    cx.run_until_parked();

    cx.dispatch_action(NextProject);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspace(), &workspace_b);
    });

    cx.dispatch_action(PreviousProject);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspace(), &workspace_c);
        assert_ne!(multi_workspace.workspace(), &workspace_a);
    });
}

#[gpui::test]
async fn test_click_workspace_tab_activates_workspace(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let workspace_a = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });

    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspaces().count(), 2);
        assert_ne!(multi_workspace.workspace(), &workspace_a);
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    let tab_bounds = cx
        .debug_bounds("WORKSPACE-TAB-1")
        .expect("inactive workspace tab should render with debug bounds");

    cx.simulate_click(tab_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspace(), &workspace_a);
    });
}

#[gpui::test]
async fn test_workspace_tabs_start_below_macos_traffic_lights(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    cx.run_until_parked();

    let expected_top = cx.update(|window, _cx| platform_title_bar_height(window));
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );

    let tab_bounds = cx
        .debug_bounds("WORKSPACE-TAB-0")
        .expect("first workspace tab should render with debug bounds");
    if cfg!(target_os = "macos") {
        assert!(
            tab_bounds.origin.y >= expected_top,
            "first workspace tab should start below the macOS titlebar controls"
        );
    }
}

#[gpui::test]
async fn test_workspace_tabs_extend_title_bar_background(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    cx.run_until_parked();

    let expected_height = cx.update(|window, _cx| platform_title_bar_height(window));
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );

    if cfg!(target_os = "macos") {
        let fill_bounds = cx
            .debug_bounds("WORKSPACE-TAB-TITLE-BAR-FILL")
            .expect("workspace tab strip should extend the titlebar background");
        assert_eq!(fill_bounds.origin.y, gpui::px(0.));
        assert_eq!(fill_bounds.size.height, expected_height);

        let heading_bounds = cx
            .debug_bounds("WORKSPACE-TABS-HEADING")
            .expect("workspace tab strip should render a section heading");
        let first_tab_bounds = cx
            .debug_bounds("WORKSPACE-TAB-0")
            .expect("first workspace tab should render with debug bounds");
        assert!(
            heading_bounds.origin.y >= expected_height,
            "workspace tab heading should start below the titlebar fill"
        );
        assert!(
            first_tab_bounds.origin.y >= heading_bounds.bottom(),
            "first workspace tab should start below the Workspaces heading"
        );
    }
}

#[gpui::test]
async fn test_workspace_tabs_mark_workspaces_with_unsaved_changes(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let workspace_a = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    cx.run_until_parked();

    let dirty_item = cx.new(|cx| TestItem::new(cx).with_dirty(true));
    workspace_a.update_in(cx, |workspace, window, cx| {
        workspace.add_item_to_active_pane(Box::new(dirty_item.clone()), None, true, window, cx);
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_unsaved_states(cx),
            vec![("root_b".to_string(), false), ("root_a".to_string(), true)],
        );
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );

    assert!(
        cx.debug_bounds("WORKSPACE-TAB-UNSAVED-1").is_some(),
        "dirty workspace tab should render an unsaved marker",
    );
    assert!(
        cx.debug_bounds("WORKSPACE-TAB-UNSAVED-0").is_none(),
        "clean workspace tab should not render an unsaved marker",
    );
}

#[gpui::test]
async fn test_workspace_tab_order_follows_project_group_reorder(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_c", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/root_b".as_ref()], cx).await;
    let project_c = Project::test(fs, ["/root_c".as_ref()], cx).await;

    let key_b = project_b.read_with(cx, |project, cx| project.project_group_key(cx));

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_c, window, cx);
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_labels(cx),
            vec!["root_c", "root_b", "root_a"],
        );
    });

    multi_workspace.update(cx, |multi_workspace, cx| {
        assert!(multi_workspace.move_project_group_up(&key_b, cx));
    });
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_labels(cx),
            vec!["root_b", "root_c", "root_a"],
        );
    });

    multi_workspace.update(cx, |multi_workspace, cx| {
        assert!(multi_workspace.move_project_group_down(&key_b, cx));
    });
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_labels(cx),
            vec!["root_c", "root_b", "root_a"],
        );
    });
}

#[gpui::test]
async fn test_workspace_tabs_drag_reorder_project_groups(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_c", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/root_b".as_ref()], cx).await;
    let project_c = Project::test(fs, ["/root_c".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_c, window, cx);
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_labels(cx),
            vec!["root_c", "root_b", "root_a"],
        );
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    let source_bounds = cx
        .debug_bounds("WORKSPACE-TAB-1")
        .expect("source workspace tab should render with debug bounds");
    let target_bounds = cx
        .debug_bounds("WORKSPACE-TAB-0")
        .expect("target workspace tab should render with debug bounds");

    cx.simulate_mouse_down(
        source_bounds.center(),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_move(
        target_bounds.center(),
        Some(MouseButton::Left),
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_up(
        target_bounds.center(),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_labels(cx),
            vec!["root_b", "root_c", "root_a"],
        );
    });
}

#[gpui::test]
async fn test_workspace_tabs_drag_reorder_with_multiple_tabs_in_project_group(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/repo/root_a", json!({ "file.txt": "" }))
        .await;
    fs.insert_tree("/repo/root_b", json!({ "file.txt": "" }))
        .await;
    let project_a = Project::test(fs.clone(), ["/repo/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/repo/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.restore_project_groups(
            vec![SerializedProjectGroupState {
                key: ProjectGroupKey::new(None, PathList::new(&[path!("/repo")])),
                expanded: true,
            }],
            cx,
        );
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_labels(cx),
            vec!["root_a", "root_b"],
        );
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    let source_bounds = cx
        .debug_bounds("WORKSPACE-TAB-0")
        .expect("source workspace tab should render with debug bounds");
    let target_bounds = cx
        .debug_bounds("WORKSPACE-TAB-1")
        .expect("target workspace tab should render with debug bounds");

    cx.simulate_mouse_down(
        source_bounds.center(),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_move(
        target_bounds.center(),
        Some(MouseButton::Left),
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_up(
        target_bounds.center(),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_labels(cx),
            vec!["root_b", "root_a"],
        );
        assert_eq!(
            multi_workspace.project_group_keys(),
            vec![ProjectGroupKey::new(None, PathList::new(&[path!("/repo")]))],
        );
    });
}

#[gpui::test]
async fn test_workspace_tabs_drag_reorder_target_index(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_c", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/root_b".as_ref()], cx).await;
    let project_c = Project::test(fs, ["/root_c".as_ref()], cx).await;
    let key_b = project_b.read_with(cx, |project, cx| project.project_group_key(cx));

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_c, window, cx);
    });
    cx.run_until_parked();

    multi_workspace.update(cx, |multi_workspace, cx| {
        assert!(multi_workspace.move_project_group_to_index(&key_b, 0, cx));
    });
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_labels(cx),
            vec!["root_b", "root_c", "root_a"],
        );
    });

    multi_workspace.update(cx, |multi_workspace, cx| {
        assert!(multi_workspace.move_project_group_to_index(&key_b, 2, cx));
    });
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.test_workspace_tab_labels(cx),
            vec!["root_c", "root_a", "root_b"],
        );
    });
}

#[gpui::test]
async fn test_workspace_tabs_render_management_menu(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    cx.run_until_parked();

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );

    assert!(
        cx.debug_bounds("WORKSPACE-TAB-MENU-0").is_some(),
        "workspace tab should expose a management menu",
    );
    assert!(
        cx.debug_bounds("WORKSPACE-TAB-MENU-1").is_some(),
        "each workspace tab should expose a management menu",
    );
}

#[gpui::test]
async fn workspace_configuration_ui_keeps_single_workspace_strip_and_switch_action_reachable(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    reset_workspace_configuration_store(cx).await;
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    let project = Project::test(fs, ["/root_a".as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace
            .workspace()
            .update(cx, |workspace, _| workspace.set_random_database_id());
    });

    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Daily".to_string(), cx)
    });
    let daily_id = save
        .await
        .expect("failed to save the single-workspace UI fixture");
    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Review".to_string(), cx)
    });
    let review_id = save
        .await
        .expect("failed to save the second management UI fixture");
    cx.run_until_parked();

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    let heading_bounds = cx
        .debug_bounds("WORKSPACE-TABS-HEADING")
        .expect("saved configuration should keep the workspace heading visible");
    let trigger_content_bounds = cx
        .debug_bounds("WORKSPACE-CONFIGURATION-TRIGGER-CONTENT")
        .expect("workspace configuration trigger content should render");
    assert!(
        trigger_content_bounds.origin.x <= heading_bounds.origin.x + gpui::px(8.),
        "workspace configuration heading content should be left-aligned"
    );
    assert!(cx.debug_bounds("WORKSPACE-TAB-0").is_some());

    cx.dispatch_action(SwitchWorkspaceConfiguration);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, _| {
        assert!(multi_workspace.test_workspace_configuration_menu_is_deployed());
    });
    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.workspace_configuration_menu_handle.hide(cx);
    });

    cx.dispatch_action(ManageWorkspaceConfigurations);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(
            multi_workspace.test_workspace_configuration_management_modal_is_open(cx),
            "saved configurations should be manageable without switching"
        );
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    assert!(
        cx.debug_bounds("WORKSPACE-CONFIGURATION-MANAGEMENT-MODAL")
            .is_some(),
        "management should render as a dedicated modal"
    );
    cx.dispatch_action(SelectNextWorkspaceConfiguration);
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(
            multi_workspace.test_workspace_configuration_management_modal_is_renaming(cx),
            "Return on the selected configuration should start an inline rename"
        );
        assert_eq!(
            multi_workspace.test_workspace_configuration_management_rename_text(cx),
            Some("Review".to_string()),
            "Down should visibly select the next configuration before Return renames it"
        );
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(
        cx.update(|window, cx| {
            multi_workspace
                .read(cx)
                .test_workspace_configuration_management_focused_control(window, cx)
        }),
        Some("rename-cancel")
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        workspace_configuration(review_id, cx).map(|configuration| configuration.name),
        Some("Review".to_string()),
        "Enter on the rename Cancel button must not save"
    );
    assert!(workspace_configuration(daily_id, cx).is_some());
    assert!(workspace_configuration(review_id, cx).is_some());

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let rename_input = multi_workspace
        .read_with(cx, |multi_workspace, cx| {
            multi_workspace.test_workspace_configuration_management_rename_input(cx)
        })
        .expect("Return should reopen the selected configuration's rename field");
    rename_input.update_in(cx, |input, window, cx| {
        input.set_text("   ", window, cx);
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(
            multi_workspace.test_workspace_configuration_management_modal_is_renaming(cx),
            "an empty name should keep the inline rename open"
        );
    });
    assert_eq!(
        workspace_configuration(review_id, cx).map(|configuration| configuration.name),
        Some("Review".to_string())
    );

    rename_input.update_in(cx, |input, window, cx| {
        input.set_text("Renamed Review", window, cx);
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        workspace_configuration(review_id, cx).map(|configuration| configuration.name),
        Some("Renamed Review".to_string()),
        "Save should commit the inline rename"
    );
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(
            !multi_workspace.test_workspace_configuration_management_modal_is_renaming(cx),
            "a successful rename should return to the management list"
        );
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    cx.simulate_keystrokes("tab enter");
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(
            !multi_workspace.test_workspace_configuration_management_modal_is_open(cx),
            "Enter on Done should dismiss the manager"
        );
    });
}

#[gpui::test]
async fn workspace_configuration_ui_keeps_blocked_store_recovery_reachable(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    reset_workspace_configuration_store(cx).await;
    let newer = format!(
        r#"{{"schema_version":{},"configurations":[]}}"#,
        crate::persistence::model::WORKSPACE_CONFIGURATION_SCHEMA_VERSION + 1
    );
    let kvp = cx.update(|cx| KeyValueStore::global(cx));
    kvp.scoped("workspace_configurations")
        .write("collection".to_string(), newer)
        .await
        .expect("failed to seed the blocked configuration store");
    cx.update(WorkspaceConfigurationStore::reload_for_tests);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    let project = Project::test(fs, ["/root_a".as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    assert!(
        cx.debug_bounds("WORKSPACE-TABS-HEADING").is_some(),
        "a blocked store must keep its recovery menu reachable with one workspace"
    );

    cx.dispatch_action(SwitchWorkspaceConfiguration);
    cx.run_until_parked();
    multi_workspace.read_with(cx, |multi_workspace, _| {
        assert!(multi_workspace.test_workspace_configuration_menu_is_deployed());
    });
}

#[gpui::test]
async fn test_workspace_tabs_hide_when_agent_sidebar_is_open(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    cx.run_until_parked();

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    assert!(
        cx.debug_bounds("WORKSPACE-TAB-0").is_some(),
        "workspace tabs should render while the agent sidebar is closed",
    );

    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.open_sidebar(cx);
    });
    cx.run_until_parked();

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    assert!(
        cx.debug_bounds("WORKSPACE-TAB-0").is_none(),
        "workspace tabs should hide while the agent sidebar's project switcher is visible",
    );
}

#[gpui::test]
async fn test_workspace_tabs_stay_visible_when_ai_is_disabled(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx);
    });
    cx.run_until_parked();

    cx.update(|_window, cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    assert!(
        cx.debug_bounds("WORKSPACE-TAB-0").is_some(),
        "AI-off workspace tabs should stay visible because the agent sidebar is unavailable",
    );
}

#[gpui::test]
async fn test_workspace_tabs_show_overflow_cue_for_hidden_tabs(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    let mut projects = Vec::new();
    for ix in 0..8 {
        let path = format!("/root_{ix}");
        fs.insert_tree(&path, json!({ "file.txt": "" })).await;
        projects.push(Project::test(fs.clone(), [path.as_ref()], cx).await);
    }

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(projects.remove(0), window, cx));
    for project in projects {
        multi_workspace.update_in(cx, |multi_workspace, window, cx| {
            multi_workspace.test_add_workspace(project, window, cx);
        });
    }
    cx.run_until_parked();
    cx.simulate_resize(gpui::size(gpui::px(800.), gpui::px(160.)));

    for _ in 0..2 {
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(800.), gpui::px(160.)),
            |_, _| {
                div()
                    .w(gpui::px(800.))
                    .h(gpui::px(160.))
                    .child(multi_workspace.clone())
            },
        );
    }

    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert!(
            multi_workspace.workspace_tabs_scroll_handle.max_offset().y > gpui::px(2.),
            "crowded workspace tab strip should be vertically scrollable",
        );
    });
    assert!(
        cx.debug_bounds("workspace-tab-overflow-above").is_some()
            || cx.debug_bounds("workspace-tab-overflow-below").is_some(),
        "crowded workspace tab strip should show where hidden tabs continue",
    );
}

#[gpui::test]
async fn test_workspace_tabs_do_not_snap_back_after_manual_scroll(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    let mut projects = Vec::new();
    for ix in 0..8 {
        let path = format!("/root_{ix}");
        fs.insert_tree(&path, json!({ "file.txt": "" })).await;
        projects.push(Project::test(fs.clone(), [path.as_ref()], cx).await);
    }

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(projects.remove(0), window, cx));
    for project in projects {
        multi_workspace.update_in(cx, |multi_workspace, window, cx| {
            multi_workspace.test_add_workspace(project, window, cx);
        });
    }
    cx.run_until_parked();
    cx.simulate_resize(gpui::size(gpui::px(800.), gpui::px(160.)));

    for _ in 0..2 {
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(800.), gpui::px(160.)),
            |_, _| {
                div()
                    .w(gpui::px(800.))
                    .h(gpui::px(160.))
                    .child(multi_workspace.clone())
            },
        );
    }

    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        let max_offset = multi_workspace.workspace_tabs_scroll_handle.max_offset().y;
        assert!(
            max_offset > gpui::px(2.),
            "crowded workspace tab strip should be vertically scrollable",
        );
        multi_workspace
            .workspace_tabs_scroll_handle
            .set_offset(gpui::point(gpui::px(0.), -max_offset));
    });
    multi_workspace.update(cx, |_multi_workspace, cx| cx.notify());

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(160.)),
        |_, _| {
            div()
                .w(gpui::px(800.))
                .h(gpui::px(160.))
                .child(multi_workspace.clone())
        },
    );

    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(
            multi_workspace.workspace_tabs_scroll_handle.offset().y,
            -multi_workspace.workspace_tabs_scroll_handle.max_offset().y,
            "repainting without an active-tab/order change should not override manual scroll",
        );
    });
}

#[gpui::test]
async fn test_click_workspace_tab_close_removes_inactive_workspace(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_c", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/root_b".as_ref()], cx).await;
    let project_c = Project::test(fs, ["/root_c".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let workspace_a = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });

    let workspace_b = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx)
    });
    let workspace_c = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_c, window, cx)
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspace(), &workspace_c);
        assert_eq!(multi_workspace.workspaces().count(), 3);
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    let close_bounds = cx
        .debug_bounds("WORKSPACE-TAB-CLOSE-2")
        .expect("inactive workspace tab close button should render with debug bounds");

    cx.simulate_click(close_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.workspace(),
            &workspace_c,
            "closing an inactive workspace tab should keep the active workspace selected"
        );
        let workspaces = multi_workspace.workspaces().cloned().collect::<Vec<_>>();
        assert_eq!(workspaces.len(), 2);
        assert!(!workspaces.contains(&workspace_a));
        assert!(workspaces.contains(&workspace_b));
        assert!(workspaces.contains(&workspace_c));
        multi_workspace
            .assert_project_group_key_integrity(cx)
            .expect("closing an inactive tab should keep project groups consistent");
    });
}

#[gpui::test]
async fn test_click_workspace_tab_close_activates_neighbor(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_c", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/root_b".as_ref()], cx).await;
    let project_c = Project::test(fs, ["/root_c".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let workspace_a = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });

    let workspace_b = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx)
    });
    let workspace_c = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_c, window, cx)
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.workspace(), &workspace_c);
        assert_eq!(multi_workspace.workspaces().count(), 3);
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    let close_bounds = cx
        .debug_bounds("WORKSPACE-TAB-CLOSE-0")
        .expect("active workspace tab close button should render with debug bounds");

    cx.simulate_click(close_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.workspace(),
            &workspace_b,
            "closing the active workspace tab should activate the nearest retained neighbor"
        );
        let workspaces = multi_workspace.workspaces().cloned().collect::<Vec<_>>();
        assert_eq!(workspaces.len(), 2);
        assert!(workspaces.contains(&workspace_a));
        assert!(workspaces.contains(&workspace_b));
        assert!(!workspaces.contains(&workspace_c));
        multi_workspace
            .assert_project_group_key_integrity(cx)
            .expect("closing the active tab should keep project groups consistent");
    });
}

#[gpui::test]
async fn test_project_group_keys_initial(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    let project = Project::test(fs, ["/root_a".as_ref()], cx).await;

    let expected_key = project.read_with(cx, |project, cx| project.project_group_key(cx));

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));

    multi_workspace.update(cx, |mw, cx| {
        mw.open_sidebar(cx);
    });

    multi_workspace.read_with(cx, |mw, _cx| {
        let keys: Vec<ProjectGroupKey> = mw.project_group_keys();
        assert_eq!(keys.len(), 1, "should have exactly one key on creation");
        assert_eq!(keys[0], expected_key);
    });
}

#[gpui::test]
async fn test_project_group_keys_add_workspace(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/root_b".as_ref()], cx).await;

    let key_a = project_a.read_with(cx, |p, cx| p.project_group_key(cx));
    let key_b = project_b.read_with(cx, |p, cx| p.project_group_key(cx));
    assert_ne!(
        key_a, key_b,
        "different roots should produce different keys"
    );

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));

    multi_workspace.update(cx, |mw, cx| {
        mw.open_sidebar(cx);
    });

    multi_workspace.read_with(cx, |mw, _cx| {
        assert_eq!(mw.project_group_keys().len(), 1);
    });

    // Adding a workspace with a different project root adds a new key.
    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(project_b, window, cx);
    });

    multi_workspace.read_with(cx, |mw, _cx| {
        let keys: Vec<ProjectGroupKey> = mw.project_group_keys();
        assert_eq!(
            keys.len(),
            2,
            "should have two keys after adding a second workspace"
        );
        assert_eq!(keys[0], key_b);
        assert_eq!(keys[1], key_a);
    });
}

#[gpui::test]
async fn test_open_new_window_does_not_open_sidebar_on_existing_window(cx: &mut TestAppContext) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;

    let project = Project::test(app_state.fs.clone(), [path!("/project_a").as_ref()], cx).await;

    let window = cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));

    window
        .read_with(cx, |mw, _cx| {
            assert!(!mw.sidebar_open(), "sidebar should start closed",);
        })
        .unwrap();

    cx.update(|cx| {
        open_paths(
            &[PathBuf::from(path!("/project_b"))],
            app_state,
            OpenOptions {
                open_mode: OpenMode::NewWindow,
                ..OpenOptions::default()
            },
            cx,
        )
    })
    .await
    .unwrap();

    window
        .read_with(cx, |mw, _cx| {
            assert!(
                !mw.sidebar_open(),
                "opening a project in a new window must not open the sidebar on the original window",
            );
        })
        .unwrap();
}

#[gpui::test]
async fn test_workspace_tab_project_group_opens_in_new_window(cx: &mut TestAppContext) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;

    let project_a = Project::test(app_state.fs.clone(), [path!("/project_a").as_ref()], cx).await;
    let project_b = Project::test(app_state.fs.clone(), [path!("/project_b").as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let workspace_b = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx)
    });
    cx.run_until_parked();

    let key_b = workspace_b.read_with(cx, |workspace, cx| workspace.project_group_key(cx));
    let source_window = cx.read(|cx| {
        cx.active_window()
            .expect("source window should be active before opening a new window")
    });
    assert_eq!(cx.read(|cx| cx.windows().len()), 1);

    let open_in_new_window = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.open_project_group_in_new_window(&key_b, window, cx)
    });
    open_in_new_window
        .await
        .expect("project group should open in a new window");
    cx.run_until_parked();

    assert_eq!(cx.read(|cx| cx.windows().len()), 2);
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert!(
            multi_workspace
                .workspaces()
                .all(|workspace| workspace.read(cx).project_group_key(cx) != key_b),
            "source window should no longer retain the moved workspace",
        );
    });

    let new_multi_workspace = cx.read(|cx| {
        cx.active_window()
            .expect("new window should become active after opening the project group")
    });
    assert_ne!(new_multi_workspace.window_id(), source_window.window_id());
    new_multi_workspace
        .read::<MultiWorkspace, _, _>(cx, |multi_workspace, cx| {
            assert!(
                multi_workspace
                    .read(cx)
                    .workspaces()
                    .any(|workspace| workspace.read(cx).project_group_key(cx) == key_b),
                "new window should own the moved workspace project group",
            );
        })
        .expect("new window should contain a MultiWorkspace root");
}

#[gpui::test]
async fn test_open_directory_in_empty_workspace_does_not_open_sidebar(cx: &mut TestAppContext) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project"), json!({ "file.txt": "" }))
        .await;

    let project = Project::test(app_state.fs.clone(), [], cx).await;
    let window = cx.add_window(|window, cx| {
        let mw = MultiWorkspace::test_new(project, window, cx);
        // Simulate a blank project that has an untitled editor tab,
        // so that workspace_windows_for_location finds this window.
        mw.workspace().update(cx, |workspace, cx| {
            workspace.active_pane().update(cx, |pane, cx| {
                let item = cx.new(|cx| item::test::TestItem::new(cx));
                pane.add_item(Box::new(item), false, false, None, window, cx);
            });
        });
        mw
    });

    window
        .read_with(cx, |mw, _cx| {
            assert!(!mw.sidebar_open(), "sidebar should start closed");
        })
        .unwrap();

    // Simulate what open_workspace_for_paths does for an empty workspace:
    // it downgrades OpenMode::NewWindow to Activate and sets requesting_window.
    cx.update(|cx| {
        open_paths(
            &[PathBuf::from(path!("/project"))],
            app_state,
            OpenOptions {
                requesting_window: Some(window),
                open_mode: OpenMode::Activate,
                ..OpenOptions::default()
            },
            cx,
        )
    })
    .await
    .unwrap();

    window
        .read_with(cx, |mw, _cx| {
            assert!(
                !mw.sidebar_open(),
                "opening a directory in a blank project via the file picker must not open the sidebar",
            );
        })
        .unwrap();
}

#[gpui::test]
async fn test_open_directory_adds_workspace_tab_when_ai_is_disabled(cx: &mut TestAppContext) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;

    let project = Project::test(app_state.fs.clone(), [path!("/project_a").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));

    cx.update(|_window, cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    let opened = cx
        .update(|_window, cx| {
            open_paths(
                &[PathBuf::from(path!("/project_b"))],
                app_state.clone(),
                OpenOptions::default(),
                cx,
            )
        })
        .await
        .unwrap();

    multi_workspace.read_with(cx, |mw, cx| {
        assert!(!mw.sidebar_ui_enabled(cx));
        assert!(!mw.sidebar_open());
        assert_eq!(mw.workspaces().count(), 2);
        assert_eq!(mw.project_group_keys().len(), 2);
        assert_eq!(mw.workspace(), &opened.workspace);
    });
}

#[gpui::test]
async fn test_opened_workspace_tabs_serialize_for_restart_when_ai_is_disabled(
    cx: &mut TestAppContext,
) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;

    cx.update(|cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    let first_open = cx
        .update(|cx| {
            Workspace::new_local(
                vec![PathBuf::from(path!("/project_a"))],
                app_state.clone(),
                None,
                None,
                None,
                OpenMode::Activate,
                cx,
            )
        })
        .await
        .expect("first workspace should open");
    cx.run_until_parked();

    let window = first_open.window;
    cx.update(|cx| {
        Workspace::new_local(
            vec![PathBuf::from(path!("/project_b"))],
            app_state.clone(),
            Some(window),
            None,
            None,
            OpenMode::Activate,
            cx,
        )
    })
    .await
    .expect("second workspace should open in the same window");
    cx.run_until_parked();

    let session_id = window
        .read_with(cx, |multi_workspace, cx| {
            multi_workspace.workspace().read(cx).session_id()
        })
        .expect("window should still be alive")
        .expect("active workspace should be session-bound");
    let db = cx.update(|cx| WorkspaceDb::global(cx));
    let session_workspaces = db
        .last_session_workspace_locations(&session_id, None, fs.as_ref())
        .await
        .expect("session workspaces should load");

    let mut serialized_paths = session_workspaces
        .iter()
        .map(|workspace| workspace.paths.paths().to_vec())
        .collect::<Vec<_>>();
    serialized_paths.sort();
    assert_eq!(
        serialized_paths,
        vec![
            vec![PathBuf::from(path!("/project_a"))],
            vec![PathBuf::from(path!("/project_b"))],
        ]
    );

    let mut serialized_multi_workspaces =
        cx.update(|cx| read_serialized_multi_workspaces(session_workspaces, cx));
    assert_eq!(serialized_multi_workspaces.len(), 1);

    let serialized_multi_workspace = serialized_multi_workspaces.remove(0);
    assert_eq!(serialized_multi_workspace.workspaces.len(), 2);

    let restored_window = cx
        .update(|cx| {
            cx.spawn(async move |mut cx| {
                crate::restore_multiworkspace(serialized_multi_workspace, app_state, &mut cx).await
            })
        })
        .await
        .expect("restore should succeed");
    cx.run_until_parked();

    restored_window
        .read_with(cx, |multi_workspace, cx| {
            assert!(!multi_workspace.sidebar_ui_enabled(cx));
            assert_eq!(multi_workspace.workspaces().count(), 2);

            let mut tab_labels = multi_workspace.test_workspace_tab_labels(cx);
            tab_labels.sort();
            assert_eq!(
                tab_labels,
                vec!["project_a".to_string(), "project_b".to_string()]
            );
        })
        .expect("restored window should still be alive");
}

#[gpui::test]
async fn test_app_quit_rebinds_workspace_tabs_for_restart_when_ai_is_disabled(
    cx: &mut TestAppContext,
) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_c"), json!({ "file.txt": "" }))
        .await;

    cx.update(|cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    let first_open = cx
        .update(|cx| {
            Workspace::new_local(
                vec![PathBuf::from(path!("/project_a"))],
                app_state.clone(),
                None,
                None,
                None,
                OpenMode::Activate,
                cx,
            )
        })
        .await
        .expect("first workspace should open");
    cx.run_until_parked();

    let window = first_open.window;
    cx.update(|cx| {
        Workspace::new_local(
            vec![PathBuf::from(path!("/project_b"))],
            app_state,
            Some(window),
            None,
            None,
            OpenMode::Activate,
            cx,
        )
    })
    .await
    .expect("second workspace should open in the same window");
    cx.run_until_parked();

    let (session_id, window_id, workspace_ids, project_b_workspace_id) = window
        .read_with(cx, |multi_workspace, cx| {
            let mut workspace_ids = Vec::new();
            let mut project_b_workspace_id = None;
            for workspace in multi_workspace.workspaces() {
                let workspace = workspace.read(cx);
                let database_id = workspace
                    .database_id()
                    .expect("opened workspace should be serialized");
                if PathList::new(&workspace.root_paths(cx)) == PathList::new(&[path!("/project_b")])
                {
                    project_b_workspace_id = Some(database_id);
                }
                workspace_ids.push(database_id);
            }

            (
                multi_workspace
                    .workspace()
                    .read(cx)
                    .session_id()
                    .expect("active workspace should be session-bound"),
                multi_workspace.test_window_id().as_u64(),
                workspace_ids,
                project_b_workspace_id.expect("project B workspace should exist"),
            )
        })
        .expect("window should still be alive");
    assert_eq!(workspace_ids.len(), 2);

    let db = cx.update(|cx| WorkspaceDb::global(cx));
    for (index, workspace_id) in workspace_ids.iter().enumerate() {
        db.set_session_binding(
            *workspace_id,
            Some(session_id.clone()),
            Some(100 + index as u64),
        )
        .await
        .unwrap();
    }
    let stale_docks = crate::persistence::model::DockStructure {
        left: crate::persistence::model::DockData {
            visible: true,
            active_panel: Some("project-panel".to_string()),
            zoom: true,
        },
        right: crate::persistence::model::DockData {
            visible: true,
            active_panel: Some("outline-panel".to_string()),
            zoom: false,
        },
        bottom: crate::persistence::model::DockData {
            visible: true,
            active_panel: Some("terminal-panel".to_string()),
            zoom: true,
        },
    };
    db.save_workspace(crate::persistence::model::SerializedWorkspace {
        id: project_b_workspace_id,
        paths: PathList::new(&[path!("/project_c")]),
        identity_paths: None,
        location: crate::persistence::model::SerializedWorkspaceLocation::Local,
        center_group: Default::default(),
        window_bounds: Default::default(),
        display: Default::default(),
        docks: stale_docks.clone(),
        bookmarks: Default::default(),
        breakpoints: Default::default(),
        centered_layout: false,
        session_id: Some(session_id.clone()),
        window_id: Some(101),
        user_toolchains: Default::default(),
    })
    .await;

    let stale_session_workspaces = db
        .last_session_workspace_locations(&session_id, None, fs.as_ref())
        .await
        .expect("session workspaces should load before app quit");
    assert_eq!(stale_session_workspaces.len(), 2);
    assert_ne!(
        stale_session_workspaces[0].window_id, stale_session_workspaces[1].window_id,
        "test setup should mimic stale rows split across window ids",
    );

    cx.quit();

    let session_workspaces = db
        .last_session_workspace_locations(&session_id, None, fs.as_ref())
        .await
        .expect("session workspaces should load after app quit");
    assert_eq!(session_workspaces.len(), 2);
    assert!(
        session_workspaces
            .iter()
            .all(|workspace| workspace.window_id == Some(WindowId::from(window_id))),
        "app quit should rebind all retained workspaces to the current multi-workspace window",
    );
    assert!(
        session_workspaces.iter().any(|workspace| {
            workspace.workspace_id == project_b_workspace_id
                && workspace.paths == PathList::new(&[path!("/project_b")])
        }),
        "app quit should flush the current child workspace serialization before shutdown: {session_workspaces:?}",
    );
    let flushed_workspace = db
        .workspace_for_id(project_b_workspace_id)
        .expect("workspace row should still exist");
    assert_eq!(
        flushed_workspace.docks,
        Default::default(),
        "app quit should flush the active workspace's current dock state instead of preserving stale dock state"
    );
    assert_ne!(
        flushed_workspace.docks, stale_docks,
        "stale dock state should not survive the active workspace shutdown flush"
    );
    assert!(
        flushed_workspace.window_bounds.is_some(),
        "app quit should flush the active workspace's current window bounds"
    );
}

#[gpui::test]
async fn test_restore_multiworkspace_state_restores_project_groups_when_ai_is_disabled(
    cx: &mut TestAppContext,
) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;

    let project = Project::test(app_state.fs.clone(), [path!("/project_a").as_ref()], cx).await;
    let window = cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));

    cx.update(|cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    let state = crate::persistence::model::MultiWorkspaceState {
        active_workspace_id: None,
        project_groups: vec![
            crate::persistence::model::SerializedProjectGroup::from_group(
                &ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                true,
            ),
            crate::persistence::model::SerializedProjectGroup::from_group(
                &ProjectGroupKey::new(None, PathList::new(&[path!("/project_b")])),
                true,
            ),
        ],
        sidebar_open: true,
        sidebar_state: None,
        active_configuration_id: None,
    };
    let fs = app_state.fs.clone();
    cx.update(|cx| {
        cx.spawn(async move |mut cx| {
            apply_restored_multiworkspace_state(window, &state, fs, &mut cx).await;
        })
    })
    .await;

    window
        .read_with(cx, |mw, cx| {
            assert!(!mw.sidebar_ui_enabled(cx));
            assert!(!mw.sidebar_open());
            assert_eq!(
                mw.project_group_keys(),
                vec![
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_b")])),
                ]
            );
        })
        .unwrap();
}

#[gpui::test]
async fn test_restore_multiworkspace_state_restores_project_groups_when_agent_is_disabled(
    cx: &mut TestAppContext,
) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;

    let project = Project::test(app_state.fs.clone(), [path!("/project_a").as_ref()], cx).await;
    let window = cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));

    cx.update(|cx| {
        let mut settings = AgentSettings::get_global(cx).clone();
        settings.enabled = false;
        AgentSettings::override_global(settings, cx);
    });
    cx.run_until_parked();

    let state = crate::persistence::model::MultiWorkspaceState {
        active_workspace_id: None,
        project_groups: vec![
            crate::persistence::model::SerializedProjectGroup::from_group(
                &ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                true,
            ),
            crate::persistence::model::SerializedProjectGroup::from_group(
                &ProjectGroupKey::new(None, PathList::new(&[path!("/project_b")])),
                true,
            ),
        ],
        sidebar_open: true,
        sidebar_state: None,
        active_configuration_id: None,
    };
    let fs = app_state.fs.clone();
    cx.update(|cx| {
        cx.spawn(async move |mut cx| {
            apply_restored_multiworkspace_state(window, &state, fs, &mut cx).await;
        })
    })
    .await;

    window
        .read_with(cx, |mw, cx| {
            assert!(!mw.sidebar_ui_enabled(cx));
            assert!(!mw.sidebar_open());
            assert!(mw.retention_enabled(cx));
            assert_eq!(
                mw.project_group_keys(),
                vec![
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_b")])),
                ]
            );
        })
        .unwrap();
}

#[gpui::test]
async fn test_restore_multiworkspace_derives_missing_project_groups_for_restored_workspaces(
    cx: &mut TestAppContext,
) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;

    cx.update(|cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    let serialized_multi_workspace = crate::persistence::model::SerializedMultiWorkspace {
        active_workspace: crate::persistence::model::SessionWorkspace {
            workspace_id: WorkspaceId::from_i64(1),
            location: crate::persistence::model::SerializedWorkspaceLocation::Local,
            paths: PathList::new(&[path!("/project_a")]),
            window_id: Some(WindowId::from(10u64)),
        },
        workspaces: vec![
            crate::persistence::model::SessionWorkspace {
                workspace_id: WorkspaceId::from_i64(1),
                location: crate::persistence::model::SerializedWorkspaceLocation::Local,
                paths: PathList::new(&[path!("/project_a")]),
                window_id: Some(WindowId::from(10u64)),
            },
            crate::persistence::model::SessionWorkspace {
                workspace_id: WorkspaceId::from_i64(2),
                location: crate::persistence::model::SerializedWorkspaceLocation::Local,
                paths: PathList::new(&[path!("/project_b")]),
                window_id: Some(WindowId::from(10u64)),
            },
        ],
        state: crate::persistence::model::MultiWorkspaceState {
            active_workspace_id: Some(WorkspaceId::from_i64(1)),
            project_groups: vec![
                crate::persistence::model::SerializedProjectGroup::from_group(
                    &ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                    false,
                ),
            ],
            sidebar_open: true,
            sidebar_state: None,
            active_configuration_id: None,
        },
    };

    let restored_window = cx
        .update(|cx| {
            cx.spawn(async move |mut cx| {
                crate::restore_multiworkspace(serialized_multi_workspace, app_state, &mut cx).await
            })
        })
        .await
        .expect("restore should succeed");
    cx.run_until_parked();

    restored_window
        .read_with(cx, |multi_workspace, _cx| {
            let keys = multi_workspace.project_group_keys();
            assert!(
                keys.contains(&ProjectGroupKey::new(
                    None,
                    PathList::new(&[path!("/project_a")])
                )),
                "restored KVP project group should remain present: {keys:?}"
            );
            assert!(
                keys.contains(&ProjectGroupKey::new(
                    None,
                    PathList::new(&[path!("/project_b")])
                )),
                "project groups should be derived for retained workspaces missing from stale KVP state: {keys:?}"
            );
            assert_eq!(
                keys.len(),
                2,
                "restore should not duplicate derived project groups: {keys:?}"
            );
        })
        .expect("restored window should still be alive");
}

#[gpui::test]
async fn test_restore_multiworkspace_derives_project_groups_from_empty_state(
    cx: &mut TestAppContext,
) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;

    cx.update(|cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    let serialized_multi_workspace = crate::persistence::model::SerializedMultiWorkspace {
        active_workspace: crate::persistence::model::SessionWorkspace {
            workspace_id: WorkspaceId::from_i64(1),
            location: crate::persistence::model::SerializedWorkspaceLocation::Local,
            paths: PathList::new(&[path!("/project_a")]),
            window_id: Some(WindowId::from(10u64)),
        },
        workspaces: vec![
            crate::persistence::model::SessionWorkspace {
                workspace_id: WorkspaceId::from_i64(1),
                location: crate::persistence::model::SerializedWorkspaceLocation::Local,
                paths: PathList::new(&[path!("/project_a")]),
                window_id: Some(WindowId::from(10u64)),
            },
            crate::persistence::model::SessionWorkspace {
                workspace_id: WorkspaceId::from_i64(2),
                location: crate::persistence::model::SerializedWorkspaceLocation::Local,
                paths: PathList::new(&[path!("/project_b")]),
                window_id: Some(WindowId::from(10u64)),
            },
        ],
        state: crate::persistence::model::MultiWorkspaceState {
            active_workspace_id: Some(WorkspaceId::from_i64(1)),
            project_groups: Vec::new(),
            sidebar_open: true,
            sidebar_state: None,
            active_configuration_id: None,
        },
    };

    let restored_window = cx
        .update(|cx| {
            cx.spawn(async move |mut cx| {
                crate::restore_multiworkspace(serialized_multi_workspace, app_state, &mut cx).await
            })
        })
        .await
        .expect("restore should succeed");
    cx.run_until_parked();

    restored_window
        .read_with(cx, |multi_workspace, _cx| {
            let keys = multi_workspace.project_group_keys();
            assert!(
                keys.contains(&ProjectGroupKey::new(
                    None,
                    PathList::new(&[path!("/project_a")])
                )),
                "project groups should be derived for the active restored workspace: {keys:?}"
            );
            assert!(
                keys.contains(&ProjectGroupKey::new(
                    None,
                    PathList::new(&[path!("/project_b")])
                )),
                "project groups should be derived for inactive restored workspaces when persisted state is empty: {keys:?}"
            );
            assert_eq!(
                keys.len(),
                2,
                "empty persisted project group state should not leave retained workspaces ungrouped: {keys:?}"
            );
        })
        .expect("restored window should still be alive");
}

#[gpui::test]
async fn test_restore_multiworkspace_skips_inactive_remote_workspaces_without_live_connection(
    cx: &mut TestAppContext,
) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;

    cx.update(|cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    let remote_location = crate::persistence::model::SerializedWorkspaceLocation::Remote(
        remote::RemoteConnectionOptions::Mock(remote::MockConnectionOptions { id: 1 }),
    );
    let serialized_multi_workspace = crate::persistence::model::SerializedMultiWorkspace {
        active_workspace: crate::persistence::model::SessionWorkspace {
            workspace_id: WorkspaceId::from_i64(1),
            location: crate::persistence::model::SerializedWorkspaceLocation::Local,
            paths: PathList::new(&[path!("/project_a")]),
            window_id: Some(WindowId::from(10u64)),
        },
        workspaces: vec![
            crate::persistence::model::SessionWorkspace {
                workspace_id: WorkspaceId::from_i64(1),
                location: crate::persistence::model::SerializedWorkspaceLocation::Local,
                paths: PathList::new(&[path!("/project_a")]),
                window_id: Some(WindowId::from(10u64)),
            },
            crate::persistence::model::SessionWorkspace {
                workspace_id: WorkspaceId::from_i64(2),
                location: remote_location,
                paths: PathList::new(&[path!("/remote/project_b")]),
                window_id: Some(WindowId::from(10u64)),
            },
        ],
        state: crate::persistence::model::MultiWorkspaceState {
            active_workspace_id: Some(WorkspaceId::from_i64(1)),
            project_groups: Vec::new(),
            sidebar_open: true,
            sidebar_state: None,
            active_configuration_id: None,
        },
    };

    let restored_window = cx
        .update(|cx| {
            cx.spawn(async move |mut cx| {
                crate::restore_multiworkspace(serialized_multi_workspace, app_state, &mut cx).await
            })
        })
        .await
        .expect("restore should succeed");
    cx.run_until_parked();

    restored_window
        .read_with(cx, |multi_workspace, _cx| {
            assert_eq!(
                multi_workspace.workspaces().count(),
                1,
                "inactive remote workspaces require a live remote connection and should not be restored from persisted rows alone"
            );
        })
        .expect("restored window should still be alive");
}

#[gpui::test]
async fn test_restore_multiworkspace_state_restores_sidebar_when_ai_is_enabled(
    cx: &mut TestAppContext,
) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file.txt": "" }))
        .await;

    let project = Project::test(app_state.fs.clone(), [path!("/project_a").as_ref()], cx).await;
    let window = cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));

    let state = crate::persistence::model::MultiWorkspaceState {
        active_workspace_id: None,
        project_groups: vec![
            crate::persistence::model::SerializedProjectGroup::from_group(
                &ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                true,
            ),
            crate::persistence::model::SerializedProjectGroup::from_group(
                &ProjectGroupKey::new(None, PathList::new(&[path!("/project_b")])),
                true,
            ),
        ],
        sidebar_open: true,
        sidebar_state: None,
        active_configuration_id: None,
    };
    let fs = app_state.fs.clone();
    cx.update(|cx| {
        cx.spawn(async move |mut cx| {
            apply_restored_multiworkspace_state(window, &state, fs, &mut cx).await;
        })
    })
    .await;

    window
        .read_with(cx, |multi_workspace, cx| {
            assert!(multi_workspace.sidebar_ui_enabled(cx));
            assert!(multi_workspace.sidebar_open());
            assert_eq!(
                multi_workspace.project_group_keys(),
                vec![
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_b")])),
                ]
            );
        })
        .unwrap();
}

#[gpui::test]
async fn test_restore_multiworkspace_restores_inactive_workspaces_when_ai_is_disabled(
    cx: &mut TestAppContext,
) {
    restore_inactive_workspaces_with_sidebar_ui_disabled(cx, DisabledSidebarSetting::DisableAi)
        .await;
}

#[gpui::test]
async fn test_restore_multiworkspace_restores_inactive_workspaces_when_agent_is_disabled(
    cx: &mut TestAppContext,
) {
    restore_inactive_workspaces_with_sidebar_ui_disabled(cx, DisabledSidebarSetting::DisableAgent)
        .await;
}

#[derive(Clone, Copy)]
enum DisabledSidebarSetting {
    DisableAi,
    DisableAgent,
}

async fn restore_inactive_workspaces_with_sidebar_ui_disabled(
    cx: &mut TestAppContext,
    setting: DisabledSidebarSetting,
) {
    init_test(cx);

    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file.txt": "" }))
        .await;
    fs.insert_tree(
        path!("/repo"),
        json!({ "dotfiles": { "rb-agents": { "skills": { "SKILL.md": "" } } } }),
    )
    .await;
    let nested_workspace_path = path!("/repo/dotfiles/rb-agents/skills");

    cx.update(|cx| match setting {
        DisabledSidebarSetting::DisableAi => {
            DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
        }
        DisabledSidebarSetting::DisableAgent => {
            let mut settings = AgentSettings::get_global(cx).clone();
            settings.enabled = false;
            AgentSettings::override_global(settings, cx);
        }
    });
    cx.run_until_parked();

    let serialized_multi_workspace = crate::persistence::model::SerializedMultiWorkspace {
        active_workspace: crate::persistence::model::SessionWorkspace {
            workspace_id: WorkspaceId::from_i64(1),
            location: crate::persistence::model::SerializedWorkspaceLocation::Local,
            paths: PathList::new(&[path!("/project_a")]),
            window_id: Some(WindowId::from(10u64)),
        },
        workspaces: vec![
            crate::persistence::model::SessionWorkspace {
                workspace_id: WorkspaceId::from_i64(1),
                location: crate::persistence::model::SerializedWorkspaceLocation::Local,
                paths: PathList::new(&[path!("/project_a")]),
                window_id: Some(WindowId::from(10u64)),
            },
            crate::persistence::model::SessionWorkspace {
                workspace_id: WorkspaceId::from_i64(2),
                location: crate::persistence::model::SerializedWorkspaceLocation::Local,
                paths: PathList::new(&[nested_workspace_path]),
                window_id: Some(WindowId::from(10u64)),
            },
        ],
        state: crate::persistence::model::MultiWorkspaceState {
            active_workspace_id: Some(WorkspaceId::from_i64(1)),
            project_groups: vec![
                crate::persistence::model::SerializedProjectGroup::from_group(
                    &ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                    true,
                ),
                crate::persistence::model::SerializedProjectGroup::from_group(
                    &ProjectGroupKey::new(None, PathList::new(&[path!("/repo")])),
                    true,
                ),
            ],
            sidebar_open: true,
            sidebar_state: None,
            active_configuration_id: None,
        },
    };

    let restored_window = cx
        .update(|cx| {
            cx.spawn(async move |mut cx| {
                crate::restore_multiworkspace(serialized_multi_workspace, app_state, &mut cx).await
            })
        })
        .await;

    cx.run_until_parked();

    let restored_window = restored_window.expect("restore should succeed");

    restored_window
        .read_with(cx, |multi_workspace, cx| {
            assert!(!multi_workspace.sidebar_ui_enabled(cx));
            assert!(!multi_workspace.sidebar_open());
            assert_eq!(multi_workspace.workspaces().count(), 2);
            assert_eq!(
                multi_workspace.project_group_keys(),
                vec![
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                    ProjectGroupKey::new(None, PathList::new(&[path!("/repo")])),
                ]
            );
            let mut workspace_paths = multi_workspace
                .workspaces()
                .map(|workspace| {
                    workspace
                        .read(cx)
                        .root_paths(cx)
                        .into_iter()
                        .map(|path| path.to_path_buf())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            workspace_paths.sort();
            assert_eq!(
                workspace_paths,
                vec![
                    vec![PathBuf::from(path!("/project_a"))],
                    vec![PathBuf::from(nested_workspace_path)],
                ]
            );

            let mut tab_labels = multi_workspace.test_workspace_tab_labels(cx);
            tab_labels.sort();
            assert_eq!(
                tab_labels,
                vec!["project_a".to_string(), "skills".to_string()]
            );

            let repo_group_key = ProjectGroupKey::new(None, PathList::new(&[path!("/repo")]));
            // Upstream's `workspaces_for_project_group` returns a plain `Vec`; a missing
            // group is an empty one, which the length assertion below still catches.
            let repo_group_workspaces =
                multi_workspace.workspaces_for_project_group(&repo_group_key, cx);
            assert_eq!(
                repo_group_workspaces.len(),
                1,
                "restored parent group should own the nested workspace",
            );
            assert_eq!(
                repo_group_workspaces[0]
                    .read(cx)
                    .root_paths(cx)
                    .into_iter()
                    .map(|path| path.to_path_buf())
                    .collect::<Vec<_>>(),
                vec![PathBuf::from(nested_workspace_path)],
            );
            assert_eq!(
                multi_workspace.project_group_key_for_workspace(&repo_group_workspaces[0], cx),
                repo_group_key,
                "workspace-tab actions should use the owning parent project group",
            );
        })
        .unwrap();

    restored_window
        .update(cx, |multi_workspace, window, cx| {
            let repo_group_key = ProjectGroupKey::new(None, PathList::new(&[path!("/repo")]));
            let nested_workspace = multi_workspace
                .workspaces_for_project_group(&repo_group_key, cx)
                .into_iter()
                .next()
                .expect("restored parent group should own the nested workspace");

            multi_workspace.activate(nested_workspace.clone(), None, window, cx);

            assert_eq!(
                multi_workspace.project_group_keys(),
                vec![
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")])),
                    repo_group_key.clone(),
                ],
                "activating the nested workspace should not create a raw child project group",
            );
            assert_eq!(
                multi_workspace
                    .last_active_workspace_for_group(&repo_group_key, cx)
                    .expect("activated nested workspace should be recorded as the parent group's last active workspace")
                    .entity_id(),
                nested_workspace.entity_id(),
            );
        })
        .unwrap();
}

#[gpui::test]
async fn test_project_group_keys_duplicate_not_added(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    // A second project entity pointing at the same path produces the same key.
    let project_a2 = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;

    let key_a = project_a.read_with(cx, |p, cx| p.project_group_key(cx));
    let key_a2 = project_a2.read_with(cx, |p, cx| p.project_group_key(cx));
    assert_eq!(key_a, key_a2, "same root path should produce the same key");

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));

    multi_workspace.update(cx, |mw, cx| {
        mw.open_sidebar(cx);
    });

    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(project_a2, window, cx);
    });

    multi_workspace.read_with(cx, |mw, _cx| {
        let keys: Vec<ProjectGroupKey> = mw.project_group_keys();
        assert_eq!(
            keys.len(),
            1,
            "duplicate key should not be added when a workspace with the same root is inserted"
        );
    });
}

#[gpui::test]
async fn test_adding_worktree_updates_project_group_key(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "other.txt": "" })).await;
    let project = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;

    let initial_key = project.read_with(cx, |p, cx| p.project_group_key(cx));

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

    // Open sidebar to retain the workspace and create the initial group.
    multi_workspace.update(cx, |mw, cx| {
        mw.open_sidebar(cx);
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |mw, _cx| {
        let keys = mw.project_group_keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0], initial_key);
    });

    // Add a second worktree to the project. This triggers WorktreeAdded →
    // handle_workspace_key_change, which should update the group key.
    project
        .update(cx, |project, cx| {
            project.find_or_create_worktree("/root_b", true, cx)
        })
        .await
        .expect("adding worktree should succeed");
    cx.run_until_parked();

    let updated_key = project.read_with(cx, |p, cx| p.project_group_key(cx));
    assert_ne!(
        initial_key, updated_key,
        "adding a worktree should change the project group key"
    );

    multi_workspace.read_with(cx, |mw, _cx| {
        let keys = mw.project_group_keys();
        assert!(
            keys.contains(&updated_key),
            "should contain the updated key; got {keys:?}"
        );
    });
}

#[gpui::test]
async fn test_find_or_create_local_workspace_reuses_active_workspace_when_sidebar_closed(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    let project = Project::test(fs, ["/root_a".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));

    let active_workspace = multi_workspace.read_with(cx, |mw, cx| {
        assert!(
            mw.project_groups(cx).is_empty(),
            "sidebar-closed setup should start with no retained project groups"
        );
        mw.workspace().clone()
    });
    let active_workspace_id = active_workspace.entity_id();

    let workspace = multi_workspace
        .update_in(cx, |mw, window, cx| {
            mw.find_or_create_local_workspace(
                PathList::new(&[PathBuf::from("/root_a")]),
                None,
                None,
                OpenMode::Activate,
                None,
                window,
                cx,
            )
        })
        .await
        .expect("reopening the same local workspace should succeed");

    assert_eq!(
        workspace.entity_id(),
        active_workspace_id,
        "should reuse the current active workspace when the sidebar is closed"
    );

    multi_workspace.read_with(cx, |mw, _cx| {
        assert_eq!(
            mw.workspace().entity_id(),
            active_workspace_id,
            "active workspace should remain unchanged after reopening the same path"
        );
        assert_eq!(
            mw.workspaces().count(),
            1,
            "reusing the active workspace should not create a second open workspace"
        );
    });
}

#[gpui::test]
async fn test_find_or_create_workspace_uses_project_group_key_when_paths_are_missing(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        "/project",
        json!({
            ".git": {},
            "src": {},
        }),
    )
    .await;
    cx.update(|cx| <dyn Fs>::set_global(fs.clone(), cx));
    let project = Project::test(fs.clone(), ["/project".as_ref()], cx).await;
    project
        .update(cx, |project, cx| project.git_scans_complete(cx))
        .await;

    let project_group_key = project.read_with(cx, |project, cx| project.project_group_key(cx));

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));

    let main_workspace = multi_workspace.read_with(cx, |mw, _cx| mw.workspace().clone());
    let main_workspace_id = main_workspace.entity_id();

    let workspace = multi_workspace
        .update_in(cx, |mw, window, cx| {
            mw.find_or_create_workspace(
                PathList::new(&[PathBuf::from("/wt-feature-a")]),
                None,
                Some(project_group_key.clone()),
                |_options, _window, _cx| Task::ready(Ok(None)),
                None,
                OpenMode::Activate,
                None,
                window,
                cx,
            )
        })
        .await
        .expect("opening a missing linked-worktree path should fall back to the project group key workspace");

    assert_eq!(
        workspace.entity_id(),
        main_workspace_id,
        "missing linked-worktree paths should reuse the main worktree workspace from the project group key"
    );

    multi_workspace.read_with(cx, |mw, cx| {
        assert_eq!(
            mw.workspace().entity_id(),
            main_workspace_id,
            "the active workspace should remain the main worktree workspace"
        );
        assert_eq!(
            PathList::new(&mw.workspace().read(cx).root_paths(cx)),
            project_group_key.path_list().clone(),
            "the activated workspace should use the project group key path list rather than the missing linked-worktree path"
        );
        assert_eq!(
            mw.workspaces().count(),
            1,
            "falling back to the project group key should not create a second workspace"
        );
    });
}

#[gpui::test]
async fn test_remove_fallback_via_find_or_create_skips_removed_workspaces(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_a".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));

    let workspace_a = multi_workspace.read_with(cx, |mw, _cx| mw.workspace().clone());
    let workspace_b = multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(project_b, window, cx)
    });
    cx.run_until_parked();

    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.activate(workspace_a.clone(), None, window, cx);
    });

    let removed = multi_workspace
        .update_in(cx, |mw, window, cx| {
            mw.remove(
                vec![workspace_a.clone()],
                RemovalIntent::CloseProject,
                window,
                cx,
            )
        })
        .await
        .expect("removing the active workspace should succeed");
    assert!(removed, "the workspace should have been removed");

    multi_workspace.read_with(cx, |mw, _cx| {
        assert_eq!(
            mw.workspace().entity_id(),
            workspace_b.entity_id(),
            "the non-excluded workspace should become active"
        );
        assert!(
            mw.workspaces()
                .all(|workspace| workspace.entity_id() != workspace_a.entity_id()),
            "the removed workspace should be gone"
        );
    });
}

#[gpui::test]
async fn test_remove_keeping_the_project_does_not_switch_projects(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file.txt": "" })).await;
    cx.update(|cx| <dyn Fs>::set_global(fs.clone(), cx));
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));

    let workspace_a = multi_workspace.read_with(cx, |mw, _cx| mw.workspace().clone());
    let _workspace_b = multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(project_b, window, cx)
    });
    cx.run_until_parked();

    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.activate(workspace_a.clone(), None, window, cx);
    });
    cx.run_until_parked();

    multi_workspace
        .update_in(cx, |mw, window, cx| {
            mw.remove(
                vec![workspace_a.clone()],
                RemovalIntent::KeepProject,
                window,
                cx,
            )
        })
        .await
        .expect("removing the active workspace should succeed");
    cx.run_until_parked();

    multi_workspace.read_with(cx, |mw, cx| {
        assert_eq!(
            PathList::new(&mw.workspace().read(cx).root_paths(cx)),
            PathList::new(&[PathBuf::from("/root_a")]),
            "the replacement workspace should be in the removed workspace's project"
        );
    });
}

#[gpui::test]
async fn test_find_or_create_local_workspace_reuses_active_workspace_after_sidebar_open(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    let project = Project::test(fs, ["/root_a".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));

    multi_workspace.update(cx, |mw, cx| {
        mw.open_sidebar(cx);
    });
    cx.run_until_parked();

    let active_workspace = multi_workspace.read_with(cx, |mw, cx| {
        assert_eq!(
            mw.project_groups(cx).len(),
            1,
            "opening the sidebar should retain the active workspace in a project group"
        );
        mw.workspace().clone()
    });
    let active_workspace_id = active_workspace.entity_id();

    let workspace = multi_workspace
        .update_in(cx, |mw, window, cx| {
            mw.find_or_create_local_workspace(
                PathList::new(&[PathBuf::from("/root_a")]),
                None,
                None,
                OpenMode::Activate,
                None,
                window,
                cx,
            )
        })
        .await
        .expect("reopening the same retained local workspace should succeed");

    assert_eq!(
        workspace.entity_id(),
        active_workspace_id,
        "should reuse the retained active workspace after the sidebar is opened"
    );

    multi_workspace.read_with(cx, |mw, _cx| {
        assert_eq!(
            mw.workspaces().count(),
            1,
            "reopening the same retained workspace should not create another workspace"
        );
    });
}

#[gpui::test]
async fn test_close_workspace_prefers_already_loaded_neighboring_workspace(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file_a.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file_b.txt": "" })).await;
    fs.insert_tree("/root_c", json!({ "file_c.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/root_b".as_ref()], cx).await;
    let project_b_key = project_b.read_with(cx, |project, cx| project.project_group_key(cx));
    let project_c = Project::test(fs, ["/root_c".as_ref()], cx).await;
    let project_c_key = project_c.read_with(cx, |project, cx| project.project_group_key(cx));

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));

    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.open_sidebar(cx);
    });
    cx.run_until_parked();

    let workspace_a = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });
    let workspace_b = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_b, window, cx)
    });

    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.activate(workspace_a.clone(), None, window, cx);
        multi_workspace.test_add_project_group(ProjectGroup {
            key: project_c_key.clone(),
            workspaces: Vec::new(),
            expanded: true,
        });
    });

    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        let keys = multi_workspace.project_group_keys();
        assert_eq!(
            keys.len(),
            3,
            "expected three project groups in the test setup"
        );
        assert_eq!(keys[0], project_b_key);
        assert_eq!(
            keys[1],
            workspace_a.read_with(cx, |workspace, cx| { workspace.project_group_key(cx) })
        );
        assert_eq!(keys[2], project_c_key);
        assert_eq!(
            multi_workspace.workspace().entity_id(),
            workspace_a.entity_id(),
            "workspace A should be active before closing"
        );
    });

    let closed = multi_workspace
        .update_in(cx, |multi_workspace, window, cx| {
            multi_workspace.remove(
                [workspace_a.clone()],
                RemovalIntent::CloseProject,
                window,
                cx,
            )
        })
        .await
        .expect("closing the active workspace should succeed");

    assert!(
        closed,
        "close_workspace should report that it removed a workspace"
    );

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.workspace().entity_id(),
            workspace_b.entity_id(),
            "closing workspace A should activate the already-loaded workspace B instead of opening group C"
        );
        assert_eq!(
            multi_workspace.workspaces().count(),
            1,
            "only workspace B should remain loaded after closing workspace A"
        );
        assert!(
            multi_workspace
                .workspaces_for_project_group(&project_c_key, cx)
                .is_empty(),
            "the unloaded neighboring group C should remain unopened"
        );
    });
}

#[gpui::test]
async fn test_close_workspace_prefers_workspace_in_same_project_group(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    fs.insert_tree("/project-b", json!({})).await;

    let project_a_1 = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_a_2 = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    let key_a = project_a_1.read_with(cx, |project, cx| project.project_group_key(cx));
    let (multi_workspace, cx) = setup_multi_workspace(&[project_a_1, project_a_2, project_b], cx);

    let (workspace_a_1, workspace_a_2) = multi_workspace.read_with(cx, |multi_workspace, cx| {
        let mut workspaces = multi_workspace
            .workspaces_for_project_group(&key_a, cx)
            .into_iter();
        let first = workspaces
            .next()
            .expect("project group A should have a first workspace");
        let second = workspaces
            .next()
            .expect("project group A should have a second workspace");

        assert!(
            workspaces.next().is_none(),
            "project group A should have exactly two workspaces"
        );

        (first, second)
    });

    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.activate(workspace_a_1.clone(), None, window, cx);
    });

    let closed = multi_workspace
        .update_in(cx, |multi_workspace, window, cx| {
            multi_workspace.remove(
                [workspace_a_1.clone()],
                RemovalIntent::CloseProject,
                window,
                cx,
            )
        })
        .await
        .expect("closing the active workspace should succeed");

    assert!(closed, "close_workspace should remove the active workspace");
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.workspace(),
            &workspace_a_2,
            "the second workspace for project group a should be preferred"
        );

        assert_eq!(
            multi_workspace.workspaces_for_project_group(&key_a, cx),
            vec![workspace_a_2],
            "only the fallback workspace should remain in project group A"
        );
    });
}

#[gpui::test]
async fn test_close_workspace_opens_unloaded_local_neighbor(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    fs.insert_tree("/project-b", json!({})).await;
    cx.update(|cx| <dyn Fs>::set_global(fs.clone(), cx));

    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    let key_b = project_b.read_with(cx, |project, cx| project.project_group_key(cx));
    let (multi_workspace, cx) = setup_multi_workspace(&[project_a], cx);
    let workspace_a = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });

    multi_workspace.update(cx, |multi_workspace, _cx| {
        multi_workspace.test_add_project_group(ProjectGroup {
            key: key_b.clone(),
            workspaces: Vec::new(),
            expanded: true,
        });
    });

    let closed = multi_workspace
        .update_in(cx, |multi_workspace, window, cx| {
            multi_workspace.remove(
                [workspace_a.clone()],
                RemovalIntent::CloseProject,
                window,
                cx,
            )
        })
        .await
        .expect("closing the active workspace should succeed");

    assert!(closed, "close_workspace should remove the active workspace");
    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.workspace().read(cx).project_group_key(cx),
            key_b,
            "the unloaded local neighboring group should be opened"
        );
    });
}

#[gpui::test]
async fn test_remove_project_group_opens_unloaded_local_neighbor(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    fs.insert_tree("/project-b", json!({})).await;
    cx.update(|cx| <dyn Fs>::set_global(fs.clone(), cx));

    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    let key_a = project_a.read_with(cx, |project, cx| project.project_group_key(cx));
    let key_b = project_b.read_with(cx, |project, cx| project.project_group_key(cx));
    let (multi_workspace, cx) = setup_multi_workspace(&[project_a], cx);

    multi_workspace.update(cx, |multi_workspace, _cx| {
        multi_workspace.test_add_project_group(ProjectGroup {
            key: key_b.clone(),
            workspaces: Vec::new(),
            expanded: true,
        });
    });

    let removed = multi_workspace
        .update_in(cx, |multi_workspace, window, cx| {
            multi_workspace.remove_project_group(&key_a, window, cx)
        })
        .await
        .expect("removing the active project group should succeed");

    assert!(
        removed,
        "remove_project_group should remove the active group"
    );

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.workspace().read(cx).project_group_key(cx),
            key_b,
            "the unloaded local neighboring group should be opened"
        );
    });
}

#[gpui::test]
async fn test_remove_project_group_replaces_unretained_active_workspace(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;

    let project_a = Project::test(fs, ["/project-a".as_ref()], cx).await;
    let key_a = project_a.read_with(cx, |project, cx| project.project_group_key(cx));
    let remote_key = ProjectGroupKey::new(
        Some(RemoteConnectionOptions::Mock(
            remote::MockConnectionOptions { id: 1 },
        )),
        PathList::new(&[PathBuf::from("/remote/project")]),
    );
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let workspace_a = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });

    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.restore_project_groups(
            vec![
                SerializedProjectGroupState {
                    key: key_a.clone(),
                    expanded: true,
                },
                SerializedProjectGroupState {
                    key: remote_key.clone(),
                    expanded: true,
                },
            ],
            cx,
        );

        assert!(
            !multi_workspace.active_workspace_is_retained(),
            "the active workspace should remain provisional"
        );
        assert_eq!(
            multi_workspace.project_group_keys(),
            vec![key_a.clone(), remote_key.clone()],
            "the remote project group should immediately follow the active local group"
        );
    });

    multi_workspace
        .update_in(cx, |multi_workspace, window, cx| {
            multi_workspace.remove_project_group(&key_a, window, cx)
        })
        .await
        .expect("removing the active project group should succeed");

    multi_workspace.read_with(cx, |multi_workspace, cx| {
        assert_ne!(
            multi_workspace.workspace(),
            &workspace_a,
            "removing the active project group should replace its provisional workspace"
        );
        assert!(
            multi_workspace
                .workspace()
                .read(cx)
                .root_paths(cx)
                .is_empty(),
            "an unloaded remote neighbor should fall back to an empty workspace"
        );
        assert_eq!(
            multi_workspace.project_group_keys(),
            vec![remote_key],
            "only the remote project group should remain"
        );
    });
}

#[gpui::test]
async fn test_switching_projects_with_sidebar_closed_retains_old_active_workspace(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file_a.txt": "" })).await;
    fs.insert_tree("/root_b", json!({ "file_b.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/root_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));

    let workspace_a = multi_workspace.read_with(cx, |mw, cx| {
        assert!(
            mw.project_groups(cx).is_empty(),
            "sidebar-closed setup should start with no retained project groups"
        );
        mw.workspace().clone()
    });
    assert!(
        workspace_a.read_with(cx, |workspace, _cx| workspace.session_id().is_some()),
        "initial active workspace should start attached to the session"
    );

    let workspace_b = multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(project_b, window, cx)
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |mw, cx| {
        assert_eq!(
            mw.workspace().entity_id(),
            workspace_b.entity_id(),
            "the new workspace should become active"
        );
        assert_eq!(
            mw.workspaces().count(),
            2,
            "the previous active workspace should remain open after switching with the sidebar closed"
        );
        assert_eq!(mw.project_groups(cx).len(), 2);
    });

    assert!(
        workspace_a.read_with(cx, |workspace, _cx| workspace.session_id().is_some()),
        "the previous active workspace should remain attached when switching away with the sidebar closed"
    );
}

#[gpui::test]
async fn test_remote_project_root_dir_changes_update_groups(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    fs.insert_tree("/local_b", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs.clone(), ["/root_a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/local_b".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));

    multi_workspace.update(cx, |mw, cx| {
        mw.open_sidebar(cx);
    });
    cx.run_until_parked();

    let workspace_b = multi_workspace.update_in(cx, |mw, window, cx| {
        let workspace = cx.new(|cx| Workspace::test_new(project_b.clone(), window, cx));
        let key = workspace.read(cx).project_group_key(cx);
        mw.activate_provisional_workspace(workspace.clone(), key, window, cx);
        workspace
    });
    cx.run_until_parked();

    multi_workspace.read_with(cx, |mw, _cx| {
        assert_eq!(
            mw.workspace().entity_id(),
            workspace_b.entity_id(),
            "registered workspace should become active"
        );
    });

    let initial_key = project_b.read_with(cx, |p, cx| p.project_group_key(cx));
    multi_workspace.read_with(cx, |mw, _cx| {
        let keys = mw.project_group_keys();
        assert!(
            keys.contains(&initial_key),
            "project groups should contain the initial key for the registered workspace"
        );
    });

    let remote_worktree = project_b.update(cx, |project, cx| {
        project.add_test_remote_worktree("/remote/project", cx)
    });
    cx.run_until_parked();

    let worktree_id = remote_worktree.read_with(cx, |wt, _| wt.id().to_proto());
    remote_worktree.update(cx, |worktree, _cx| {
        worktree
            .as_remote()
            .unwrap()
            .update_from_remote(proto::UpdateWorktree {
                project_id: 0,
                worktree_id,
                abs_path: "/remote/project".to_string(),
                root_name: "project".to_string(),
                updated_entries: vec![proto::Entry {
                    id: 1,
                    is_dir: true,
                    path: "".to_string(),
                    inode: 1,
                    mtime: Some(proto::Timestamp {
                        seconds: 0,
                        nanos: 0,
                    }),
                    is_ignored: false,
                    is_hidden: false,
                    is_external: false,
                    is_fifo: false,
                    size: None,
                    canonical_path: None,
                }],
                removed_entries: vec![],
                scan_id: 1,
                is_last_update: true,
                updated_repositories: vec![],
                removed_repositories: vec![],
                root_repo_common_dir: None,
                root_repo_is_linked_worktree: false,
            });
    });
    cx.run_until_parked();

    let updated_key = project_b.read_with(cx, |p, cx| p.project_group_key(cx));
    assert_ne!(
        initial_key, updated_key,
        "remote worktree update should change the project group key"
    );

    multi_workspace.read_with(cx, |mw, _cx| {
        let keys = mw.project_group_keys();
        assert!(
            keys.contains(&updated_key),
            "project groups should contain the updated key after remote change; got {keys:?}"
        );
        assert!(
            !keys.contains(&initial_key),
            "project groups should no longer contain the stale initial key; got {keys:?}"
        );
    });
}

#[gpui::test]
async fn test_open_project_closes_empty_workspace_but_not_non_empty_ones(cx: &mut TestAppContext) {
    init_test(cx);
    let app_state = cx.update(AppState::test);
    let fs = app_state.fs.as_fake();
    fs.insert_tree(path!("/project_a"), json!({ "file_a.txt": "" }))
        .await;
    fs.insert_tree(path!("/project_b"), json!({ "file_b.txt": "" }))
        .await;

    // Start with an empty (no-worktrees) workspace.
    let project = Project::test(app_state.fs.clone(), [], cx).await;
    let window = cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));
    cx.run_until_parked();

    window
        .update(cx, |mw, _window, cx| mw.open_sidebar(cx))
        .unwrap();
    cx.run_until_parked();

    let empty_workspace = window
        .read_with(cx, |mw, _| mw.workspace().clone())
        .unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);

    // Add a dirty untitled item to the empty workspace.
    let dirty_item = cx.new(|cx| TestItem::new(cx).with_dirty(true));
    empty_workspace.update_in(cx, |workspace, window, cx| {
        workspace.add_item_to_active_pane(Box::new(dirty_item.clone()), None, true, window, cx);
    });

    // Opening a project while the lone empty workspace has unsaved
    // changes prompts the user.
    let open_task = window
        .update(cx, |mw, window, cx| {
            mw.open_project(
                vec![PathBuf::from(path!("/project_a"))],
                OpenMode::Activate,
                window,
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();

    // Cancelling keeps the empty workspace.
    assert!(cx.has_pending_prompt(),);
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(open_task.await.unwrap(), empty_workspace);
    window
        .read_with(cx, |mw, _cx| {
            assert_eq!(mw.workspaces().count(), 1);
            assert_eq!(mw.workspace(), &empty_workspace);
            assert_eq!(mw.project_group_keys(), vec![]);
        })
        .unwrap();

    // Discarding the unsaved changes closes the empty workspace
    // and opens the new project in its place.
    let open_task = window
        .update(cx, |mw, window, cx| {
            mw.open_project(
                vec![PathBuf::from(path!("/project_a"))],
                OpenMode::Activate,
                window,
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();

    assert!(cx.has_pending_prompt(),);
    cx.simulate_prompt_answer("Don't Save");
    cx.run_until_parked();

    let workspace_a = open_task.await.unwrap();
    assert_ne!(workspace_a, empty_workspace);

    window
        .read_with(cx, |mw, _cx| {
            assert_eq!(mw.workspaces().count(), 1);
            assert_eq!(mw.workspace(), &workspace_a);
            assert_eq!(
                mw.project_group_keys(),
                vec![ProjectGroupKey::new(
                    None,
                    PathList::new(&[path!("/project_a")])
                )]
            );
        })
        .unwrap();
    assert!(
        empty_workspace.read_with(cx, |workspace, _cx| workspace.session_id().is_none()),
        "the detached empty workspace should no longer be attached to the session",
    );

    let dirty_item = cx.new(|cx| TestItem::new(cx).with_dirty(true));
    workspace_a.update_in(cx, |workspace, window, cx| {
        workspace.add_item_to_active_pane(Box::new(dirty_item.clone()), None, true, window, cx);
    });
    cx.update(|_window, cx| {
        DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
    });
    cx.run_until_parked();

    // Opening another project does not close the existing project or prompt.
    let workspace_b = window
        .update(cx, |mw, window, cx| {
            mw.open_project(
                vec![PathBuf::from(path!("/project_b"))],
                OpenMode::Activate,
                window,
                cx,
            )
        })
        .unwrap()
        .await
        .unwrap();
    cx.run_until_parked();

    assert!(!cx.has_pending_prompt());
    assert_ne!(workspace_b, workspace_a);
    window
        .read_with(cx, |mw, _cx| {
            assert_eq!(mw.workspaces().count(), 2);
            assert_eq!(mw.workspace(), &workspace_b);
            assert_eq!(
                mw.project_group_keys(),
                vec![
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_b")])),
                    ProjectGroupKey::new(None, PathList::new(&[path!("/project_a")]))
                ]
            );
        })
        .unwrap();
    assert!(workspace_a.read_with(cx, |workspace, _cx| workspace.session_id().is_some()),);
}

#[gpui::test]
async fn test_close_workspace_with_remote_neighbor_does_not_create_local_workspace(
    cx: &mut TestAppContext,
) {
    // Regression test: closing a workspace whose neighboring group is
    // remote with no existing workspace should not create a local
    // workspace with the remote paths.
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs, ["/root_a".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));

    multi_workspace.update(cx, |mw, cx| {
        mw.open_sidebar(cx);
    });
    cx.run_until_parked();

    // Add a mock-remote group with no workspace as the second group.
    let remote_key = ProjectGroupKey::new(
        Some(RemoteConnectionOptions::Mock(
            remote::MockConnectionOptions { id: 1 },
        )),
        PathList::new(&[PathBuf::from("/remote/project")]),
    );
    multi_workspace.update(cx, |mw, _cx| {
        mw.test_add_project_group(ProjectGroup {
            key: remote_key.clone(),
            workspaces: Vec::new(),
            expanded: true,
        });
    });

    let workspace_a = multi_workspace.read_with(cx, |mw, _cx| mw.workspace().clone());

    // Close workspace A. The neighbor is the remote group with no workspace.
    // The fix should skip find_or_create_local_workspace and fall through
    // to creating an empty workspace instead.
    multi_workspace
        .update_in(cx, |mw, window, cx| {
            mw.remove(
                [workspace_a.clone()],
                RemovalIntent::CloseProject,
                window,
                cx,
            )
        })
        .await
        .expect("close_workspace should succeed");

    cx.run_until_parked();

    multi_workspace.update(cx, |mw, cx| {
        // The active workspace should NOT be a local workspace with the
        // remote paths. It should be an empty workspace (no worktrees).
        let workspaces: Vec<_> = mw.workspaces().cloned().collect();
        for ws in &workspaces {
            let key = ws.read(cx).project_group_key(cx);
            assert!(
                key.host().is_some()
                    || key.path_list().paths() != [PathBuf::from("/remote/project")],
                "remote neighbor should not have created a local workspace"
            );
        }
    });
}

#[gpui::test]
async fn test_remove_project_group_with_remote_neighbor_does_not_create_local_workspace(
    cx: &mut TestAppContext,
) {
    // Regression test: removing a project group whose neighboring group is
    // remote with no workspace should not create a local workspace with
    // the remote paths.
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/root_a", json!({ "file.txt": "" })).await;
    let project_a = Project::test(fs, ["/root_a".as_ref()], cx).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));

    multi_workspace.update(cx, |mw, cx| {
        mw.open_sidebar(cx);
    });
    cx.run_until_parked();

    let key_a = project_a.read_with(cx, |p, cx| p.project_group_key(cx));

    // Add a mock-remote group with no workspace.
    let remote_key = ProjectGroupKey::new(
        Some(RemoteConnectionOptions::Mock(
            remote::MockConnectionOptions { id: 1 },
        )),
        PathList::new(&[PathBuf::from("/remote/project")]),
    );
    multi_workspace.update(cx, |mw, _cx| {
        mw.test_add_project_group(ProjectGroup {
            key: remote_key.clone(),
            workspaces: Vec::new(),
            expanded: true,
        });
    });

    // Remove the local group A. The neighbor is the remote group with no
    // workspace. The fix should skip find_or_create_local_workspace and
    // fall through to creating an empty workspace.
    multi_workspace
        .update_in(cx, |mw, window, cx| {
            mw.remove_project_group(&key_a, window, cx)
        })
        .await
        .expect("remove_project_group should succeed");

    cx.run_until_parked();

    multi_workspace.update(cx, |mw, cx| {
        let workspaces: Vec<_> = mw.workspaces().cloned().collect();
        for ws in &workspaces {
            let key = ws.read(cx).project_group_key(cx);
            assert!(
                key.host().is_some() || key.path_list().paths() != [PathBuf::from("/remote/project")],
                "remote neighbor should not have created a local workspace after remove_project_group"
            );
        }
    });
}

#[gpui::test]
async fn test_nearest_retained_workspace(cx: &mut TestAppContext) {
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    fs.insert_tree("/project-b", json!({})).await;
    fs.insert_tree("/project-c", json!({})).await;
    fs.insert_tree("/project-d", json!({})).await;

    // These two projects create separate workspaces in the same project group. The second
    // workspace is activated after the first, making it the group's last active workspace.
    let project_a_1 = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_a_2 = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/project-b".as_ref()], cx).await;
    let project_c = Project::test(fs.clone(), ["/project-c".as_ref()], cx).await;
    let project_d = Project::test(fs, ["/project-d".as_ref()], cx).await;
    let key_a = project_a_1.read_with(cx, |project, cx| project.project_group_key(cx));
    let key_b = project_b.read_with(cx, |project, cx| project.project_group_key(cx));
    let key_c = project_c.read_with(cx, |project, cx| project.project_group_key(cx));
    let key_d = project_d.read_with(cx, |project, cx| project.project_group_key(cx));

    let (multi_workspace, cx) = setup_multi_workspace(
        &[project_a_1, project_a_2, project_b, project_c, project_d],
        cx,
    );

    multi_workspace.update(cx, |multi_workspace, cx| {
        assert_eq!(
            multi_workspace.project_group_keys(),
            vec![key_d.clone(), key_c.clone(), key_b.clone(), key_a.clone()],
            "new project groups should be inserted before existing groups"
        );

        let group_c_index = multi_workspace
            .project_groups(cx)
            .iter()
            .position(|project_group| project_group.key == key_c)
            .expect("project group for project-c should exist");
        let workspace_b = multi_workspace
            .workspaces_for_project_group(&key_b, cx)
            .into_iter()
            .next()
            .expect("workspace for project-b should exist");
        let workspace_d = multi_workspace
            .workspaces_for_project_group(&key_d, cx)
            .into_iter()
            .next()
            .expect("workspace for project-d should exist");
        let retained_workspaces_a = multi_workspace
            .workspaces_for_project_group(&key_a, cx);
        assert_eq!(
            retained_workspaces_a.len(),
            2,
            "project group A should retain both workspaces"
        );
        let workspace_a_1 = retained_workspaces_a
            .first()
            .expect("project group A should have a retained workspace")
            .clone();
        let workspace_a_2 = multi_workspace
            .last_active_workspace_for_group(&key_a, cx)
            .expect("project group A should have a last active workspace");
        assert_ne!(
            workspace_a_1, workspace_a_2,
            "project group A's last active workspace should differ from its first retained workspace"
        );

        // Since Project Group B is the one after C, it is preferred over
        // Project Group D, even if they're at the same distance.
        assert_eq!(
            multi_workspace.nearest_retained_workspace(group_c_index, &[], cx),
            Some(workspace_b.clone()),
            "the following project group should be preferred at equal distance"
        );

        // With Project Group B being excluded, Project Group D is picked as it
        // is the one with the smallest distance.
        assert_eq!(
            multi_workspace.nearest_retained_workspace(
                group_c_index,
                std::slice::from_ref(&workspace_b),
                cx,
            ),
            Some(workspace_d.clone()),
            "the preceding project group should be used when the following workspace is excluded"
        );

        // With both adjacent Project Groups excluded, the search expands and
        // reaches Project A at distance 2 and prefers its last active workspace (A2)
        // over its first retained workspace (A1).
        assert_eq!(
            multi_workspace.nearest_retained_workspace(
                group_c_index,
                &[workspace_b.clone(), workspace_d.clone()],
                cx
            ),
            Some(workspace_a_2.clone()),
            "the farther group's last active workspace should be preferred"
        );

        // With the group's most recently activated workspace excluded, the
        // search falls back to the member activated before it.
        assert_eq!(
            multi_workspace.nearest_retained_workspace(
                group_c_index,
                &[
                    workspace_b.clone(),
                    workspace_d.clone(),
                    workspace_a_2.clone()
                ],
                cx
            ),
            Some(workspace_a_1.clone()),
            "the previously activated workspace should be used when the last active one is excluded"
        );

        // Excluding every neighboring workspace exhausts the search.
        assert_eq!(
            multi_workspace.nearest_retained_workspace(
                group_c_index,
                &[
                    workspace_b,
                    workspace_d,
                    workspace_a_1,
                    workspace_a_2,
                ],
                cx
            ),
            None,
            "no workspace should be returned when every candidate is excluded"
        );
    });
}

#[gpui::test]
async fn test_nearest_retained_workspace_skips_disconnected_workspace(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    fs.insert_tree("/project-b", json!({})).await;

    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    let key_a = project_a.read_with(cx, |project, cx| project.project_group_key(cx));
    let (multi_workspace, cx) = setup_multi_workspace(&[project_a.clone(), project_b.clone()], cx);

    project_b.update(cx, |project, cx| {
        project.mark_as_collab_for_testing();
        project.disconnected_from_host(cx);
    });
    cx.run_until_parked();

    multi_workspace.update(cx, |multi_workspace, cx| {
        let group_a_index = multi_workspace
            .project_groups(cx)
            .iter()
            .position(|group| group.key == key_a)
            .expect("project group A should exist");

        assert_eq!(
            multi_workspace.nearest_retained_workspace(group_a_index, &[], cx),
            None,
            "a disconnected workspace should not be selected as a fallback"
        );
    });
}

#[gpui::test]
async fn workspace_configuration_checkpoint_save_as_and_live_changes(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    fs.insert_tree("/project-b", json!({})).await;
    fs.insert_tree("/project-c", json!({})).await;
    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs.clone(), ["/project-b".as_ref()], cx).await;
    let project_c = Project::test(fs, ["/project-c".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project_a, project_b], cx);
    let (workspace_a, workspace_b) = multi_workspace.update(cx, |multi_workspace, cx| {
        let workspaces = multi_workspace.ordered_workspaces(cx);
        for workspace in &workspaces {
            workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
        }
        let [workspace_a, workspace_b] = workspaces.as_slice() else {
            panic!("expected exactly two workspaces");
        };
        (workspace_a.clone(), workspace_b.clone())
    });

    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Daily".to_string(), cx)
    });
    let configuration_id = match save.await {
        Ok(configuration_id) => configuration_id,
        Err(error) => panic!("failed to save workspace configuration: {error:#}"),
    };

    let configuration = match workspace_configuration(configuration_id, cx) {
        Some(configuration) => configuration,
        None => panic!("saved workspace configuration was not published"),
    };
    let workspace_a_id = match workspace_a.read_with(cx, |workspace, _cx| workspace.database_id()) {
        Some(workspace_id) => workspace_id,
        None => panic!("workspace A lost its database id"),
    };
    let workspace_b_id = match workspace_b.read_with(cx, |workspace, _cx| workspace.database_id()) {
        Some(workspace_id) => workspace_id,
        None => panic!("workspace B lost its database id"),
    };
    let initial_active_id = multi_workspace.read_with(cx, |multi_workspace, cx| {
        multi_workspace.workspace().read(cx).database_id()
    });
    let initial_active_id = match initial_active_id {
        Some(workspace_id) => workspace_id,
        None => panic!("the active workspace lost its database id"),
    };
    let (target_workspace, target_workspace_id) = if initial_active_id == workspace_a_id {
        (workspace_b.clone(), workspace_b_id)
    } else {
        (workspace_a.clone(), workspace_a_id)
    };
    assert_eq!(
        configuration
            .members
            .iter()
            .map(|member| member.workspace_id)
            .collect::<Vec<_>>(),
        vec![workspace_a_id, workspace_b_id]
    );
    assert_eq!(configuration.active_member, Some(initial_active_id));
    assert_eq!(
        multi_workspace.read_with(cx, |multi_workspace, _cx| {
            multi_workspace.active_configuration_id()
        }),
        Some(configuration_id)
    );

    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.activate(target_workspace, None, window, cx);
    });
    multi_workspace.update(cx, |multi_workspace, cx| {
        assert!(multi_workspace.move_workspace_tab_to_index(&workspace_b, 0, cx));
    });
    let checkpoint = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.retry_configuration_checkpoint(cx)
    });
    if let Err(error) = checkpoint.await {
        panic!("failed to checkpoint live workspace changes: {error:#}");
    }

    let configuration = match workspace_configuration(configuration_id, cx) {
        Some(configuration) => configuration,
        None => panic!("live workspace configuration disappeared"),
    };
    assert_eq!(
        configuration
            .members
            .iter()
            .map(|member| member.workspace_id)
            .collect::<Vec<_>>(),
        vec![workspace_b_id, workspace_a_id]
    );
    assert_eq!(configuration.active_member, Some(target_workspace_id));
    assert_eq!(
        multi_workspace.read_with(cx, |multi_workspace, _cx| {
            multi_workspace
                .configuration_checkpoint_error()
                .map(str::to_string)
        }),
        None
    );

    let workspace_c = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.test_add_workspace(project_c, window, cx)
    });
    workspace_c.update(cx, |workspace, _cx| workspace.set_random_database_id());
    let workspace_c_id = match workspace_c.read_with(cx, |workspace, _cx| workspace.database_id()) {
        Some(workspace_id) => workspace_id,
        None => panic!("workspace C lost its database id"),
    };
    let checkpoint = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.retry_configuration_checkpoint(cx)
    });
    if let Err(error) = checkpoint.await {
        panic!("failed to checkpoint an added workspace: {error:#}");
    }
    let configuration = match workspace_configuration(configuration_id, cx) {
        Some(configuration) => configuration,
        None => panic!("workspace configuration disappeared after add"),
    };
    assert_eq!(configuration.members.len(), 3);
    assert!(
        configuration
            .members
            .iter()
            .any(|member| member.workspace_id == workspace_c_id)
    );
    assert_eq!(configuration.active_member, Some(workspace_c_id));

    let close = multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.close_workspace(&workspace_c, window, cx)
    });
    match close.await {
        Ok(true) => {}
        Ok(false) => panic!("workspace C was not closed"),
        Err(error) => panic!("failed to close workspace C: {error:#}"),
    }
    let checkpoint = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.retry_configuration_checkpoint(cx)
    });
    if let Err(error) = checkpoint.await {
        panic!("failed to checkpoint a closed workspace: {error:#}");
    }
    let configuration = match workspace_configuration(configuration_id, cx) {
        Some(configuration) => configuration,
        None => panic!("workspace configuration disappeared after close"),
    };
    assert_eq!(configuration.members.len(), 2);
    assert!(
        configuration
            .members
            .iter()
            .all(|member| member.workspace_id != workspace_c_id)
    );
}

#[gpui::test]
async fn workspace_configuration_checkpoint_save_as_publishes_nothing_after_source_drift(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    fs.insert_tree("/project-b", json!({})).await;
    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project_a], cx);
    multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace
            .workspace()
            .update(cx, |workspace, _cx| workspace.set_random_database_id());
    });
    let (configuration_created, resume_save) = multi_workspace
        .update(cx, |multi_workspace, _cx| {
            multi_workspace.pause_configuration_save_for_test()
        });
    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Drifted".to_string(), cx)
    });

    configuration_created
        .await
        .expect("save ended before the final publication barrier");
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        let workspace = multi_workspace.test_add_workspace(project_b, window, cx);
        workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
    });
    resume_save
        .send(())
        .expect("save dropped the final publication barrier");
    let error = save
        .await
        .expect_err("save published a configuration after its source changed");
    assert!(format!("{error:#}").contains("workspace set changed"));
    assert!(workspace_configuration_store_is_empty(cx));
    multi_workspace.read_with(cx, |multi_workspace, _cx| {
        assert_eq!(multi_workspace.active_configuration_id(), None);
    });
}

#[gpui::test]
async fn workspace_configuration_checkpoint_failure_is_stale_until_retry(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    fs.insert_tree("/project-b", json!({})).await;
    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project_a, project_b], cx);
    let (workspace_a, workspace_b) = multi_workspace.update(cx, |multi_workspace, cx| {
        let workspaces = multi_workspace.ordered_workspaces(cx);
        for workspace in &workspaces {
            workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
        }
        let [workspace_a, workspace_b] = workspaces.as_slice() else {
            panic!("expected exactly two workspaces");
        };
        (workspace_a.clone(), workspace_b.clone())
    });
    let workspace_a_id = match workspace_a.read_with(cx, |workspace, _cx| workspace.database_id()) {
        Some(workspace_id) => workspace_id,
        None => panic!("workspace A lost its database id"),
    };
    let workspace_b_id = match workspace_b.read_with(cx, |workspace, _cx| workspace.database_id()) {
        Some(workspace_id) => workspace_id,
        None => panic!("workspace B lost its database id"),
    };
    let initial_active_id = multi_workspace.read_with(cx, |multi_workspace, cx| {
        multi_workspace.workspace().read(cx).database_id()
    });
    let initial_active_id = match initial_active_id {
        Some(workspace_id) => workspace_id,
        None => panic!("the active workspace lost its database id"),
    };
    let (target_workspace, target_workspace_id) = if initial_active_id == workspace_a_id {
        (workspace_b, workspace_b_id)
    } else {
        (workspace_a, workspace_a_id)
    };
    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Recovery".to_string(), cx)
    });
    let configuration_id = match save.await {
        Ok(configuration_id) => configuration_id,
        Err(error) => panic!("failed to save initial workspace configuration: {error:#}"),
    };

    let workspace_db = cx.update(|_window, cx| WorkspaceDb::global(cx));
    if let Err(error) = workspace_db.set_query_only_for_tests(true).await {
        panic!("failed to force workspace checkpoint failure: {error:#}");
    }
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.activate(target_workspace, None, window, cx);
    });
    let failed_checkpoint = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.retry_configuration_checkpoint(cx)
    });
    assert!(failed_checkpoint.await.is_err());
    if let Err(error) = workspace_db.set_query_only_for_tests(false).await {
        panic!("failed to restore workspace database writes: {error:#}");
    }

    assert!(multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.configuration_checkpoint_error().is_some()
    }));
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.close_sidebar(window, cx);
    });
    cx.run_until_parked();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(600.)),
        |_, _| multi_workspace.clone().into_any_element(),
    );
    let heading_bounds = cx.debug_bounds("WORKSPACE-TABS-HEADING");
    let trigger_bounds = cx.debug_bounds("WORKSPACE-CONFIGURATION-TRIGGER-CONTENT");
    let stale_warning_bounds = cx.debug_bounds("WORKSPACE-CONFIGURATION-STALE-WARNING");
    assert!(
        stale_warning_bounds.is_some(),
        "a stale active configuration must remain visibly marked without opening the menu; heading={heading_bounds:?}, trigger={trigger_bounds:?}"
    );
    let configuration = match workspace_configuration(configuration_id, cx) {
        Some(configuration) => configuration,
        None => panic!("failed live checkpoint removed the prior configuration"),
    };
    assert_eq!(configuration.active_member, Some(initial_active_id));

    let retry = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.retry_configuration_checkpoint(cx)
    });
    if let Err(error) = retry.await {
        panic!("workspace configuration retry failed: {error:#}");
    }
    assert_eq!(
        multi_workspace.read_with(cx, |multi_workspace, _cx| {
            multi_workspace
                .configuration_checkpoint_error()
                .map(str::to_string)
        }),
        None
    );
    let configuration = match workspace_configuration(configuration_id, cx) {
        Some(configuration) => configuration,
        None => panic!("retried workspace configuration disappeared"),
    };
    assert_eq!(configuration.active_member, Some(target_workspace_id));
}

#[gpui::test]
async fn workspace_configuration_checkpoint_save_as_write_failure_publishes_nothing(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    let project = Project::test(fs, ["/project-a".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project], cx);
    multi_workspace.update(cx, |multi_workspace, cx| {
        for workspace in multi_workspace.workspaces() {
            workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
        }
    });
    let kvp = cx.update(|_window, cx| KeyValueStore::global(cx));
    set_configuration_kvp_query_only(&kvp, true).await;
    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Broken".to_string(), cx)
    });
    assert!(save.await.is_err());
    set_configuration_kvp_query_only(&kvp, false).await;

    assert_eq!(
        multi_workspace.read_with(cx, |multi_workspace, _cx| {
            multi_workspace.active_configuration_id()
        }),
        None
    );
    assert!(workspace_configuration_store_is_empty(cx));
}

#[gpui::test]
async fn workspace_configuration_checkpoint_save_as_rejects_remote_members(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    project.update(cx, |project, _cx| project.mark_as_collab_for_testing());
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project], cx);
    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Remote".to_string(), cx)
    });
    let error = match save.await {
        Ok(_) => panic!("remote workspace configuration was unexpectedly saved"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("local workspaces"));
    assert_eq!(
        multi_workspace.read_with(cx, |multi_workspace, _cx| {
            multi_workspace.active_configuration_id()
        }),
        None
    );
    assert!(workspace_configuration_store_is_empty(cx));
}

#[gpui::test]
async fn workspace_configuration_checkpoint_quit_retries_stale_configuration(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    fs.insert_tree("/project-b", json!({})).await;
    let project_a = Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let project_b = Project::test(fs, ["/project-b".as_ref()], cx).await;
    reset_workspace_configuration_store(cx).await;

    let (multi_workspace, cx) = setup_multi_workspace(&[project_a, project_b], cx);
    let (target_workspace, target_workspace_id, window_id) =
        multi_workspace.update(cx, |multi_workspace, cx| {
            let workspaces = multi_workspace.ordered_workspaces(cx);
            for workspace in &workspaces {
                workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
            }
            let active_workspace = multi_workspace.workspace();
            let target_workspace = match workspaces
                .iter()
                .find(|workspace| *workspace != active_workspace)
            {
                Some(workspace) => workspace.clone(),
                None => panic!("expected a non-active workspace"),
            };
            let target_workspace_id = match target_workspace.read(cx).database_id() {
                Some(workspace_id) => workspace_id,
                None => panic!("target workspace lost its database id"),
            };
            (
                target_workspace,
                target_workspace_id,
                multi_workspace.test_window_id(),
            )
        });
    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Quit Recovery".to_string(), cx)
    });
    let configuration_id = match save.await {
        Ok(configuration_id) => configuration_id,
        Err(error) => panic!("failed to save initial configuration: {error:#}"),
    };

    let workspace_db = cx.update(|_window, cx| WorkspaceDb::global(cx));
    if let Err(error) = workspace_db.set_query_only_for_tests(true).await {
        panic!("failed to force stale configuration: {error:#}");
    }
    multi_workspace.update_in(cx, |multi_workspace, window, cx| {
        multi_workspace.activate(target_workspace, None, window, cx);
    });
    let failed_checkpoint = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.retry_configuration_checkpoint(cx)
    });
    assert!(failed_checkpoint.await.is_err());
    if let Err(error) = workspace_db.set_query_only_for_tests(false).await {
        panic!("failed to restore workspace writes before quit: {error:#}");
    }

    assert!(multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.configuration_checkpoint_error().is_some()
    }));
    assert_eq!(
        multi_workspace.read_with(cx, |multi_workspace, cx| {
            multi_workspace.workspace().read(cx).database_id()
        }),
        Some(target_workspace_id),
        "the stale checkpoint must not revert the active workspace before quit"
    );
    let kvp = cx.update(|_window, cx| KeyValueStore::global(cx));

    cx.executor().allow_parking();
    let app = cx.cx.clone();
    app.quit();

    let configurations = kvp
        .scoped("workspace_configurations")
        .read("collection")
        .ok()
        .flatten()
        .and_then(|json| {
            serde_json::from_str::<crate::persistence::model::WorkspaceConfigurationCollection>(
                &json,
            )
            .ok()
        });
    let configurations = match configurations {
        Some(configurations) => configurations,
        None => panic!("quit retry did not persist workspace configurations"),
    };
    let configuration = match configurations.find(configuration_id) {
        Some(configuration) => configuration,
        None => panic!("quit retry lost the workspace configuration"),
    };
    assert_eq!(configuration.active_member, Some(target_workspace_id));
    let state_after_quit = kvp
        .scoped("multi_workspace_state")
        .read(&window_id.as_u64().to_string())
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<MultiWorkspaceState>(&json).ok());
    let state_after_quit = match state_after_quit {
        Some(state) => state,
        None => panic!("quit retry did not persist multi-workspace state"),
    };
    assert_eq!(
        state_after_quit.active_configuration_id,
        Some(configuration_id)
    );
}

#[gpui::test]
async fn workspace_configuration_checkpoint_quit_after_save(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree("/project-a", json!({})).await;
    let project = Project::test(fs, ["/project-a".as_ref()], cx).await;
    cx.update(WorkspaceConfigurationStore::init);

    let (multi_workspace, cx) = setup_multi_workspace(&[project], cx);
    multi_workspace.update(cx, |multi_workspace, cx| {
        for workspace in multi_workspace.workspaces() {
            workspace.update(cx, |workspace, _cx| workspace.set_random_database_id());
        }
    });
    let save = multi_workspace.update(cx, |multi_workspace, cx| {
        multi_workspace.save_configuration_as("Quit".to_string(), cx)
    });
    if let Err(error) = save.await {
        panic!("failed to save the configuration before quit: {error:#}");
    }
    let app = cx.cx.clone();
    app.quit();
}
