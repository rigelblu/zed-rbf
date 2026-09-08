use crate::{
    ItemHandle, MultiWorkspace, Pane, SidebarSide, StatusBarItemLocation, StatusBarSettings,
    ToggleWorkspaceSidebar, WorkspaceLayoutSettings, layout_registry::CustomizeWorkspaceLayout,
    sidebar_side_context_menu,
};
use gpui::{
    Anchor, AnyView, App, Context, Decorations, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, Role, SharedString, Styled, Subscription, WeakEntity, Window,
};
use settings::Settings as _;
use settings::{SettingsContent, update_settings_file, update_settings_file_with_completion};
use std::{any::TypeId, sync::Arc};
use theme::CLIENT_SIDE_DECORATION_ROUNDING;
use ui::{ContextMenu, IconPosition, Indicator, Tooltip, prelude::*, right_click_menu};

/// Describes how a status-bar item can be hidden by the user.
///
/// Every [`StatusItemView`] must either provide this (so that the user gets a
/// "Hide Button" entry in the right-click menu) or explicitly return `None`
/// to opt out. Returning `None` should be reserved for items that are
/// already conditional on some other setting exposed elsewhere (e.g., the
/// activity indicator, which disappears on its own once there's no work to
/// display).
#[derive(Clone)]
pub struct HideStatusItem {
    hide: Arc<dyn Fn(&mut SettingsContent) + Send + Sync>,
}

impl HideStatusItem {
    pub fn new(hide: impl Fn(&mut SettingsContent) + Send + Sync + 'static) -> Self {
        Self {
            hide: Arc::new(hide),
        }
    }

    /// Persists the hide by updating the user settings file.
    pub fn apply(&self, cx: &App) {
        let hide = self.hide.clone();
        let fs = <dyn fs::Fs>::global(cx);
        update_settings_file(fs, cx, move |settings, _cx| (hide)(settings));
    }
}

pub trait StatusItemView: Render {
    /// Event callback that is triggered when the active pane item changes.
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn crate::ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// Returns metadata describing how this item can be hidden from the
    /// status bar by writing to the user settings file.
    ///
    /// Implementors that return `None` must be inherently conditional on
    /// another user-exposed setting; otherwise, they should return `Some` so
    /// that the status bar can show a "Hide Button" entry in its
    /// right-click menu.
    fn hide_setting(&self, cx: &App) -> Option<HideStatusItem>;
}

trait StatusItemViewHandle: Send {
    fn to_any(&self) -> AnyView;
    fn set_active_pane_item(
        &self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    );
    fn item_type(&self) -> TypeId;
    fn hide_setting(&self, cx: &App) -> Option<HideStatusItem>;
}

#[derive(Default)]
struct SidebarStatus {
    open: bool,
    side: SidebarSide,
    has_notifications: bool,
}

impl SidebarStatus {
    /// The slot's own state: whether it is open at all, and on which side.
    ///
    /// This is the question the bar's rounded corners ask — a corner is square when
    /// the slot covers it, whichever occupant happens to be showing.
    fn slot(multi_workspace: &Option<WeakEntity<MultiWorkspace>>, cx: &App) -> Self {
        multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| {
                let mw = mw.read(cx);
                Self {
                    open: mw.sidebar_open() && mw.sidebar_slot_enabled(cx),
                    side: mw.sidebar_side(cx),
                    has_notifications: mw.sidebar_has_notifications(cx),
                }
            })
            .unwrap_or_default()
    }

    /// The slot's state as `id`'s own icon should read it.
    ///
    /// `open` here is narrower than the slot's flag: the slot holds one occupant at
    /// a time, so an icon reads open only when the slot is open *and* showing that
    /// occupant. Using the slot's flag would light both icons at once, which is the
    /// opposite of the exclusive on/off switch the two are specified to be.
    fn query(
        id: &'static str,
        multi_workspace: &Option<WeakEntity<MultiWorkspace>>,
        cx: &App,
    ) -> Self {
        let showing_this_occupant = multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .is_some_and(|mw| mw.read(cx).active_sidebar_id() == Some(id));
        let slot = Self::slot(multi_workspace, cx);
        Self {
            open: slot.open && showing_this_occupant,
            ..slot
        }
    }
}

/// Binds a live status control to its registry id and to the legacy setting that
/// governed it before placement existed.
///
/// It carries only what a static table cannot: the id, and a closure reading that
/// item's own setting. Label, icon, aliases, registered position and valid zones
/// all come from `layout_registry`. The closure exists because those settings live
/// in crates `workspace` cannot depend on — `editor`'s `search.button`, `git_ui`'s
/// inline blame — so it is supplied where the status bar is assembled.
#[derive(Clone)]
pub struct StatusControlAdapter {
    id: &'static str,
    legacy_enabled: Arc<dyn Fn(&App) -> bool + Send + Sync>,
    /// Whether the item exists to be placed at all, for items whose presence is
    /// not a placement question. A component-backed item cannot answer this from
    /// its position — the threads sidebar has a side whether or not the agent is
    /// enabled — and it is deliberately *not* `legacy_enabled` wearing a second
    /// hat: that closure answers "did the user's old show/hide key say yes",
    /// which an item that never had one cannot be asked.
    ///
    /// Unavailable resolves to `Hidden` rather than to an empty element, because
    /// a zone item that renders nothing still takes its gap — the defect dogfood
    /// round B4-6 found.
    available: Arc<dyn Fn(&App) -> bool + Send + Sync>,
}

impl StatusControlAdapter {
    pub fn new(
        id: &'static str,
        legacy_enabled: impl Fn(&App) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            id,
            legacy_enabled: Arc::new(legacy_enabled),
            available: Arc::new(|_| true),
        }
    }

    /// A component-backed bar item: placed by `workspace_layout.component_positions`
    /// like the component it opens, and present only while `available` says so.
    pub fn for_component(
        id: &'static str,
        available: impl Fn(&App) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            id,
            legacy_enabled: Arc::new(|_| true),
            available: Arc::new(available),
        }
    }

    pub fn id(&self) -> &'static str {
        self.id
    }

    fn effective_location(&self, cx: &App) -> Option<StatusBarItemLocation> {
        // A component-backed item is placed by the setting that moves its
        // component, so its icon cannot end up in a zone its component is not on.
        // The status-control resolver declines these ids by design, and this is
        // the sibling that answers them — asked first, because `legacy_enabled`
        // below is meaningless for an item that never had a legacy show/hide key.
        if let Some(zone) = WorkspaceLayoutSettings::get_global(cx).resolved_component_zone(self.id)
        {
            return Some(if (self.available)(cx) {
                zone
            } else {
                StatusBarItemLocation::Hidden
            });
        }
        StatusBarSettings::get_global(cx)
            .resolved_status_control_location(self.id, (self.legacy_enabled)(cx))
    }
}

struct StatusBarItem {
    item: Box<dyn StatusItemViewHandle>,
    adapter: Option<StatusControlAdapter>,
    /// Where the item sits when nothing else decides — for the dock button strips
    /// and anything else added without an adapter, this is the whole answer.
    default_location: StatusBarItemLocation,
}

impl StatusBarItem {
    fn effective_location(&self, cx: &App) -> StatusBarItemLocation {
        self.adapter
            .as_ref()
            .and_then(|adapter| adapter.effective_location(cx))
            .unwrap_or(self.default_location)
    }
}

