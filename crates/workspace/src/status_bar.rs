use crate::{
    ItemHandle, MultiWorkspace, Pane, SidebarSide, StatusBarItemLocation, StatusBarSettings,
    ToggleWorkspaceSidebar, layout_registry::CustomizeWorkspaceLayout, sidebar_side_context_menu,
};
use gpui::{
    Anchor, AnyView, App, Context, Decorations, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, Role, SharedString, Styled, Subscription, WeakEntity, Window,
};
use settings::Settings as _;
use settings::{SettingsContent, update_settings_file, update_settings_file_with_completion};
use std::{any::TypeId, sync::Arc};
use theme::CLIENT_SIDE_DECORATION_ROUNDING;
use ui::{ContextMenu, Divider, IconPosition, Indicator, Tooltip, prelude::*, right_click_menu};

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
    show_toggle: bool,
}

impl SidebarStatus {
    fn query(multi_workspace: &Option<WeakEntity<MultiWorkspace>>, cx: &App) -> Self {
        multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| {
                let mw = mw.read(cx);
                let enabled = mw.sidebar_ui_enabled(cx);
                Self {
                    open: mw.sidebar_open() && enabled,
                    side: mw.sidebar_side(cx),
                    has_notifications: mw.sidebar_has_notifications(cx),
                    show_toggle: enabled,
                }
            })
            .unwrap_or_default()
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
}

impl StatusControlAdapter {
    pub fn new(
        id: &'static str,
        legacy_enabled: impl Fn(&App) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            id,
            legacy_enabled: Arc::new(legacy_enabled),
        }
    }

    pub fn id(&self) -> &'static str {
        self.id
    }

    fn effective_location(&self, cx: &App) -> Option<StatusBarItemLocation> {
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
fn zone_order(locations: &[StatusBarItemLocation], zone: StatusBarItemLocation) -> Vec<usize> {
    let mut indices: Vec<usize> = locations
        .iter()
        .enumerate()
        .filter(|(_, location)| **location == zone && zone != StatusBarItemLocation::Hidden)
        .map(|(index, _)| index)
        .collect();
    if zone == StatusBarItemLocation::Right {
        indices.reverse();
    }
    indices
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
        let sidebar = SidebarStatus::query(&self.multi_workspace, cx);

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
            .child(self.render_left_tools(&sidebar, cx))
            // The middle zone sits between the two groups rather than being pinned
            // to the window centre. True centring — independent of how wide the
            // left group happens to be — comes with the overflow work, and until
            // then a placed item is visible here rather than lost.
            .child(self.render_middle_tools(cx))
            .child(self.render_right_tools(&sidebar, cx))
    }
}

