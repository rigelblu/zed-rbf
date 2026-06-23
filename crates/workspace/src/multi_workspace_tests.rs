use std::path::PathBuf;

use super::*;
use crate::item::test::TestItem;
use agent_settings::AgentSettings;
use client::proto;
use fs::{FakeFs, Fs};
use gpui::{IntoElement, MouseButton, TestAppContext, VisualTestContext, WindowId, div};
use project::DisableAiSettings;
use serde_json::json;
use settings::{Settings, SettingsStore};
use ui::utils::platform_title_bar_height;
use util::path;

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        DisableAiSettings::register(cx);
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

    let window = first_open.window.clone();
    cx.update(|cx| {
        Workspace::new_local(
            vec![PathBuf::from(path!("/project_b"))],
            app_state.clone(),
            Some(window.clone()),
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

    let window = first_open.window.clone();
    cx.update(|cx| {
        Workspace::new_local(
            vec![PathBuf::from(path!("/project_b"))],
            app_state,
            Some(window.clone()),
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
            let repo_group_workspaces = multi_workspace
                .workspaces_for_project_group(&repo_group_key, cx)
                .expect("restored parent project group should exist");
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
                .expect("restored parent project group should exist")
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
                &[],
                None,
                OpenMode::Activate,
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
                &[],
                None,
                OpenMode::Activate,
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
                &[],
                None,
                OpenMode::Activate,
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
            multi_workspace.close_workspace(&workspace_a, window, cx)
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
                .unwrap_or_default()
                .is_empty(),
            "the unloaded neighboring group C should remain unopened"
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