/// Which items a zone renders, in the order it renders them.
///
/// Right renders reversed, so the item added first ends up furthest from the
/// editor — the ordering the two separate lists used to encode by having one of
/// them iterated backwards. `Hidden` names no zone, so an item resolving to it is
/// returned by no call: hiding *is* this filter.
///
/// Split out of rendering because this is the half that can be wrong, and the
/// rendered half cannot be asserted on — `cx.draw` panics once an element
/// registers a deferred draw, which a tooltip does.
fn zone_order(
    locations: &[StatusBarItemLocation],
    ids: &[Option<&'static str>],
    order: &[String],
    zone: StatusBarItemLocation,
) -> Vec<usize> {
    let mut indices: Vec<usize> = locations
        .iter()
        .enumerate()
        .filter(|(_, location)| **location == zone && zone != StatusBarItemLocation::Hidden)
        .map(|(index, _)| index)
        .collect();
    indices.sort_by_key(|index| zone_rank(ids.get(*index).copied().flatten(), *index, order));
    if zone == StatusBarItemLocation::Right {
        indices.reverse();
    }
    indices
}

/// Where an item sits inside its zone, as a sort key.
///
/// **The registry decides, and until 2026-09-06 it did not.** `zone_order` sorted by
/// registration index — the order `add_status_control` happens to be called in, in
/// one function in the `zed` crate — while `layout_registry` carried the identical
/// numbers, gap at 11 included, and the `n` in `SourceRank::StatusControl(n)` was
/// read by nothing: both call sites matched `StatusControl(_)` and discarded it. Two
/// sources of truth for one order, and the live one was a call order rather than a
/// table, which left status-control ordering no single sequence to write to.
///
/// Exactly the shape panels had before 2026-09-05, and switched the same way and for
/// the same reason. Checked rather than assumed: the fourteen table ranks and the
/// fourteen `add_status_control` calls name the same ids in the same order, which is
/// why this is a no-op and the unchanged suite is the proof.
///
/// An item the table does not know keeps its registration index and sorts **ahead**
/// of every registered control, because that is where it already sits — the dock
/// button strips carry no adapter and are added in `Workspace::new`, before any
/// status control exists.
///
/// `order` is the user's sequence from `status_bar.item_order`, and it outranks the
/// table. Four bands rather than three, because the strips have to stay in front of
/// everything: strips first, then listed controls by their place in the list, then
/// unlisted registered ones by `source_rank`, then anything else. So a partial list
/// orders only what it names, and an empty list reproduces the table exactly.
fn zone_rank(id: Option<&str>, registration_index: usize, order: &[String]) -> (u8, u32) {
    use crate::layout_registry::SourceRank;

    let rank = id
        .and_then(crate::layout_registry::get)
        .map(|item| item.source_rank);
    // Band 0 is the dock button strips, which stay in front of every control. Only
    // two things belong there: an item with no id at all, and a *component* id — a
    // panel is not a status control, and listing one here is inert for the same
    // reason it is inert in `item_locations`.
    //
    // An id the table simply does not know is **not** one of them. Putting the
    // recognised-control check here instead sent an unknown id to band 0, so a typo
    // in the user's list shoved itself ahead of every panel icon.
    let Some(id) = id else {
        return (0, registration_index as u32);
    };
    if matches!(rank, Some(SourceRank::Panel(_))) {
        return (0, registration_index as u32);
    }
    if let Some(index) = order.iter().position(|listed| listed == id) {
        return (1, index as u32);
    }
    match rank {
        Some(SourceRank::StatusControl(rank)) => (2, rank as u32),
        // Unknown and unlisted: behind everything the table places, which is where
        // an extension's status item already sits.
        _ => (3, registration_index as u32),
    }
}

pub struct StatusBar {
    /// One list rather than a list per zone: an item's zone is now resolved from
    /// settings on every render, so membership can no longer carry it.
    items: Vec<StatusBarItem>,
    active_pane: Entity<Pane>,
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
    focus_handle: FocusHandle,
    _observe_active_pane: Subscription,
}

impl Focusable for StatusBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for StatusBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = SidebarStatus::slot(&self.multi_workspace, cx);

        h_flex()
            .id("status-bar")
            .track_focus(&self.focus_handle)
            .key_context("StatusBar")
            // Expose the status bar as an ARIA toolbar so assistive technology
            // announces it as a toolbar and region navigation can reach its
            // controls. The controls inside form a tab group: region navigation
            // lands on the first control (per the ARIA toolbar pattern), Tab
            // steps through them, and arrow keys move between them once focus is
            // inside.
            .role(Role::Toolbar)
            .aria_label("Status bar")
            .tab_group()
            .on_key_down(
                cx.listener(|status_bar, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.modified() {
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "right" => {
                            status_bar.move_item_focus(true, window, cx);
                            cx.stop_propagation();
                        }
                        "left" => {
                            status_bar.move_item_focus(false, window, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }),
            )
            .w_full()
            .justify_between()
            .gap(DynamicSpacing::Base08.rems(cx))
            .p(DynamicSpacing::Base04.rems(cx))
            .bg(cx.theme().colors().status_bar_background)
            .map(|el| match window.window_decorations() {
                Decorations::Server => el,
                Decorations::Client { tiling, .. } => el
                    .when(
                        !(tiling.bottom || tiling.right)
                            && !(sidebar.open && sidebar.side == SidebarSide::Right),
                        |el| el.rounded_br(CLIENT_SIDE_DECORATION_ROUNDING),
                    )
                    .when(
                        !(tiling.bottom || tiling.left)
                            && !(sidebar.open && sidebar.side == SidebarSide::Left),
                        |el| el.rounded_bl(CLIENT_SIDE_DECORATION_ROUNDING),
                    )
                    // This border is to avoid a transparent gap in the rounded corners
                    .mb(px(-1.))
                    .mt({
                        #[cfg(target_os = "linux")]
                        let needs_gap_fix = {
                            // Running on Wayland and using some scaling levels other than 100% causes a
                            // 1px gap above the status bar; adding a margin avoids this.
                            gpui::guess_compositor() == "Wayland" && window.scale_factor() != 1.0
                        };
                        #[cfg(not(target_os = "linux"))]
                        let needs_gap_fix = false;
                        if needs_gap_fix { px(-1.) } else { px(0.) }
                    })
                    .border_b(px(1.0))
                    .border_color(cx.theme().colors().status_bar_background),
            })
            .child(self.render_left_tools(cx))
            // The middle zone sits between the two groups rather than being pinned
            // to the window centre. True centring — independent of how wide the
            // left group happens to be — comes with the overflow work, and until
            // then a placed item is visible here rather than lost.
            .child(self.render_middle_tools(cx))
            .child(self.render_right_tools(cx))
    }
}

impl StatusBar {
    fn render_left_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .min_w_0()
            .overflow_x_hidden()
            .children(self.render_zone(StatusBarItemLocation::Left, "status-bar-left", cx))
    }

    /// Items whose resolved zone is `location`, in canonical order. An item
    /// resolving to `Hidden` matches no zone and so renders nowhere — hiding is
    /// this filter, not a per-item concern.
    fn render_zone(
        &self,
        location: StatusBarItemLocation,
        side: &'static str,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let locations: Vec<_> = self
            .items
            .iter()
            .map(|entry| entry.effective_location(cx))
            .collect();
        let ids: Vec<Option<&'static str>> = self
            .items
            .iter()
            .map(|entry| entry.adapter.as_ref().map(|adapter| adapter.id))
            .collect();
        let order = StatusBarSettings::get_global(cx).item_order.clone();
        let strip = zone_control_strip(&locations, &ids, &order, location);
        let full = materialized_control_order(&order);
        let zone_indices = zone_order(&locations, &ids, &order, location);
        let last_control = zone_end_target_follows(&zone_indices, &ids);
        zone_indices
            .into_iter()
            .filter_map(|index| self.items.get(index).map(|entry| (index, entry)))
            .flat_map(|(index, entry)| {
                // The index is the item's position in the merged list, not a
                // per-zone counter, so ids stay unique when an item changes zone.
                //
                // `location` is handed down rather than re-resolved: it is the
                // zone that actually rendered this item, so a placement menu
                // cannot mark a zone the item is not in.
                let item = render_status_bar_item(
                    side,
                    index,
                    entry,
                    location,
                    &strip,
                    &full,
                    &self.focus_handle,
                    cx,
                );
                let end = (Some(index) == last_control)
                    .then(|| zone_end_drop_target(location, &strip, &full))
                    .flatten();
                std::iter::once(item).chain(end)
            })
            .collect()
    }

    fn render_middle_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .min_w_0()
            .overflow_x_hidden()
            .children(self.render_zone(StatusBarItemLocation::Middle, "status-bar-middle", cx))
    }

    fn render_right_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .gap_1()
            .overflow_x_hidden()
            .children(self.render_zone(StatusBarItemLocation::Right, "status-bar-right", cx))
    }
}