impl StatusBar {
    fn render_left_tools(
        &self,
        sidebar: &SidebarStatus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .gap_1()
            .min_w_0()
            .overflow_x_hidden()
            .when(
                sidebar.show_toggle && !sidebar.open && sidebar.side == SidebarSide::Left,
                |this| this.child(self.render_sidebar_toggle(sidebar, cx)),
            )
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
        zone_order(&locations, location)
            .into_iter()
            .filter_map(|index| self.items.get(index).map(|entry| (index, entry)))
            .map(|(index, entry)| {
                // The index is the item's position in the merged list, not a
                // per-zone counter, so ids stay unique when an item changes zone.
                //
                // `location` is handed down rather than re-resolved: it is the
                // zone that actually rendered this item, so a placement menu
                // cannot mark a zone the item is not in.
                render_status_bar_item(side, index, entry, location, &self.focus_handle, cx)
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

    fn render_right_tools(
        &self,
        sidebar: &SidebarStatus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .gap_1()
            .overflow_x_hidden()
            .children(self.render_zone(StatusBarItemLocation::Right, "status-bar-right", cx))
            .when(
                sidebar.show_toggle && !sidebar.open && sidebar.side == SidebarSide::Right,
                |this| this.child(self.render_sidebar_toggle(sidebar, cx)),
            )
    }

    fn render_sidebar_toggle(
        &self,
        sidebar: &SidebarStatus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                    if on_right {
                        IconName::ThreadsSidebarRightClosed
                    } else {
                        IconName::ThreadsSidebarLeftClosed
                    },
                )
                .icon_size(IconSize::Small)
                .tab_index(0isize)
                .aria_label("Open threads sidebar")
                .when(has_notifications, |this| {
                    this.indicator(Indicator::dot().color(Color::Accent))
                        .indicator_border_color(Some(indicator_border))
                })
                .tooltip(move |_, cx| {
                    Tooltip::for_action("Open Threads Sidebar", &ToggleWorkspaceSidebar, cx)
                })
                .on_click(move |_, window, cx| {
                    if let Some(multi_workspace) = window.root::<MultiWorkspace>().flatten() {
                        multi_workspace.update(cx, |multi_workspace, cx| {
                            multi_workspace.toggle_sidebar(window, cx);
                        });
                    }
                })
            });

        h_flex()
            .gap_0p5()
            .when(on_right, |this| {
                this.child(Divider::vertical().color(ui::DividerColor::Border))
            })
            .child(toggle)
            .when(!on_right, |this| {
                this.child(Divider::vertical().color(ui::DividerColor::Border))
            })
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
    focus_handle: &FocusHandle,
    cx: &App,
) -> AnyElement {
    let view = entry.item.to_any();
    let menu_id: SharedString = format!("{side}-item-menu-{index}").into();

    if let Some(adapter) = &entry.adapter {
        let item_id = adapter.id();
        let focus_handle = focus_handle.clone();
        return right_click_menu(menu_id)
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

    const ZONES: [StatusBarItemLocation; 3] = [Left, Middle, Right];

    #[test]
    fn a_zone_takes_only_its_own_items_and_keeps_their_order() {
        let locations = [Left, Right, Left, Middle, Left];
        assert_eq!(zone_order(&locations, Left), vec![0, 2, 4]);
        assert_eq!(zone_order(&locations, Middle), vec![3]);
    }

    #[test]
    fn the_right_zone_renders_reversed() {
        // The item added first ends up furthest from the editor. Two separate
        // lists used to encode this by iterating one of them backwards; with one
        // list it has to be stated.
        let locations = [Right, Right, Right];
        assert_eq!(zone_order(&locations, Right), vec![2, 1, 0]);
        assert_ne!(
            zone_order(&locations, Right),
            vec![0, 1, 2],
            "source order and render order are indistinguishable here, so this \
             fixture cannot tell a reversal from its absence"
        );
    }

    #[test]
    fn only_the_right_zone_reverses() {
        let locations = [Left, Left, Middle, Middle];
        assert_eq!(zone_order(&locations, Left), vec![0, 1]);
        assert_eq!(zone_order(&locations, Middle), vec![2, 3]);
    }

    #[test]
    fn a_hidden_item_is_returned_by_no_zone() {
        // Hiding is this filter. An item resolving to Hidden must match nothing,
        // including a query for Hidden itself — there is no Hidden renderer.
        let locations = [Hidden, Left, Hidden];
        for zone in ZONES {
            assert!(
                !zone_order(&locations, zone).contains(&0),
                "a hidden item reached the {zone:?} zone"
            );
            assert!(!zone_order(&locations, zone).contains(&2));
        }
        assert!(
            zone_order(&locations, Hidden).is_empty(),
            "asking for the Hidden zone returned items to render"
        );
        assert_eq!(zone_order(&locations, Left), vec![1]);
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
                .filter(|zone| zone_order(&locations, **zone).contains(&index))
                .count();
            let expected = if *location == Hidden { 0 } else { 1 };
            assert_eq!(
                appearances, expected,
                "item {index} at {location:?} rendered in {appearances} zones"
            );
        }

        let total: usize = ZONES
            .iter()
            .map(|zone| zone_order(&locations, *zone).len())
            .sum();
        assert_eq!(
            total,
            locations.iter().filter(|l| **l != Hidden).count(),
            "the zones together render a different number of items than are placed"
        );
    }
}