/// The threads sidebar's bar item.
///
/// It renders inside its zone like every other placeable item rather than being
/// appended beside one, so it takes the zone its component's position names and
/// the order the user drags it into. It also no longer vanishes when the sidebar
/// opens — it is a switch now, and a switch that disappears in one of its two
/// states cannot be switched back.
pub struct ThreadsSidebarButton {
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
}

impl ThreadsSidebarButton {
    pub fn new(multi_workspace: Option<WeakEntity<MultiWorkspace>>) -> Self {
        Self { multi_workspace }
    }
}

impl StatusItemView for ThreadsSidebarButton {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _cx: &App) -> Option<HideStatusItem> {
        // Deliberately none. `HideStatusItem` writes a legacy per-item show/hide
        // key, and this item never had one — it is hidden through its component's
        // `hidden` flag, which the layout surface already offers and which
        // `resolved_component_zone` already honours.
        None
    }
}

impl Render for ThreadsSidebarButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = SidebarStatus::query(
            crate::layout_registry::ids::THREADS_SIDEBAR,
            &self.multi_workspace,
            cx,
        );
        let on_right = sidebar.side == SidebarSide::Right;
        let has_notifications = sidebar.has_notifications;
        let indicator_border = cx.theme().colors().status_bar_background;

        let toggle = sidebar_side_context_menu("sidebar-status-toggle-menu", cx)
            .anchor(if on_right {
                Anchor::BottomRight
            } else {
                Anchor::BottomLeft
            })
            .attach(if on_right {
                Anchor::TopRight
            } else {
                Anchor::TopLeft
            })
            .trigger(move |_is_active, _window, _cx| {
                IconButton::new(
                    "toggle-workspace-sidebar",
                    match (on_right, sidebar.open) {
                        (true, true) => IconName::ThreadsSidebarRightOpen,
                        (true, false) => IconName::ThreadsSidebarRightClosed,
                        (false, true) => IconName::ThreadsSidebarLeftOpen,
                        (false, false) => IconName::ThreadsSidebarLeftClosed,
                    },
                )
                .icon_size(IconSize::Small)
                .tab_index(0isize)
                // The glyph already swaps open/closed, but that reads as a different
                // picture rather than a state. Now that this and Workspaces are a
                // matched pair of exclusive on/off switches, they have to highlight
                // the same way every other bar item does, or only one of the two
                // looks like it is on.
                .toggle_state(sidebar.open)
                .aria_label(if sidebar.open {
                    "Close threads sidebar"
                } else {
                    "Open threads sidebar"
                })
                .when(has_notifications, |this| {
                    this.indicator(Indicator::dot().color(Color::Accent))
                        .indicator_border_color(Some(indicator_border))
                })
                .tooltip(move |_, cx| {
                    Tooltip::for_action(
                        if sidebar.open {
                            "Close Threads Sidebar"
                        } else {
                            "Open Threads Sidebar"
                        },
                        &ToggleWorkspaceSidebar,
                        cx,
                    )
                })
                .on_click(move |_, window, cx| {
                    if let Some(multi_workspace) = window.root::<MultiWorkspace>().flatten() {
                        multi_workspace.update(cx, |multi_workspace, cx| {
                            multi_workspace.toggle_sidebar_occupant(
                                crate::layout_registry::ids::THREADS_SIDEBAR,
                                window,
                                cx,
                            );
                        });
                    }
                })
            });

        h_flex().gap_0p5().child(toggle)
    }
}

/// The bottom-bar icon for the workspace tab strip, the sidebar slot's other
/// occupant.
///
/// It mirrors `ThreadsSidebarButton` rather than sharing a body with it: the two
/// differ in icon set, label, and the side each one reads, and folding them into
/// one parameterised button would trade four small differences for a type that
/// answers "which occupant am I" at every line.
pub struct WorkspacesButton {
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
}

impl WorkspacesButton {
    pub fn new(multi_workspace: Option<WeakEntity<MultiWorkspace>>) -> Self {
        Self { multi_workspace }
    }
}

impl StatusItemView for WorkspacesButton {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _cx: &App) -> Option<HideStatusItem> {
        // None, for the same reason the threads sidebar has none: this item never
        // had a legacy per-item show/hide key, so there is nothing for
        // `HideStatusItem` to write. It hides through its component's `hidden` flag.
        None
    }
}

impl Render for WorkspacesButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = SidebarStatus::query(
            crate::layout_registry::ids::WORKSPACES,
            &self.multi_workspace,
            cx,
        );
        let open = sidebar.open;

        h_flex().gap_0p5().child(
            IconButton::new("toggle-workspaces-sidebar", IconName::ThisWindow)
                .icon_size(IconSize::Small)
                .tab_index(0isize)
                .toggle_state(open)
                .aria_label(if open {
                    "Close workspaces"
                } else {
                    "Open workspaces"
                })
                .tooltip(move |_, cx| {
                    Tooltip::simple(
                        if open {
                            "Close Workspaces"
                        } else {
                            "Open Workspaces"
                        },
                        cx,
                    )
                })
                .on_click(move |_, window, cx| {
                    if let Some(multi_workspace) = window.root::<MultiWorkspace>().flatten() {
                        multi_workspace.update(cx, |multi_workspace, cx| {
                            multi_workspace.toggle_sidebar_occupant(
                                crate::layout_registry::ids::WORKSPACES,
                                window,
                                cx,
                            );
                        });
                    }
                }),
        )
    }
}

/// Wraps an item in the right-click menu that fits it.
///
/// A registered status control gets `Bottom Bar Placement` — the gesture that
/// makes placement a feature rather than a settings-file edit. Anything else
/// keeps the older `Hide Button`, which is all the status bar knows how to
/// offer an item it has no registry row for.
fn render_status_bar_item(
    side: &'static str,
    index: usize,
    entry: &StatusBarItem,
    resolved_location: StatusBarItemLocation,
    strip: &[String],
    full: &[String],
    focus_handle: &FocusHandle,
    cx: &App,
) -> AnyElement {
    let view = entry.item.to_any();
    let menu_id: SharedString = format!("{side}-item-menu-{index}").into();

    if let Some(adapter) = &entry.adapter {
        let item_id = adapter.id();
        let focus_handle = focus_handle.clone();
        let menu = right_click_menu(menu_id)
            .trigger(move |_is_active, _window, _cx| view)
            .menu(move |window, cx| {
                let focus_handle = focus_handle.clone();
                ContextMenu::build(window, cx, move |menu, _window, _cx| {
                    add_status_control_placement_entries(
                        menu,
                        item_id,
                        resolved_location,
                        focus_handle,
                    )
                })
            });

        // Arranging a zone is the gesture a menu cannot carry: it closes after every
        // move, so a three-place move is three right-clicks. Dogfooded on components
        // 2026-09-06 and that is exactly what it cost, so the controls never shipped
        // the menu version.
        let Some(at) = strip.iter().position(|id| id == item_id) else {
            return menu.into_any_element();
        };
        let registered = crate::layout_registry::get(item_id);
        let strip = strip.to_vec();
        let full = full.to_vec();
        return div()
            .id(("status-control", index))
            .child(menu)
            .on_drag(
                DraggedStatusControl {
                    item_id: item_id.into(),
                    label: registered.map(|item| item.label).unwrap_or(item_id).into(),
                    icon: registered.and_then(|item| item.icon),
                    from: resolved_location,
                    at,
                },
                |dragged, _, _, cx| cx.new(|_| dragged.clone()),
            )
            .drag_over::<DraggedStatusControl>(move |style, dragged, _, cx| {
                if !control_drop_would_land(dragged, resolved_location, Some(item_id)) {
                    return style;
                }
                let style = style
                    .border_color(cx.theme().colors().drop_target_border)
                    .border_0();
                // Mark the edge the control would arrive from. One from another zone
                // takes this one's place and pushes it along, which is the same as
                // arriving from the left.
                if dragged.from != resolved_location || at < dragged.at {
                    style.border_l_2()
                } else {
                    style.border_r_2()
                }
            })
            .on_drop(move |dragged: &DraggedStatusControl, window, cx| {
                apply_control_drop(
                    dragged,
                    Some(item_id),
                    resolved_location,
                    &strip,
                    &full,
                    window,
                    cx,
                );
            })
            .into_any_element();
    }

    let Some(hide) = entry.item.hide_setting(cx) else {
        return view.into_any_element();
    };

    right_click_menu(menu_id)
        .trigger(move |_is_active, _window, _cx| view)
        .menu(move |window, cx| {
            let hide = hide.clone();
            ContextMenu::build(window, cx, move |menu, _window, _cx| {
                add_hide_button_entry(menu, hide)
            })
        })
        .into_any_element()
}

/// The four zones a status control can sit in, as menu entries.
///
/// `Hidden` is one of them rather than a separate "Hide Button": hiding is a
/// placement in this model, and offering both gestures would write two settings
/// keys for one visible outcome. Its checkmark is unreachable in practice — an
/// item resolving to `Hidden` renders in no zone, so there is nothing left to
/// right-click. Getting it back is the layout surface's job.
fn add_status_control_placement_entries(
    mut menu: ContextMenu,
    item_id: &'static str,
    resolved_location: StatusBarItemLocation,
    focus_handle: FocusHandle,
) -> ContextMenu {
    // Action entries focus this handle before they dispatch, and
    // `CustomizeWorkspaceLayout` is registered on `Workspace`. The status bar is
    // inside the workspace's element tree, so its own handle puts the registered
    // handler above the dispatch; without it the entry would resolve to nothing.
    menu = menu.context(focus_handle);
    menu = menu.header("Bottom Bar Placement");
    for (location, label) in [
        (StatusBarItemLocation::Left, "Left"),
        (StatusBarItemLocation::Middle, "Middle"),
        (StatusBarItemLocation::Right, "Right"),
        (StatusBarItemLocation::Hidden, "Hidden"),
    ] {
        menu = menu.toggleable_entry(
            label,
            location == resolved_location,
            IconPosition::Start,
            None,
            move |window, cx| set_status_control_location(item_id, location, window, cx),
        );
    }
    // The way back from `Hidden`. An item placed `Hidden` renders in no zone, so
    // this menu disappears along with it — the exit has to be offered while the
    // item is still there to right-click. Carries `item_id` so the surface opens
    // on this control's row rather than at the top of twenty-one.
    menu = menu.separator().action(
        "Customize Workspace Layout…",
        Box::new(CustomizeWorkspaceLayout {
            item_id: Some(item_id.to_string()),
        }),
    );
    menu
}

/// The whole settings mutation a placement makes.
///
/// Split out of the write so it can be asserted: this is the half that decides
/// which keys a placement gesture touches, and the half a future port could
/// quietly widen.
fn place_status_control(
    settings: &mut SettingsContent,
    item_id: &str,
    location: StatusBarItemLocation,
) {
    settings
        .status_bar
        .get_or_insert_default()
        .item_locations
        .get_or_insert_default()
        .insert(item_id.to_string(), location);
}

/// Persists a status control's placement.
///
/// Only `status_bar.item_locations` is written. An explicit placement already
/// outranks the item's own legacy boolean in
/// `StatusBarSettings::resolved_status_control_location`, so writing that
/// boolean too would leave two settings describing one visible state — and it
/// would silently edit keys the user never touched.
///
/// The write is completion-aware and goes through the same reporter a component
/// write does: the old placement stays on screen until it lands, a failure names
/// the item rather than the settings key, focus returns to the surface that made
/// the choice, and one gesture raises one dialog however many writes it queued.
pub fn set_status_control_location(
    item_id: &'static str,
    location: StatusBarItemLocation,
    window: &mut Window,
    cx: &mut App,
) {
    let fs = <dyn fs::Fs>::global(cx);
    let update = update_settings_file_with_completion(fs, cx, move |settings, _cx| {
        place_status_control(settings, item_id, location);
    });
    let message = crate::dock::placement_failure_message(
        item_id,
        match location {
            StatusBarItemLocation::Hidden => "hide",
            _ => "move",
        },
    );
    crate::dock::report_layout_write(update, message, window, cx);
}

/// The one slot a drop cannot otherwise name.
///
/// Dropping onto a control means taking its place, so it always lands to that
/// control's left, leaving the position past the last one unreachable from another
/// zone. Found on the component side 2026-09-06 by Tom trying exactly that drop; a
/// zone of N controls has N+1 slots and dropping-onto names only N.
///
/// Absent when the zone draws no controls: there is nothing to go after, and nothing
/// drawn to aim at either.
fn zone_end_drop_target(
    zone: StatusBarItemLocation,
    strip: &[String],
    full: &[String],
) -> Option<AnyElement> {
    if strip.is_empty() {
        return None;
    }
    let id: &'static str = match zone {
        StatusBarItemLocation::Left => "status-zone-end-left",
        StatusBarItemLocation::Middle => "status-zone-end-middle",
        StatusBarItemLocation::Right => "status-zone-end-right",
        StatusBarItemLocation::Hidden => return None,
    };
    let strip = strip.to_vec();
    let full = full.to_vec();
    Some(
        div()
            .id(id)
            .w(px(12.))
            .h(px(18.))
            .border_0()
            .drag_over::<DraggedStatusControl>(move |style, dragged, _, cx| {
                if !control_drop_would_land(dragged, zone, None) {
                    return style;
                }
                style
                    .border_color(cx.theme().colors().drop_target_border)
                    .border_l_2()
            })
            .on_drop(move |dragged: &DraggedStatusControl, window, cx| {
                apply_control_drop(dragged, None, zone, &strip, &full, window, cx);
            })
            .into_any_element(),
    )
}

/// A status control being dragged along its zone to a new place in the sequence.
///
/// Carries the zone it left so a drop can re-place it when it lands in another one,
/// and its registry label so the drag preview reads as the item — a control renders
/// as text as often as an icon, and the live view cannot be drawn twice.
#[derive(Clone)]
pub struct DraggedStatusControl {
    item_id: SharedString,
    label: SharedString,
    icon: Option<IconName>,
    from: StatusBarItemLocation,
    at: usize,
}

impl Render for DraggedStatusControl {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .px_1()
            .rounded_sm()
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(IconSize::Small))
            })
            .child(Label::new(self.label.clone()).size(LabelSize::Small))
    }
}

/// Whether a drop on this zone would change anything, which is also whether it
/// should promise to. `target` is the control under the pointer, or `None` for the
/// end-of-zone zone, which always has somewhere to put a control that is not
/// already last.
///
/// A panel icon is never a target: the strips share these zones and are always in
/// front of the controls, so there is no slot on their side of the boundary.
fn control_drop_would_land(
    dragged: &DraggedStatusControl,
    onto: StatusBarItemLocation,
    target: Option<&str>,
) -> bool {
    if onto == StatusBarItemLocation::Hidden {
        return false;
    }
    if dragged.from != onto {
        return true;
    }
    match target {
        Some(id) => id != dragged.item_id.as_ref(),
        None => true,
    }
}

/// Applies a dropped status control to this zone: the zone it joins, if it is
/// joining, and where it lands once there.
///
/// Shared by the controls and the end-of-zone target because they differ in one
/// thing — a control means *take this one's place*, the end means *go last*.
fn apply_control_drop(
    dragged: &DraggedStatusControl,
    target: Option<&str>,
    onto: StatusBarItemLocation,
    strip: &[String],
    full: &[String],
    window: &mut Window,
    cx: &mut App,
) {
    use crate::sequence_order::{LandsOn, order_after_joining, order_after_placing};

    if !control_drop_would_land(dragged, onto, target) {
        return;
    }
    let joining = dragged.from != onto;
    let reversed = onto == StatusBarItemLocation::Right;

    let (target, lands) = match target {
        Some(id) => (id.to_string(), LandsOn::Before),
        // An empty zone has no control to go after, and nothing drawn to aim at
        // either — the placement entries cover it.
        None => match strip.last() {
            Some(last) => (last.clone(), LandsOn::After),
            None => return,
        },
    };
    let sequence = if joining {
        order_after_joining(full, reversed, &dragged.item_id, &target, lands)
    } else {
        order_after_placing(full, strip, reversed, &dragged.item_id, &target)
    };
    let Some(sequence) = sequence else {
        return;
    };

    // The zone first, then where it lands in it. Both ride the one serialized queue,
    // so the order the control arrives in is the order written here.
    if joining && let Some(id) = crate::layout_registry::get(&dragged.item_id).map(|item| item.id) {
        set_status_control_location(id, onto, window, cx);
    }
    set_status_control_order(&dragged.item_id, sequence, window, cx);
}

/// Persists the whole status-control sequence, reporting a failure the same way
/// every other placement write does.
pub fn set_status_control_order(
    item_id: &str,
    order: Vec<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let fs = <dyn fs::Fs>::global(cx);
    let message = crate::dock::placement_failure_message(item_id, "move");
    let update = update_settings_file_with_completion(fs, cx, move |settings, _cx| {
        settings.status_bar.get_or_insert_default().item_order = Some(order);
    });
    crate::dock::report_layout_write(update, message, window, cx);
}

/// Every registered status control, in the order the bar currently reads them.
///
/// The first move writes the whole sequence, for the reason the component list does:
/// it makes every later move a plain list splice, and a control whose crate loads
/// after the write cannot jump, because it is already named in the place the table
/// gave it. Ids the user's list names that the table does not know are kept.
fn materialized_control_order(order: &[String]) -> Vec<String> {
    use crate::layout_registry::{LayoutItemKind, of_kind};

    let mut ids: Vec<String> = of_kind(LayoutItemKind::StatusControl)
        .map(|item| item.id.to_string())
        .collect();
    for id in order {
        if !ids.iter().any(|known| known == id) {
            ids.push(id.clone());
        }
    }
    ids.sort_by_key(|id| zone_rank(Some(id), 0, order));
    ids
}

/// Which item the zone's end drop target is drawn after: the last **control** in
/// drawn order, not the last item in the zone.
///
/// The dock button strips share these zones and sort to band 0, so the Right zone —
/// ranked, then reversed — draws them *after* every control. Appending the target to
/// the end of the zone therefore put it on the far side of those icons from the slot
/// a drop there writes, which is after `zone_control_strip`'s last entry. The Left
/// zone draws the strips first, so the two positions coincide there and the defect
/// was invisible on the only side anyone checked.
///
/// `None` when the zone draws no control — there is no end of a sequence that has no
/// members, which is the same condition `zone_end_drop_target` refuses on.
fn zone_end_target_follows(zone_indices: &[usize], ids: &[Option<&'static str>]) -> Option<usize> {
    zone_indices.iter().rev().copied().find(|index| {
        ids.get(*index).copied().flatten().is_some_and(|id| {
            matches!(
                crate::layout_registry::get(id).map(|item| item.kind),
                Some(crate::layout_registry::LayoutItemKind::StatusControl)
            )
        })
    })
}

/// The registered controls drawn in `zone`, in the order they are drawn.
///
/// Only controls: the dock button strips share these zones and are always in front
/// of them, so they are not part of the sequence a nudge moves an item along.
fn zone_control_strip(
    locations: &[StatusBarItemLocation],
    ids: &[Option<&'static str>],
    order: &[String],
    zone: StatusBarItemLocation,
) -> Vec<String> {
    zone_order(locations, ids, order, zone)
        .into_iter()
        .filter_map(|index| ids.get(index).copied().flatten())
        .filter(|id| {
            matches!(
                crate::layout_registry::get(id).map(|item| item.kind),
                Some(crate::layout_registry::LayoutItemKind::StatusControl)
            )
        })
        .map(|id| id.to_string())
        .collect()
}

/// Appends a "Hide Button" entry aligned with surrounding toggleable entries.
pub fn add_hide_button_entry(menu: ContextMenu, hide: HideStatusItem) -> ContextMenu {
    menu.toggleable_entry(
        "Hide Button",
        false,
        IconPosition::Start,
        None,
        move |_window, cx| hide.apply(cx),
    )
}

impl StatusBar {
    pub fn new(
        active_pane: &Entity<Pane>,
        multi_workspace: Option<WeakEntity<MultiWorkspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            items: Default::default(),
            active_pane: active_pane.clone(),
            multi_workspace,
            focus_handle: cx.focus_handle(),
            _observe_active_pane: cx.observe_in(active_pane, window, |this, _, window, cx| {
                this.update_active_pane_item(window, cx)
            }),
        };
        this.update_active_pane_item(window, cx);
        this
    }

    pub fn set_multi_workspace(
        &mut self,
        multi_workspace: WeakEntity<MultiWorkspace>,
        cx: &mut Context<Self>,
    ) {
        self.multi_workspace = Some(multi_workspace);
        cx.notify();
    }

    pub fn add_left_item<T>(&mut self, item: Entity<T>, window: &mut Window, cx: &mut Context<Self>)
    where
        T: 'static + StatusItemView,
    {
        self.push_item(item, None, StatusBarItemLocation::Left, window, cx);
    }

    pub fn add_right_item<T>(
        &mut self,
        item: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        T: 'static + StatusItemView,
    {
        self.push_item(item, None, StatusBarItemLocation::Right, window, cx);
    }

    /// Adds a status control the user can place. Its default zone comes from the
    /// registry, so a call site cannot disagree with the canonical table about
    /// where an item belongs.
    pub fn add_status_control<T>(
        &mut self,
        item: Entity<T>,
        adapter: StatusControlAdapter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        T: 'static + StatusItemView,
    {
        if self
            .items
            .iter()
            .filter_map(|existing| existing.adapter.as_ref())
            .any(|existing| existing.id == adapter.id)
        {
            log::error!("duplicate status bar item id: {}", adapter.id);
        }

        let default_location = StatusBarSettings::get_global(cx)
            .status_control_location(adapter.id)
            .unwrap_or(StatusBarItemLocation::Right);
        self.push_item(item, Some(adapter), default_location, window, cx);
    }

    fn push_item<T>(
        &mut self,
        item: Entity<T>,
        adapter: Option<StatusControlAdapter>,
        default_location: StatusBarItemLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        T: 'static + StatusItemView,
    {
        let active_pane_item = self.active_pane.read(cx).active_item();
        item.set_active_pane_item(active_pane_item.as_deref(), window, cx);

        self.items.push(StatusBarItem {
            item: Box::new(item),
            adapter,
            default_location,
        });
        cx.notify();
    }

    /// Where a registered status control actually renders, `Hidden` included.
    ///
    /// The bar resolves this through each item's `StatusControlAdapter`, whose
    /// legacy-setting closure is bound where the bar is assembled and is reachable
    /// nowhere else. Anything that needs to *display* a control's placement — the
    /// layout surface — must ask here rather than resolve it again, or the two
    /// answers drift: `status_control_location` deliberately never returns
    /// `Hidden`, so a re-derived answer names a zone for an item that is not in
    /// the bar at all.
    pub fn effective_location_for(&self, item_id: &str, cx: &App) -> Option<StatusBarItemLocation> {
        self.items
            .iter()
            .find(|entry| {
                entry
                    .adapter
                    .as_ref()
                    .is_some_and(|adapter| adapter.id() == item_id)
            })
            .map(|entry| entry.effective_location(cx))
    }

    pub fn item_of_type<T: StatusItemView>(&self) -> Option<Entity<T>> {
        self.items
            .iter()
            .find_map(|entry| entry.item.to_any().downcast().ok())
    }

    pub fn position_of_item<T>(&self) -> Option<usize>
    where
        T: StatusItemView,
    {
        self.items
            .iter()
            .position(|entry| entry.item.item_type() == TypeId::of::<T>())
    }

    pub fn insert_item_after<T>(
        &mut self,
        position: usize,
        item: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        T: 'static + StatusItemView,
    {
        let active_pane_item = self.active_pane.read(cx).active_item();
        item.set_active_pane_item(active_pane_item.as_deref(), window, cx);

        let default_location = self
            .items
            .get(position)
            .map(|entry| entry.default_location)
            .unwrap_or(StatusBarItemLocation::Right);
        let index = (position + 1).min(self.items.len());
        self.items.insert(
            index,
            StatusBarItem {
                item: Box::new(item),
                adapter: None,
                default_location,
            },
        );
        cx.notify()
    }

    pub fn remove_item_at(&mut self, position: usize, cx: &mut Context<Self>) {
        if position < self.items.len() {
            self.items.remove(position);
            cx.notify();
        }
    }

    pub fn set_active_pane(
        &mut self,
        active_pane: &Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_pane = active_pane.clone();
        self._observe_active_pane = cx.observe_in(active_pane, window, |this, _, window, cx| {
            this.update_active_pane_item(window, cx)
        });
        self.update_active_pane_item(window, cx);
    }

    fn update_active_pane_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_pane_item = self.active_pane.read(cx).active_item();
        for entry in &self.items {
            entry
                .item
                .set_active_pane_item(active_pane_item.as_deref(), window, cx);
        }
    }

    /// Moves focus between the interactive controls within the status bar in
    /// response to arrow keys. Navigation is clamped to the status bar so
    /// arrows move between items and stop at the ends (ARIA toolbar semantics);
    /// Tab is still used to leave the toolbar.
    fn move_item_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let previous = window.focused(cx);
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
        let landed_in_status_bar = window
            .focused(cx)
            .is_some_and(|handle| self.focus_handle.contains(&handle, window));
        if !landed_in_status_bar && let Some(previous) = previous {
            window.focus(&previous, cx);
        }
        cx.notify();
    }
}

impl<T: StatusItemView> StatusItemViewHandle for Entity<T> {
    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn set_active_pane_item(
        &self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.update(cx, |this, cx| {
            this.set_active_pane_item(active_pane_item, window, cx)
        });
    }

    fn item_type(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn hide_setting(&self, cx: &App) -> Option<HideStatusItem> {
        self.read(cx).hide_setting(cx)
    }
}

impl From<&dyn StatusItemViewHandle> for AnyView {
    fn from(val: &dyn StatusItemViewHandle) -> Self {
        val.to_any()
    }
}

#[cfg(test)]
mod placement_menu_tests {
    use super::*;
    use crate::layout_registry::ids;
    use StatusBarItemLocation::{Hidden, Left, Middle, Right};
    use collections::HashMap;

    fn placed(item_id: &str, location: StatusBarItemLocation) -> SettingsContent {
        let mut settings = SettingsContent::default();
        place_status_control(&mut settings, item_id, location);
        settings
    }

    fn locations(settings: &SettingsContent) -> &HashMap<String, StatusBarItemLocation> {
        settings
            .status_bar
            .as_ref()
            .and_then(|status_bar| status_bar.item_locations.as_ref())
            .expect("a placement wrote no item_locations map")
    }

    #[test]
    fn placing_a_control_records_the_zone_it_was_given() {
        for location in [Left, Middle, Right, Hidden] {
            let settings = placed(ids::CURSOR_POSITION, location);
            assert_eq!(
                locations(&settings).get(ids::CURSOR_POSITION),
                Some(&location),
                "placing at {location:?} did not round-trip"
            );
        }
    }

    #[test]
    fn placing_a_control_leaves_its_legacy_setting_alone() {
        // The prior art wrote `Some(true)` back into each item's own legacy
        // boolean whenever it was placed. That is unnecessary here — an explicit
        // placement already outranks the boolean in
        // `resolved_status_control_location` — and it would leave two settings
        // describing one visible state while editing keys the user never named.
        // This is the witness for that deliberate omission, because a later port
        // of the same file is exactly how it would come back.
        let settings = placed(ids::CURSOR_POSITION, Left);
        let status_bar = settings.status_bar.as_ref().expect("no status_bar written");
        assert_eq!(
            status_bar.cursor_position_button, None,
            "placing a control rewrote its legacy visibility boolean"
        );
        assert_eq!(status_bar.show_active_file, None);
        assert_eq!(status_bar.active_language_button, None);
        assert_eq!(status_bar.line_endings_button, None);
        assert_eq!(status_bar.active_encoding_button, None);
    }

    #[test]
    fn placing_one_control_does_not_move_another() {
        let mut settings = placed(ids::CURSOR_POSITION, Left);
        place_status_control(&mut settings, ids::ACTIVE_LANGUAGE, Middle);

        assert_eq!(locations(&settings).get(ids::CURSOR_POSITION), Some(&Left));
        assert_eq!(
            locations(&settings).get(ids::ACTIVE_LANGUAGE),
            Some(&Middle)
        );
        assert_eq!(locations(&settings).len(), 2);
    }

    #[test]
    fn replacing_a_placement_overwrites_rather_than_accumulates() {
        // Every menu choice writes the same key. If it appended instead, the
        // second choice would be shadowed by the first and the menu would look
        // dead after one use.
        let mut settings = placed(ids::CURSOR_POSITION, Left);
        place_status_control(&mut settings, ids::CURSOR_POSITION, Right);

        assert_eq!(locations(&settings).get(ids::CURSOR_POSITION), Some(&Right));
        assert_eq!(locations(&settings).len(), 1);
    }
}

#[cfg(test)]
mod zone_tests {
    use super::*;
    use StatusBarItemLocation::{Hidden, Left, Middle, Right};

    /// These assertions are about zone membership and the Right reversal, not about
    /// ranking, so every item here is unregistered — which makes `zone_rank` fall to
    /// the registration index and reproduces the order they were written against.
    fn unregistered(locations: &[StatusBarItemLocation]) -> Vec<Option<&'static str>> {
        vec![None; locations.len()]
    }

    const ZONES: [StatusBarItemLocation; 3] = [Left, Middle, Right];

    #[test]
    fn a_zone_takes_only_its_own_items_and_keeps_their_order() {
        let locations = [Left, Right, Left, Middle, Left];
        assert_eq!(
            zone_order(&locations, &unregistered(&locations), &[], Left),
            vec![0, 2, 4]
        );
        assert_eq!(
            zone_order(&locations, &unregistered(&locations), &[], Middle),
            vec![3]
        );
    }

    #[test]
    fn the_right_zone_renders_reversed() {
        // The item added first ends up furthest from the editor. Two separate
        // lists used to encode this by iterating one of them backwards; with one
        // list it has to be stated.
        let locations = [Right, Right, Right];
        assert_eq!(
            zone_order(&locations, &unregistered(&locations), &[], Right),
            vec![2, 1, 0]
        );
        assert_ne!(
            zone_order(&locations, &unregistered(&locations), &[], Right),
            vec![0, 1, 2],
            "source order and render order are indistinguishable here, so this \
             fixture cannot tell a reversal from its absence"
        );
    }

    #[test]
    fn only_the_right_zone_reverses() {
        let locations = [Left, Left, Middle, Middle];
        assert_eq!(
            zone_order(&locations, &unregistered(&locations), &[], Left),
            vec![0, 1]
        );
        assert_eq!(
            zone_order(&locations, &unregistered(&locations), &[], Middle),
            vec![2, 3]
        );
    }

    #[test]
    fn a_hidden_item_is_returned_by_no_zone() {
        // Hiding is this filter. An item resolving to Hidden must match nothing,
        // including a query for Hidden itself — there is no Hidden renderer.
        let locations = [Hidden, Left, Hidden];
        for zone in ZONES {
            assert!(
                !zone_order(&locations, &unregistered(&locations), &[], zone).contains(&0),
                "a hidden item reached the {zone:?} zone"
            );
            assert!(!zone_order(&locations, &unregistered(&locations), &[], zone).contains(&2));
        }
        assert!(
            zone_order(&locations, &unregistered(&locations), &[], Hidden).is_empty(),
            "asking for the Hidden zone returned items to render"
        );
        assert_eq!(
            zone_order(&locations, &unregistered(&locations), &[], Left),
            vec![1]
        );
    }

    #[test]
    fn every_item_renders_in_exactly_one_zone_or_none() {
        // The partition property. A duplicate here is an item drawn twice — the
        // `both` failure the dogfood watched for — and a gap is an item that
        // silently disappears while its setting says it should be somewhere.
        let locations = [Left, Right, Middle, Hidden, Right, Left, Middle];

        for (index, location) in locations.iter().enumerate() {
            let appearances = ZONES
                .iter()
                .filter(|zone| {
                    zone_order(&locations, &unregistered(&locations), &[], **zone).contains(&index)
                })
                .count();
            let expected = if *location == Hidden { 0 } else { 1 };
            assert_eq!(
                appearances, expected,
                "item {index} at {location:?} rendered in {appearances} zones"
            );
        }

        let total: usize = ZONES
            .iter()
            .map(|zone| zone_order(&locations, &unregistered(&locations), &[], *zone).len())
            .sum();
        assert_eq!(
            total,
            locations.iter().filter(|l| **l != Hidden).count(),
            "the zones together render a different number of items than are placed"
        );
    }
    /// The registry decides where a status control sits in its zone, and until
    /// 2026-09-06 it did not: `zone_order` sorted by registration index while
    /// `SourceRank::StatusControl(n)`'s number was read by nothing. Two sequences
    /// for one order, and the live one was a call order in the `zed` crate — which
    /// is why status-control ordering had nothing to write to. Exactly the shape
    /// panels had before 2026-09-05.
    #[test]
    fn a_registered_control_takes_its_place_from_the_table() {
        use crate::layout_registry::{SourceRank, get, ids};

        let SourceRank::StatusControl(cursor) = get(ids::CURSOR_POSITION).unwrap().source_rank
        else {
            panic!("Cursor Position must be ranked as a status control");
        };
        assert_eq!(
            super::zone_rank(Some(ids::CURSOR_POSITION), 999, &[]),
            (2, cursor as u32),
            "a wildly wrong registration index changes nothing for a registered id"
        );
        assert!(
            super::zone_rank(Some(ids::PROJECT_SEARCH), 999, &[])
                < super::zone_rank(Some(ids::CURSOR_POSITION), 0, &[]),
            "and the table's order wins over the order they happened to be added in"
        );
    }

    /// The dock button strips carry no adapter and are added in `Workspace::new`,
    /// before any status control exists — so they already sit ahead of every control
    /// in their zone, and the switch has to leave them there. Sorting the
    /// unregistered behind would have moved the panel icons, which is the one thing
    /// a no-op change must not do.
    #[test]
    fn an_item_the_table_does_not_know_keeps_its_place_ahead_of_every_control() {
        use crate::layout_registry::ids;

        assert_eq!(super::zone_rank(None, 7, &[]), (0, 7));
        assert!(
            super::zone_rank(None, 999, &[]) < super::zone_rank(Some(ids::PROJECT_SEARCH), 0, &[]),
            "a dock button strip stays ahead of the first-ranked control"
        );
    }

    /// The Right zone is the whole point: it ranks then reverses, so the dock button
    /// strips it shares with band 0 draw *after* every control. The end drop target
    /// has to follow the last control, which is where a drop on it writes — not the
    /// last item drawn, which is an icon strip several positions further right.
    #[test]
    fn the_end_drop_target_follows_the_last_control_not_the_last_item() {
        use crate::layout_registry::ids;

        // Index 0 is a dock button strip: no id, so band 0.
        let ids: Vec<Option<&'static str>> =
            vec![None, Some(ids::CURSOR_POSITION), Some(ids::ACTIVE_LANGUAGE)];
        let locations = [Right, Right, Right];

        let drawn = zone_order(&locations, &ids, &[], Right);
        assert_eq!(
            drawn.last().copied(),
            Some(0),
            "the strip draws last in the Right zone — the precondition this is about"
        );
        assert_eq!(
            super::zone_end_target_follows(&drawn, &ids),
            Some(drawn[drawn.len() - 2]),
            "so the target follows the control before it, not the strip"
        );
        assert_ne!(
            super::zone_end_target_follows(&drawn, &ids),
            drawn.last().copied(),
            "appending to the end of the zone is the defect, not the fix"
        );

        // Left draws the strip first, so both readings coincide — which is exactly
        // why checking only this side reported the Right zone as correct.
        let locations = [Left, Left, Left];
        let drawn = zone_order(&locations, &ids, &[], Left);
        assert_eq!(
            super::zone_end_target_follows(&drawn, &ids),
            drawn.last().copied(),
            "the two positions agree on the left, and agreeing proves nothing here"
        );
    }

    #[test]
    fn a_zone_with_no_control_has_no_end_target_to_place() {
        let ids: Vec<Option<&'static str>> = vec![None, None];
        let locations = [Right, Right];
        let drawn = zone_order(&locations, &ids, &[], Right);
        assert_eq!(
            super::zone_end_target_follows(&drawn, &ids),
            None,
            "strips alone are not a sequence, so there is no end of one to aim at"
        );
    }

    /// The empty list is every install's starting state and most installs' only
    /// state, so it has to reproduce the table exactly rather than merely closely.
    #[test]
    fn an_empty_list_reproduces_the_control_table_exactly() {
        use crate::layout_registry::{LayoutItemKind, of_kind};

        let expected: Vec<String> = of_kind(LayoutItemKind::StatusControl)
            .map(|item| item.id.to_string())
            .collect();
        assert_eq!(super::materialized_control_order(&[]), expected);
        assert_eq!(expected.len(), 14, "the table's control count changed");
    }

    /// The first move writes the whole sequence, so a control whose crate loads
    /// after the write cannot jump — it is already named, in the place the table
    /// gave it. A stale or mistyped id is kept and obeyed rather than dropped,
    /// which would quietly rewrite a user's file.
    #[test]
    fn the_written_sequence_leads_with_what_the_user_listed() {
        use crate::layout_registry::ids;

        let order = vec![ids::CURSOR_POSITION.to_string(), "Ghost".to_string()];
        let sequence = super::materialized_control_order(&order);

        assert_eq!(
            sequence.first().map(String::as_str),
            Some(ids::CURSOR_POSITION),
            "the listed control leads, though the table ranks it near-last"
        );
        assert_eq!(sequence.get(1).map(String::as_str), Some("Ghost"));
        assert_eq!(sequence.len(), 15, "and nothing was dropped to make room");
    }

    /// A nudge moves an item along the controls it can see, and the dock button
    /// strips are not among them — they share the zone and always sit in front.
    /// The strip also comes back **as drawn**, so the Right zone's reversal is
    /// handled here rather than by every caller.
    #[test]
    fn the_zone_strip_is_the_controls_as_drawn_without_the_dock_strips() {
        use crate::layout_registry::ids;
        use StatusBarItemLocation::Right;

        let locations = vec![Right, Right, Right];
        let ids = vec![None, Some(ids::CURSOR_POSITION), Some(ids::VIM_MODE)];

        assert_eq!(
            super::zone_control_strip(&locations, &ids, &[], Right),
            vec![ids::CURSOR_POSITION.to_string(), ids::VIM_MODE.to_string()],
            "the strip drops the dock buttons and reads right-to-left like the zone"
        );
    }

    /// A dock strip carries no id and drops out for free. A **panel** id does not:
    /// `StatusControlAdapter::new` takes any `&'static str`, so a call site can
    /// register a component id by mistake, and it would arrive here as `Some`.
    /// `zone_rank` already sorts it with the strips; without the kind filter it
    /// would still be a neighbour a nudge could move a control past.
    #[test]
    fn a_panel_id_registered_as_a_control_is_not_a_neighbour() {
        use crate::layout_registry::ids;
        use StatusBarItemLocation::Left;

        let locations = vec![Left, Left];
        let ids = vec![Some(ids::PROJECT_PANEL), Some(ids::CURSOR_POSITION)];

        assert_eq!(
            super::zone_control_strip(&locations, &ids, &[], Left),
            vec![ids::CURSOR_POSITION.to_string()],
            "only real status controls are in the sequence a nudge moves along"
        );
    }

    /// The user's list is the top band among controls, so naming one moves it ahead
    /// of every control the table ranks — including the one the table ranks first.
    #[test]
    fn a_listed_control_outranks_every_unlisted_one() {
        use crate::layout_registry::ids;

        let order = vec![ids::CURSOR_POSITION.to_string()];
        assert_eq!(
            super::zone_rank(Some(ids::CURSOR_POSITION), 0, &order),
            (1, 0)
        );
        assert!(
            super::zone_rank(Some(ids::CURSOR_POSITION), 0, &order)
                < super::zone_rank(Some(ids::PROJECT_SEARCH), 0, &order),
            "Cursor Position is ranked last in the table and first in the list; the \
             list wins"
        );
    }

    /// A partial list is meaningful on its own: naming two controls orders those two
    /// and leaves every other one where the table had it.
    #[test]
    fn a_partial_list_disturbs_only_the_controls_it_names() {
        use crate::layout_registry::ids;

        let order = vec![ids::VIM_MODE.to_string(), ids::DIAGNOSTICS.to_string()];
        assert!(
            super::zone_rank(Some(ids::VIM_MODE), 0, &order)
                < super::zone_rank(Some(ids::DIAGNOSTICS), 0, &order)
        );
        assert_eq!(
            super::zone_rank(Some(ids::LSP_SERVERS), 0, &order)
                < super::zone_rank(Some(ids::ACTIVE_FILE), 0, &order),
            super::zone_rank(Some(ids::LSP_SERVERS), 0, &[])
                < super::zone_rank(Some(ids::ACTIVE_FILE), 0, &[]),
            "the unnamed controls keep the order they had between themselves"
        );
    }

    /// The dock button strips must stay in front of the list as well as the table —
    /// a user ordering their status controls has not asked for the panel icons to
    /// move behind them.
    #[test]
    fn the_list_does_not_pull_a_control_ahead_of_the_dock_strips() {
        use crate::layout_registry::ids;

        let order = vec![ids::CURSOR_POSITION.to_string()];
        assert!(
            super::zone_rank(None, 999, &order)
                < super::zone_rank(Some(ids::CURSOR_POSITION), 0, &order)
        );
    }

    /// A component id is inert here for the same reason it is inert in
    /// `item_locations`: the two sequences are deliberately separate, because a
    /// component's order also places its column on a dock and a control has none.
    #[test]
    fn a_component_id_in_the_list_orders_nothing() {
        use crate::layout_registry::ids;

        let order = vec![ids::PROJECT_PANEL.to_string()];
        assert_eq!(
            super::zone_rank(Some(ids::PROJECT_PANEL), 7, &order),
            (0, 7),
            "listing a component neither moves it here nor gives it a control's place"
        );
    }

    /// Panels share this id space and carry ranks of their own. One appearing as a
    /// status control's adapter id would mean the table had been edited wrongly, and
    /// borrowing its rank would interleave it into the control sequence.
    #[test]
    fn a_panel_id_is_not_given_a_place_in_the_control_sequence() {
        use crate::layout_registry::{SourceRank, get, ids};

        assert!(matches!(
            get(ids::PROJECT_PANEL).unwrap().source_rank,
            SourceRank::Panel(_)
        ));
        assert_eq!(
            super::zone_rank(Some(ids::PROJECT_PANEL), 7, &[]),
            (0, 7),
            "it sorts with the unregistered rather than among the controls"
        );
    }
}
