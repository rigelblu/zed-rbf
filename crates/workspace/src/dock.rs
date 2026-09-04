use crate::focus_follows_mouse::FocusFollowsMouse as _;
use crate::persistence::WorkspaceDb;
use crate::persistence::model::DockData;
use crate::status_bar::HideStatusItem;
use crate::{
    DraggedDock, DraggedDockSeam, Event, FocusFollowsMouse, ModalLayer, Pane,
    WorkspaceLayoutSettings, WorkspaceSettings,
};
use crate::{
    Workspace,
    layout_registry::{CustomizeWorkspaceLayout, LayoutPosition},
    status_bar::StatusItemView,
};
use anyhow::Context as _;
use client::proto;
use db::kvp::KeyValueStore;
use fs::Fs;

use gpui::{
    Action, Anchor, AnyView, App, Axis, Context, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, IntoElement, KeyContext, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement,
    Render, SharedString, StyleRefinement, Styled, Subscription, WeakEntity, Window, deferred, div,
    px,
};
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsStore, TerminalDockPosition};
use std::sync::Arc;
use ui::{
    ContextMenu, CountBadge, Divider, DividerColor, IconButton, Tooltip, prelude::*,
    right_click_menu,
};
use util::ResultExt as _;

pub(crate) const RESIZE_HANDLE_SIZE: Pixels = px(6.);

enum ComponentSetting {
    Position(settings::DockPosition),
    Hidden(bool),
    Pinned(bool),
}

fn write_component_setting(
    fs: Arc<dyn Fs>,
    panel_key: String,
    setting: ComponentSetting,
    cx: &mut App,
) {
    settings::update_settings_file(fs, cx, move |settings, _| {
        let entry = settings
            .workspace_layout
            .get_or_insert_default()
            .component_positions
            .entry(panel_key)
            .or_default();
        match setting {
            ComponentSetting::Position(position) => entry.position = Some(position),
            ComponentSetting::Hidden(hidden) => entry.hidden = Some(hidden),
            ComponentSetting::Pinned(pinned) => entry.pinned = Some(pinned),
        }
    });
}

/// A component's effective side.
///
/// `workspace_layout.component_positions` is authoritative when it names a
/// position the component actually supports; otherwise the component's own dock
/// setting stands. An unsupported saved value falls back instead of failing, so
/// one bad entry cannot strand a component off-screen.
pub fn effective_component_position(
    panel_key: &str,
    own_position: DockPosition,
    cx: &App,
    is_valid: impl Fn(DockPosition) -> bool,
) -> DockPosition {
    let settings = WorkspaceLayoutSettings::get_global(cx);
    let explicit = settings
        .component(panel_key)
        .position
        .map(DockPosition::from);
    resolve_component_position(
        explicit,
        settings.has_component(panel_key),
        registered_component_position(panel_key),
        own_position,
        is_valid,
    )
}

/// The position the registry says a component belongs at, as a dock.
///
/// Status controls share this id space and register positions of their own, so
/// the kind is checked rather than assumed — `cursor_position` registers at Right
/// and would otherwise hand back a dock it has no business naming.
fn registered_component_position(panel_key: &str) -> Option<DockPosition> {
    use crate::layout_registry::{LayoutItemKind, LayoutPosition};

    let item = crate::layout_registry::get(panel_key)?;
    if item.kind != LayoutItemKind::WorkspaceComponent {
        return None;
    }
    match item.registered_position {
        LayoutPosition::Left => Some(DockPosition::Left),
        LayoutPosition::Right => Some(DockPosition::Right),
        LayoutPosition::Bottom => Some(DockPosition::Bottom),
        // A bar zone, not a dock. No component registers here.
        LayoutPosition::Middle => None,
    }
}

/// Where a component sits, given what the user asked for, what the registry says,
/// and what the component's own legacy dock setting says.
///
/// The case worth stating: an explicit entry naming a position the component
/// cannot take does **not** fall through to the legacy dock. Doing that would
/// resurrect a setting the user may long since have replaced, on the strength of
/// a newer setting being unusable. The registered position is the defined answer,
/// and the raw entry stays in the file rather than being silently rewritten.
fn resolve_component_position(
    explicit: Option<DockPosition>,
    entry_exists: bool,
    registered: Option<DockPosition>,
    own_position: DockPosition,
    is_valid: impl Fn(DockPosition) -> bool,
) -> DockPosition {
    if let Some(explicit) = explicit.filter(|position| is_valid(*position)) {
        return explicit;
    }
    let fallback = || {
        registered
            .filter(|position| is_valid(*position))
            .unwrap_or(own_position)
    };
    // The branch is on the *entry*, not on the position within it. A malformed
    // token deserializes to `None` (`with_fallible_options` records the error and
    // substitutes the default), so branching on `explicit.is_some()` sent a typo
    // down the legacy path and resurrected a setting the 2026-09-01 ruling
    // retired. An entry existing at all is what ends legacy's authority; entries
    // without a position are prevented at the source by `seed_component_position`.
    if entry_exists {
        return fallback();
    }
    // With no entry at all, the component's own dock setting supplies the
    // position — but only while it names somewhere the component can go.
    if is_valid(own_position) {
        own_position
    } else {
        fallback()
    }
}

/// A registered workspace component as the layout surface must see it.
///
/// The static `layout_registry` answers everything that cannot change while the
/// app runs — label, icon, aliases, registered position, valid positions,
/// default hidden state. This carries only what needs the running panel: where
/// it actually sits, what it will accept, whether it is open or focused, its
/// size floor, and the action that opens it.
///
/// **Assembled on demand from the workspace's own docks, not held in a global.**
/// `initialize_panels` adds every panel to every workspace, and the hidden check
/// only closes one, so the workspace already holds a handle for every registered
/// component — hidden, closed, or open. A second map keyed by the same
/// `panel_key` would be one more pair of sources that can disagree, which is the
/// hazard this feature has already been bitten by more than once.
pub struct ComponentAdapter {
    panel: Arc<dyn PanelHandle>,
    is_open: bool,
    is_focused: bool,
}

impl ComponentAdapter {
    pub(crate) fn new(panel: Arc<dyn PanelHandle>, is_open: bool, is_focused: bool) -> Self {
        Self {
            panel,
            is_open,
            is_focused,
        }
    }

    /// Where the component actually is, resolved through the shared
    /// workspace-layout service — not the panel's own legacy dock setting.
    pub fn position(&self, window: &Window, cx: &App) -> DockPosition {
        self.panel.position(window, cx)
    }

    /// Whether the live panel will accept a position. The static table also
    /// records valid positions, and the two can disagree: this is the panel's
    /// own answer and is the one to offer a user, so a choice cannot be shown
    /// that the component then refuses.
    pub fn accepts(&self, position: DockPosition, cx: &App) -> bool {
        self.panel.position_is_valid(position, cx)
    }

    /// Open in this workspace. Closed is workspace-local; hidden is global and
    /// is a separate question the settings answer without a handle.
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn is_focused(&self) -> bool {
        self.is_focused
    }

    /// The component's own toggle action, so a surface opens it the same way its
    /// icon and its keybinding do rather than inventing a third path.
    pub fn toggle_action(&self, window: &Window, cx: &App) -> Box<dyn Action> {
        self.panel.toggle_action(window, cx)
    }

    /// The width below which this component stops being usable, at the position
    /// the shared layout service resolves for it.
    pub fn effective_min_width(&self, window: &Window, cx: &App) -> Pixels {
        effective_min_width_at(self.position(window, cx), self.panel.as_ref(), window, cx)
    }
}

/// The width below which a component stops being usable — its declared `min_size`
/// when it has one, else the registry floor for the area it sits in.
///
/// `position` is a parameter rather than read off the panel because the two
/// callers are authoritative about different things: the layout surface asks
/// where the shared service says a component belongs, while a dock computing its
/// own floor already knows which side is being dragged.
pub(crate) fn effective_min_width_at(
    position: DockPosition,
    panel: &dyn PanelHandle,
    window: &Window,
    cx: &App,
) -> Pixels {
    let area = match position {
        DockPosition::Left | DockPosition::Right => LayoutPosition::Left,
        DockPosition::Bottom => LayoutPosition::Bottom,
    };
    area.effective_min_width(panel.min_size(window, cx))
}

// Two things the design lists for this adapter are deliberately absent until
// something consumes them. The legacy dock value would need a new `PanelHandle`
// method to reach `Panel::position` underneath the resolver, and legacy
// migration is not the next piece. The post-move hook has no side effect to run
// yet — adding either now would be guessing at a shape the surface has not
// asked for.

/// Moves a component to a side globally. Its icon follows the component rather
/// than carrying a placement of its own.
pub fn set_component_position(
    fs: Arc<dyn Fs>,
    panel_key: String,
    position: settings::DockPosition,
    cx: &mut App,
) {
    write_component_setting(fs, panel_key, ComponentSetting::Position(position), cx);
}

/// Writes a component's current position into `component_positions`, but only if
/// it has no entry there yet.
///
/// Pinning or hiding a component creates an entry through `or_default()`, with no
/// position in it. Once an entry exists the component's legacy `<panel>.dock` no
/// longer applies — that is the whole point of the entry — so without this, the
/// first pin would silently relocate a component to its registered position. This
/// writes down where it already is, so managing a component through the new
/// system records its position instead of changing it.
///
/// Callers pass the component's *effective* position, which needs the live panel;
/// it cannot be derived here from the key alone.
pub fn seed_component_position(
    fs: Arc<dyn Fs>,
    panel_key: String,
    position: settings::DockPosition,
    cx: &mut App,
) {
    settings::update_settings_file(fs, cx, move |settings, _| {
        let components = &mut settings
            .workspace_layout
            .get_or_insert_default()
            .component_positions;
        if components.contains_key(&panel_key) {
            return;
        }
        components.entry(panel_key).or_default().position = Some(position);
    });
}

/// Decides whether a component keeps its own column or shares its side's single
/// unpinned slot.
pub fn set_component_pinned(fs: Arc<dyn Fs>, panel_key: String, pinned: bool, cx: &mut App) {
    write_component_setting(fs, panel_key, ComponentSetting::Pinned(pinned), cx);
}

/// Hides or restores a component everywhere.
///
/// Unhiding first forgets the component's openness in every serialized
/// workspace. A workspace that was dormant across both the hide and this
/// unhide keeps a stale open row, and current settings hold no evidence that
/// the hide ever happened — so without this the component would return open
/// the moment it became visible again.
///
/// Both unhide paths share this function precisely so neither can skip it.
pub fn set_component_hidden(fs: Arc<dyn Fs>, panel_key: String, hidden: bool, cx: &mut App) {
    if hidden {
        write_component_setting(fs, panel_key, ComponentSetting::Hidden(true), cx);
        return;
    }

    let db = WorkspaceDb::global(cx);
    cx.spawn(async move |cx| {
        // Ordered deliberately: the rows go before the setting turns visible.
        // A crash between the two leaves only forgotten openness, which is what
        // Hidden promised anyway.
        db.scrub_open_component(panel_key.clone()).await.log_err();
        cx.update(|cx| {
            write_component_setting(fs, panel_key, ComponentSetting::Hidden(false), cx);
        });
    })
    .detach();
}

pub enum PanelEvent {
    ZoomIn,
    ZoomOut,
    Activate,
    Close,
}

pub use proto::PanelId;

pub trait Panel: Focusable + EventEmitter<PanelEvent> + Render + Sized {
    fn persistent_name() -> &'static str;
    fn panel_key() -> &'static str;
    /// The `Focusable::focus_handle` root identifies the panel's subtree for containment checks
    /// and must be tracked by the panel's root element. This method returns the handle that should
    /// receive focus when the panel is activated, such as a filter, commit, or message editor; it
    /// must be a focus-tree descendant of the root or containment checks such as Zen-mode auto-close
    /// and toggle-focus will misbehave.
    fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
        self.focus_handle(cx)
    }
    fn position(&self, window: &Window, cx: &App) -> DockPosition;
    fn position_is_valid(&self, position: DockPosition) -> bool;
    fn set_position(&mut self, position: DockPosition, window: &mut Window, cx: &mut Context<Self>);
    fn default_size(&self, window: &Window, cx: &App) -> Pixels;
    fn min_size(&self, _window: &Window, _cx: &App) -> Option<Pixels> {
        None
    }
    fn initial_size_state(&self, _window: &Window, _cx: &App) -> PanelSizeState {
        PanelSizeState::default()
    }
    fn size_state_changed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
    fn supports_flexible_size(&self) -> bool {
        false
    }
    fn has_flexible_size(&self, _window: &Window, _cx: &App) -> bool {
        false
    }
    fn set_flexible_size(
        &mut self,
        _flexible: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }
    fn icon(&self, window: &Window, cx: &App) -> Option<ui::IconName>;
    fn icon_tooltip(&self, window: &Window, cx: &App) -> Option<&'static str>;
    fn toggle_action(&self) -> Box<dyn Action>;
    fn icon_label(&self, _window: &Window, _: &App) -> Option<String> {
        None
    }
    fn is_zoomed(&self, _window: &Window, _cx: &App) -> bool {
        false
    }
    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        false
    }
    fn set_zoomed(&mut self, _zoomed: bool, _window: &mut Window, _cx: &mut Context<Self>) {}
    fn set_active(&mut self, _active: bool, _window: &mut Window, _cx: &mut Context<Self>) {}
    fn pane(&self) -> Option<Entity<Pane>> {
        None
    }
    fn remote_id() -> Option<proto::PanelId> {
        None
    }
    fn activation_priority(&self) -> u32;
    fn enabled(&self, _cx: &App) -> bool {
        true
    }
    fn is_agent_panel(&self) -> bool {
        false
    }
    /// Returns metadata describing how to hide this panel's button from the
    /// status bar by writing to user settings. Implementors should return
    /// `None` if the panel button cannot be hidden through settings.
    fn hide_button_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}

pub trait PanelHandle: Send + Sync {
    fn panel_id(&self) -> EntityId;
    fn persistent_name(&self) -> &'static str;
    fn panel_key(&self) -> &'static str;
    fn position(&self, window: &Window, cx: &App) -> DockPosition;
    fn position_is_valid(&self, position: DockPosition, cx: &App) -> bool;
    fn set_position(&self, position: DockPosition, window: &mut Window, cx: &mut App);
    fn is_zoomed(&self, window: &Window, cx: &App) -> bool;
    fn set_zoomed(&self, zoomed: bool, window: &mut Window, cx: &mut App);
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App);
    fn remote_id(&self) -> Option<proto::PanelId>;
    fn pane(&self, cx: &App) -> Option<Entity<Pane>>;
    fn default_size(&self, window: &Window, cx: &App) -> Pixels;
    fn min_size(&self, window: &Window, cx: &App) -> Option<Pixels>;
    fn initial_size_state(&self, window: &Window, cx: &App) -> PanelSizeState;
    fn size_state_changed(&self, window: &mut Window, cx: &mut App);
    fn supports_flexible_size(&self, cx: &App) -> bool;
    fn has_flexible_size(&self, window: &Window, cx: &App) -> bool;
    fn set_flexible_size(&self, flexible: bool, window: &mut Window, cx: &mut App);
    fn icon(&self, window: &Window, cx: &App) -> Option<ui::IconName>;
    fn icon_tooltip(&self, window: &Window, cx: &App) -> Option<&'static str>;
    fn toggle_action(&self, window: &Window, cx: &App) -> Box<dyn Action>;
    fn icon_label(&self, window: &Window, cx: &App) -> Option<String>;
    fn panel_focus_handle(&self, cx: &App) -> FocusHandle;
    /// See `Panel::activation_focus_handle`.
    fn activation_focus_handle(&self, cx: &App) -> FocusHandle;
    fn to_any(&self) -> AnyView;
    fn activation_priority(&self, cx: &App) -> u32;
    fn enabled(&self, cx: &App) -> bool;
    fn is_agent_panel(&self, cx: &App) -> bool;
    fn hide_button_setting(&self, cx: &App) -> Option<HideStatusItem>;
    fn move_to_next_position(&self, window: &mut Window, cx: &mut App) {
        let current_position = self.position(window, cx);
        let next_position = [
            DockPosition::Left,
            DockPosition::Bottom,
            DockPosition::Right,
        ]
        .into_iter()
        .filter(|position| self.position_is_valid(*position, cx))
        .skip_while(|valid_position| *valid_position != current_position)
        .nth(1)
        .unwrap_or(DockPosition::Left);

        self.set_position(next_position, window, cx);
    }
}

impl<T> PanelHandle for Entity<T>
where
    T: Panel,
{
    fn panel_id(&self) -> EntityId {
        Entity::entity_id(self)
    }

    fn persistent_name(&self) -> &'static str {
        T::persistent_name()
    }

    fn panel_key(&self) -> &'static str {
        T::panel_key()
    }

    fn position(&self, window: &Window, cx: &App) -> DockPosition {
        let own_position = self.read(cx).position(window, cx);
        effective_component_position(T::panel_key(), own_position, cx, |position| {
            self.read(cx).position_is_valid(position)
        })
    }

    fn position_is_valid(&self, position: DockPosition, cx: &App) -> bool {
        self.read(cx).position_is_valid(position)
    }

    fn set_position(&self, position: DockPosition, window: &mut Window, cx: &mut App) {
        // `component_positions` is authoritative, but the component's own dock
        // setting is kept in step so it remains a usable fallback.
        set_component_position(
            <dyn Fs>::global(cx),
            T::panel_key().to_string(),
            position.into(),
            cx,
        );
        self.update(cx, |this, cx| this.set_position(position, window, cx))
    }

    fn is_zoomed(&self, window: &Window, cx: &App) -> bool {
        self.read(cx).is_zoomed(window, cx)
    }

    fn set_zoomed(&self, zoomed: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_zoomed(zoomed, window, cx))
    }

    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_active(active, window, cx))
    }

    fn pane(&self, cx: &App) -> Option<Entity<Pane>> {
        self.read(cx).pane()
    }

    fn remote_id(&self) -> Option<PanelId> {
        T::remote_id()
    }

    fn default_size(&self, window: &Window, cx: &App) -> Pixels {
        self.read(cx).default_size(window, cx)
    }

    fn min_size(&self, window: &Window, cx: &App) -> Option<Pixels> {
        self.read(cx).min_size(window, cx)
    }

    fn initial_size_state(&self, window: &Window, cx: &App) -> PanelSizeState {
        self.read(cx).initial_size_state(window, cx)
    }

    fn size_state_changed(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.size_state_changed(window, cx))
    }

    fn supports_flexible_size(&self, cx: &App) -> bool {
        self.read(cx).supports_flexible_size()
    }

    fn has_flexible_size(&self, window: &Window, cx: &App) -> bool {
        self.read(cx).has_flexible_size(window, cx)
    }

    fn set_flexible_size(&self, flexible: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_flexible_size(flexible, window, cx))
    }

    fn icon(&self, window: &Window, cx: &App) -> Option<ui::IconName> {
        self.read(cx).icon(window, cx)
    }

    fn icon_tooltip(&self, window: &Window, cx: &App) -> Option<&'static str> {
        self.read(cx).icon_tooltip(window, cx)
    }

    fn toggle_action(&self, _: &Window, cx: &App) -> Box<dyn Action> {
        self.read(cx).toggle_action()
    }

    fn icon_label(&self, window: &Window, cx: &App) -> Option<String> {
        self.read(cx).icon_label(window, cx)
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn panel_focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).activation_focus_handle(cx)
    }

    fn activation_priority(&self, cx: &App) -> u32 {
        self.read(cx).activation_priority()
    }

    fn enabled(&self, cx: &App) -> bool {
        self.read(cx).enabled(cx)
    }

    fn is_agent_panel(&self, cx: &App) -> bool {
        self.read(cx).is_agent_panel()
    }

    fn hide_button_setting(&self, cx: &App) -> Option<HideStatusItem> {
        self.read(cx).hide_button_setting(cx)
    }
}

impl From<&dyn PanelHandle> for AnyView {
    fn from(val: &dyn PanelHandle) -> Self {
        val.to_any()
    }
}

/// A container with a fixed [`DockPosition`] adjacent to a certain widown edge.
/// Can contain multiple panels and show/hide itself with all contents.
pub struct Dock {
    position: DockPosition,
    panel_entries: Vec<PanelEntry>,
    workspace: WeakEntity<Workspace>,
    // Every open panel renders side by side, in `panel_entries` order.
    open_panels: Vec<EntityId>,
    // What a group toggle closed, so `set_open(true)` restores the same set.
    collapsed_panels: Vec<EntityId>,
    // The open panel with stronger emphasis; the dock's focus lands here.
    focused_panel: Option<EntityId>,
    focus_handle: FocusHandle,
    focus_follows_mouse: FocusFollowsMouse,
    pub(crate) serialized_dock: Option<DockData>,
    /// Panel keys this workspace had open, restored alongside `serialized_dock`.
    /// The legacy dock columns can only name one active panel, so the rest of
    /// the open set arrives here.
    pub(crate) serialized_open_components: Option<Vec<String>>,
    /// Whether the dock-level half of `serialized_dock` (focused panel, zoom,
    /// whether the side is open) has already been applied.
    serialized_dock_applied: bool,
    zoom_layer_open: bool,
    modal_layer: Entity<ModalLayer>,
    _subscriptions: [Subscription; 2],
}

impl Focusable for Dock {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DockPosition {
    Left,
    Bottom,
    Right,
}

impl From<settings::DockPosition> for DockPosition {
    fn from(value: settings::DockPosition) -> Self {
        match value {
            settings::DockPosition::Left => Self::Left,
            settings::DockPosition::Bottom => Self::Bottom,
            settings::DockPosition::Right => Self::Right,
        }
    }
}

impl Into<settings::DockPosition> for DockPosition {
    fn into(self) -> settings::DockPosition {
        match self {
            Self::Left => settings::DockPosition::Left,
            Self::Bottom => settings::DockPosition::Bottom,
            Self::Right => settings::DockPosition::Right,
        }
    }
}

impl From<TerminalDockPosition> for DockPosition {
    fn from(value: TerminalDockPosition) -> Self {
        match value {
            TerminalDockPosition::Left => DockPosition::Left,
            TerminalDockPosition::Bottom => DockPosition::Bottom,
            TerminalDockPosition::Right => DockPosition::Right,
        }
    }
}

impl DockPosition {
    fn label(&self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Bottom => "Bottom",
            Self::Right => "Right",
        }
    }

    pub fn axis(&self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Bottom => Axis::Vertical,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelSizeState {
    pub size: Option<Pixels>,
    #[serde(default)]
    pub flex: Option<f32>,
}

struct PanelEntry {
    panel: Arc<dyn PanelHandle>,
    size_state: PanelSizeState,
    _subscriptions: [Subscription; 4],
}

pub struct PanelButtons {
    dock: Entity<Dock>,
    _settings_subscription: Subscription,
}

pub(crate) const PANEL_SIZE_STATE_KEY: &str = "dock_panel_size";

fn panel_uses_flexible_width(
    position: DockPosition,
    panel: &dyn PanelHandle,
    window: &Window,
    cx: &App,
) -> bool {
    position.axis() == Axis::Horizontal && panel.has_flexible_size(window, cx)
}

fn resize_panel_entry(
    position: DockPosition,
    entry: &mut PanelEntry,
    size: Option<Pixels>,
    flex: Option<f32>,
    window: &mut Window,
    cx: &mut App,
) -> (&'static str, PanelSizeState) {
    let size = size.map(|size| size.max(RESIZE_HANDLE_SIZE).round());
    let uses_flexible_width = panel_uses_flexible_width(position, entry.panel.as_ref(), window, cx);
    if uses_flexible_width {
        entry.size_state.flex = flex;
    } else {
        entry.size_state.size = size;
    }
    entry.panel.size_state_changed(window, cx);
    (entry.panel.panel_key(), entry.size_state)
}

impl Dock {
    pub fn new(
        position: DockPosition,
        modal_layer: Entity<ModalLayer>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let focus_handle = cx.focus_handle();
        let workspace = cx.entity();
        let dock = cx.new(|cx| {
            let focus_subscription =
                cx.on_focus(&focus_handle, window, |dock: &mut Dock, window, cx| {
                    if let Some(active_entry) = dock.active_panel_entry() {
                        active_entry
                            .panel
                            .activation_focus_handle(cx)
                            .focus(window, cx)
                    }
                });
            let zoom_subscription = cx.subscribe(&workspace, |dock, workspace, e: &Event, cx| {
                if matches!(e, Event::ZoomChanged) {
                    let is_zoomed = workspace.read(cx).zoomed.is_some();
                    dock.zoom_layer_open = is_zoomed;
                }
            });
            Self {
                position,
                workspace: workspace.downgrade(),
                panel_entries: Default::default(),
                open_panels: Vec::new(),
                collapsed_panels: Vec::new(),
                focused_panel: None,
                focus_handle: focus_handle.clone(),
                focus_follows_mouse: WorkspaceSettings::get_global(cx).focus_follows_mouse,
                _subscriptions: [focus_subscription, zoom_subscription],
                serialized_dock: None,
                serialized_open_components: None,
                serialized_dock_applied: false,
                zoom_layer_open: false,
                modal_layer,
            }
        });

        cx.on_focus_in(&focus_handle, window, {
            let dock = dock.downgrade();
            move |workspace, window, cx| {
                let Some(dock) = dock.upgrade() else {
                    return;
                };
                let Some(panel) = dock.read(cx).active_panel() else {
                    return;
                };
                if panel.is_zoomed(window, cx) {
                    workspace.zoomed = Some(panel.to_any().downgrade());
                    workspace.zoomed_position = Some(position);
                } else {
                    workspace.zoomed = None;
                    workspace.zoomed_position = None;
                }
                cx.emit(Event::ZoomChanged);
                workspace.dismiss_zoomed_items_to_reveal(Some(position), window, cx);
                workspace.update_active_view_for_followers(window, cx)
            }
        })
        .detach();

        cx.observe_in(&dock, window, move |workspace, dock, window, cx| {
            if dock.read(cx).is_open()
                && let Some(panel) = dock.read(cx).active_panel()
                && panel.is_zoomed(window, cx)
            {
                workspace.zoomed = Some(panel.to_any().downgrade());
                workspace.zoomed_position = Some(position);
                cx.emit(Event::ZoomChanged);
                return;
            }
            if workspace.zoomed_position == Some(position) {
                workspace.zoomed = None;
                workspace.zoomed_position = None;
                cx.emit(Event::ZoomChanged);
            }
        })
        .detach();

        dock
    }

    pub fn position(&self) -> DockPosition {
        self.position
    }

    pub fn is_open(&self) -> bool {
        !self.open_panels.is_empty()
    }

    fn resizable(&self, cx: &App) -> bool {
        !(self.zoom_layer_open || self.modal_layer.read(cx).has_active_modal())
    }

    pub fn panel<T: Panel>(&self) -> Option<Entity<T>> {
        self.panel_entries
            .iter()
            .find_map(|entry| entry.panel.to_any().downcast().ok())
    }

    pub fn panel_index_for_type<T: Panel>(&self) -> Option<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.to_any().downcast::<T>().is_ok())
    }

    pub fn panel_id_for_panel_key(&self, panel_key: &str) -> Option<EntityId> {
        self.panel_for_key(panel_key).map(|panel| panel.panel_id())
    }

    /// The live panel registered under a layout-registry id, if it has reached
    /// this dock.
    ///
    /// Registration is unconditional — `initialize_panels` adds every panel to
    /// every workspace, and the hidden check only closes one — so a component
    /// that is hidden or closed still answers here. It is `None` only before the
    /// panel's async load lands, or for an id no crate registers.
    pub fn panel_for_key(&self, panel_key: &str) -> Option<&Arc<dyn PanelHandle>> {
        self.panel_entries
            .iter()
            .find(|entry| entry.panel.panel_key() == panel_key)
            .map(|entry| &entry.panel)
    }

    pub fn panel_index_for_persistent_name(&self, ui_name: &str, _cx: &App) -> Option<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.persistent_name() == ui_name)
    }

    pub fn panel_index_for_proto_id(&self, panel_id: PanelId) -> Option<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.remote_id() == Some(panel_id))
    }

    pub fn panel_for_id(&self, panel_id: EntityId) -> Option<&Arc<dyn PanelHandle>> {
        self.panel_entries
            .iter()
            .find(|entry| entry.panel.panel_id() == panel_id)
            .map(|entry| &entry.panel)
    }

    pub fn first_enabled_panel_idx(&mut self, cx: &mut Context<Self>) -> anyhow::Result<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.enabled(cx))
            .with_context(|| {
                format!(
                    "Couldn't find any enabled panel for the {} dock.",
                    self.position.label()
                )
            })
    }

    fn entry_index(&self, panel_id: EntityId) -> Option<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.panel_id() == panel_id)
    }

    fn entry_for_id(&self, panel_id: EntityId) -> Option<&PanelEntry> {
        self.panel_entries
            .iter()
            .find(|entry| entry.panel.panel_id() == panel_id)
    }

    fn active_panel_entry(&self) -> Option<&PanelEntry> {
        self.entry_for_id(self.focused_panel?)
    }

    pub fn active_panel_index(&self) -> Option<usize> {
        self.focused_panel.and_then(|id| self.entry_index(id))
    }

    pub fn focused_panel_id(&self) -> Option<EntityId> {
        self.focused_panel
    }

    pub fn is_panel_open(&self, panel_id: EntityId) -> bool {
        self.open_panels.contains(&panel_id)
    }

    pub fn panel_at(&self, panel_ix: usize) -> Option<&Arc<dyn PanelHandle>> {
        self.panel_entries.get(panel_ix).map(|entry| &entry.panel)
    }

    // Display order: `panel_entries` order, reversed on the right so the
    // columns read left-to-right like the reversed icons in the bottom bar.
    /// Indices into `panel_entries` for the open panels, in the order they are
    /// drawn left to right. `open_entries` is defined in terms of this so a seam
    /// index and a rendered column cannot disagree about which pair they name.
    fn open_entry_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = self
            .panel_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.open_panels.contains(&entry.panel.panel_id()))
            .map(|(index, _)| index)
            .collect();
        if self.position == DockPosition::Right {
            indices.reverse();
        }
        indices
    }

    fn open_entries(&self) -> Vec<&PanelEntry> {
        self.open_entry_indices()
            .into_iter()
            .filter_map(|index| self.panel_entries.get(index))
            .collect()
    }

    /// What each open column is actually as wide as right now, in drawn order.
    /// A fixed column reports its stored width; a flexible one has none, so it
    /// reports the share of the dock's leftover space its flex factor earns —
    /// which is the number a drag has to reason about, because it is the one on
    /// screen.
    pub(crate) fn rendered_widths(
        &self,
        dock_total: Pixels,
        window: &Window,
        cx: &App,
    ) -> Vec<Pixels> {
        let fixed_total = self.open_fixed_size(window, cx);
        let flex_total = self.open_flex(None, window, cx).unwrap_or(0.0);
        let leftover = (dock_total - fixed_total).max(Pixels::ZERO);
        self.open_entries()
            .into_iter()
            .map(|entry| {
                if panel_uses_flexible_width(self.position, entry.panel.as_ref(), window, cx) {
                    if flex_total > 0.0 {
                        leftover * (entry.size_state.flex.unwrap_or(1.0) / flex_total)
                    } else {
                        leftover
                    }
                } else {
                    entry
                        .size_state
                        .size
                        .unwrap_or_else(|| entry.panel.default_size(window, cx))
                }
            })
            .collect()
    }

    /// Whether the seam on column `index`'s leading edge can be dragged at all.
    ///
    /// Every seam between two open columns now qualifies. A flexible neighbour
    /// used to disqualify one: a flexible column has no width of its own, so
    /// moving width into or out of it changed the side's total rather than
    /// trading inside it, and no handle was drawn rather than shipping a control
    /// that looks live and does nothing. `resize_seam` holds the side's total by
    /// construction now — it re-derives every flexible column's flex against the
    /// fixed total the drag produces — so the refusal is gone.
    pub(crate) fn seam_is_resizable(&self, index: usize, _window: &Window, _cx: &App) -> bool {
        let Some(leading_index) = index.checked_sub(1) else {
            return false;
        };
        let entries = self.open_entries();
        entries.get(leading_index).is_some() && entries.get(index).is_some()
    }

    /// Move the seam on column `index`'s leading edge so the column before it
    /// becomes `leading_target` wide, taking the difference from the column after
    /// it and leaving every other column and the side's total untouched.
    ///
    /// One path covers all four neighbour pairings rather than branching on them.
    /// Decide the whole side's target widths, split them into the fixed total the
    /// drag produces and the width left for flex, then derive one flex factor for
    /// that share and hand each flexible column its proportion of it:
    ///
    /// - **two fixed** — nothing flexible, so this reduces to writing the pair
    /// - **two flexible** — the fixed total does not move and the pair's combined
    ///   share does not either, so it is a pure redistribution between them
    /// - **one of each** — the fixed column takes its new width, the flexible one
    ///   absorbs the complement, and the side's total holds because the fixed
    ///   total grew by exactly what the flexible share lost
    ///
    /// Only the pair is written, and a flexible column outside it provably does
    /// not need touching. Its flex is `Sf * width / T`, and `Sf / T` reduces to
    /// `C / (available(fixed) - T)`, whose denominator is `W - opposite - total`.
    /// A seam holds the side's `total` by contract, so that ratio is invariant
    /// across the drag and re-deriving a bystander returns the number it already
    /// had. The bystander witness proves the outcome; this is why it is free —
    /// and a drag that *did* move the side's total would break the invariant.
    pub(crate) fn resize_seam(
        &mut self,
        index: usize,
        leading_target: Pixels,
        dock_total: Pixels,
        geometry: &crate::DockGeometry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.seam_is_resizable(index, window, cx) {
            return;
        }
        let Some(leading_slot) = index.checked_sub(1) else {
            return;
        };
        let indices = self.open_entry_indices();
        let (Some(&leading_index), Some(&trailing_index)) =
            (indices.get(leading_slot), indices.get(index))
        else {
            return;
        };
        let widths = self.rendered_widths(dock_total, window, cx);
        let (Some(&leading_width), Some(&trailing_width)) =
            (widths.get(leading_slot), widths.get(index))
        else {
            return;
        };

        let pair_total = leading_width + trailing_width;
        let position = self.position;
        let leading_min = effective_min_width_at(
            position,
            self.panel_entries[leading_index].panel.as_ref(),
            window,
            cx,
        );
        let trailing_min = effective_min_width_at(
            position,
            self.panel_entries[trailing_index].panel.as_ref(),
            window,
            cx,
        );
        // Both minimums cannot be honoured when the pair is narrower than their
        // sum, which the side floor prevents by drag but a restored layout can
        // still produce. Refusing to move is better than picking a winner.
        if pair_total < leading_min + trailing_min {
            return;
        }
        let leading_new = leading_target.clamp(leading_min, pair_total - trailing_min);

        let mut targets = widths;
        targets[leading_slot] = leading_new;
        targets[index] = pair_total - leading_new;

        let flexible: Vec<bool> = indices
            .iter()
            .map(|&entry_index| {
                panel_uses_flexible_width(
                    position,
                    self.panel_entries[entry_index].panel.as_ref(),
                    window,
                    cx,
                )
            })
            .collect();

        let mut new_fixed = Pixels::ZERO;
        let mut flexible_share = Pixels::ZERO;
        for (slot, &is_flexible) in flexible.iter().enumerate() {
            if is_flexible {
                flexible_share += targets[slot];
            } else {
                new_fixed += targets[slot];
            }
        }
        let total_flex = if flexible_share > Pixels::ZERO {
            geometry.flex_for_share(new_fixed, flexible_share)
        } else {
            0.0
        };

        let mut size_states_to_persist = Vec::new();
        for (slot, &entry_index) in indices.iter().enumerate() {
            let in_pair = slot == leading_slot || slot == index;
            if !in_pair {
                // Untouched by construction. A fixed bystander keeps whatever it
                // stored, including having stored nothing — writing its rendered
                // width back would silently pin it to today's default. A flexible
                // bystander keeps its flex because re-deriving it is a no-op
                // while the side's total holds; see the note above.
                continue;
            }
            let (size, flex) = if flexible[slot] {
                (
                    None,
                    Some(total_flex * (targets[slot] / flexible_share).max(0.0)),
                )
            } else {
                (Some(targets[slot]), None)
            };
            let Some(entry) = self.panel_entries.get_mut(entry_index) else {
                continue;
            };
            size_states_to_persist
                .push(resize_panel_entry(position, entry, size, flex, window, cx));
        }

        let workspace = self.workspace.clone();
        cx.defer(move |cx| {
            if let Some(workspace) = workspace.upgrade() {
                workspace.update(cx, |workspace, cx| {
                    for (panel_key, size_state) in size_states_to_persist {
                        workspace.persist_panel_size_state(panel_key, size_state, cx);
                    }
                });
            }
        });

        cx.notify();
    }

    pub fn open_panels(&self) -> Vec<Arc<dyn PanelHandle>> {
        self.open_entries()
            .into_iter()
            .map(|entry| entry.panel.clone())
            .collect()
    }

    /// Width the open fixed-size panels claim on a horizontal dock; flexible
    /// panels are excluded because they take their share from the flex row.
    pub fn open_fixed_size(&self, window: &Window, cx: &App) -> Pixels {
        self.open_entries()
            .into_iter()
            .filter(|entry| {
                !panel_uses_flexible_width(self.position, entry.panel.as_ref(), window, cx)
            })
            .map(|entry| {
                entry
                    .size_state
                    .size
                    .unwrap_or_else(|| entry.panel.default_size(window, cx))
            })
            .fold(Pixels::ZERO, |sum, size| sum + size)
    }

    /// Effective minimums of the open fixed-width panels, summed — the counterpart
    /// to `open_fixed_size`, taken over exactly the same entries so the two can be
    /// subtracted from one another.
    fn open_fixed_min_size(&self, window: &Window, cx: &App) -> Pixels {
        self.open_entries()
            .into_iter()
            .filter(|entry| {
                !panel_uses_flexible_width(self.position, entry.panel.as_ref(), window, cx)
            })
            .map(|entry| effective_min_width_at(self.position, entry.panel.as_ref(), window, cx))
            .fold(Pixels::ZERO, |sum, size| sum + size)
    }

    /// Total flex factor of the open flexible panels, `None` when none is flexible.
    pub fn open_flex(&self, default_flex: Option<f32>, window: &Window, cx: &App) -> Option<f32> {
        let mut total = None;
        for entry in self.open_entries() {
            if panel_uses_flexible_width(self.position, entry.panel.as_ref(), window, cx) {
                let flex = entry.size_state.flex.or(default_flex).unwrap_or(1.0);
                total = Some(total.unwrap_or(0.0) + flex);
            }
        }
        total
    }

    /// The width below which this side stops being usable. A flexible column
    /// contributes its effective minimum; a fixed column beside one contributes
    /// its stored width, because that is what it holds through a drag. A component
    /// declaring no `min_size` contributes its registry area floor rather than
    /// nothing, which is the whole point — a side holding only undeclared
    /// components used to floor at nothing, so the editor-facing divider dragged
    /// it down to `RESIZE_HANDLE_SIZE`.
    ///
    /// `None` for the bottom dock. Its resize axis is height, while the registry's
    /// bottom number is a segment *width* governing the row's internal dividers —
    /// a different quantity on a different axis, and summing it here would clamp a
    /// height against widths.
    ///
    /// No caller reaches that branch today: both clamps are on the side docks and
    /// the render call sits inside its own horizontal arm. It guards this
    /// function's contract for the bottom-divider work, not a live path — deleting
    /// it breaks no test, which is the honest reason to leave it documented rather
    /// than let the next reader defend it as load-bearing.
    pub fn open_min_size(&self, window: &Window, cx: &App) -> Option<Pixels> {
        if self.position.axis() != Axis::Horizontal {
            return None;
        }
        let entries = self.open_entries();
        // A fixed column beside a flexible one holds its stored width instead of
        // scaling toward its minimum, so the side cannot go below that width plus
        // the flexible columns' minimums. Summing every minimum was right while a
        // drag scaled every column together; once fixed means fixed it under-counts
        // by the fixed column's own headroom, and the side is free to shrink past
        // what its contents can occupy. The columns then stop tracking the pointer
        // and `overflow_hidden` clips them (Tom, dogfooding F-3, 2026-09-03).
        let mixed = entries.iter().any(|entry| {
            panel_uses_flexible_width(self.position, entry.panel.as_ref(), window, cx)
        });
        entries
            .into_iter()
            .map(|entry| {
                let min = effective_min_width_at(self.position, entry.panel.as_ref(), window, cx);
                if !mixed
                    || panel_uses_flexible_width(self.position, entry.panel.as_ref(), window, cx)
                {
                    return min;
                }
                entry
                    .size_state
                    .size
                    .unwrap_or_else(|| entry.panel.default_size(window, cx))
                    .max(min)
            })
            .reduce(|sum, size| sum + size)
    }

    /// The shared height of the bottom row: the focused panel's, else the first open panel's.
    pub fn open_row_size(&self, window: &Window, cx: &App) -> Option<Pixels> {
        let entry = self.visible_entry()?;
        Some(
            entry
                .size_state
                .size
                .unwrap_or_else(|| entry.panel.default_size(window, cx)),
        )
    }

    fn is_pinned(panel: &dyn PanelHandle, cx: &App) -> bool {
        WorkspaceLayoutSettings::get_global(cx)
            .component(panel.panel_key())
            .pinned
    }

    fn is_hidden_key(panel_key: &str, cx: &App) -> bool {
        WorkspaceLayoutSettings::get_global(cx)
            .component(panel_key)
            .hidden
    }

    fn is_hidden(panel: &dyn PanelHandle, cx: &App) -> bool {
        Self::is_hidden_key(panel.panel_key(), cx)
    }

    fn open_panel_id(
        &mut self,
        panel_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.entry_index(panel_id) else {
            return false;
        };
        if self.open_panels.contains(&panel_id) {
            return false;
        }
        // Hidden is global and removes the component everywhere. Refusing here
        // rather than at render is what keeps a hidden id out of the open set
        // itself, so a later unhide cannot find it still recorded as open.
        if self
            .panel_entries
            .get(index)
            .is_some_and(|entry| Self::is_hidden(entry.panel.as_ref(), cx))
        {
            return false;
        }
        // Unpinned panels share one slot per side: opening one displaces the
        // unpinned panel showing there. Pinned panels are never displaced.
        let new_is_pinned = self
            .panel_entries
            .get(index)
            .is_some_and(|entry| Self::is_pinned(entry.panel.as_ref(), cx));
        if !new_is_pinned {
            let displaced: Vec<usize> = self
                .panel_entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    self.open_panels.contains(&entry.panel.panel_id())
                        && !Self::is_pinned(entry.panel.as_ref(), cx)
                })
                .map(|(ix, _)| ix)
                .collect();
            for ix in displaced {
                self.close_panel_at(ix, window, cx);
            }
        }
        let insert_at = self
            .open_panels
            .iter()
            .position(|id| self.entry_index(*id).is_some_and(|ix| ix > index))
            .unwrap_or(self.open_panels.len());
        self.open_panels.insert(insert_at, panel_id);
        if let Some(entry) = self.panel_entries.get(index) {
            entry.panel.set_active(true, window, cx);
        }
        true
    }

    /// Closes one panel, leaving its siblings open. Returns the emphasis
    /// that was lost so the caller can decide where focus goes.
    pub fn close_panel_at(
        &mut self,
        panel_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(panel_id) = self
            .panel_entries
            .get(panel_ix)
            .map(|entry| entry.panel.panel_id())
        else {
            return false;
        };
        let Some(open_ix) = self.open_panels.iter().position(|id| *id == panel_id) else {
            return false;
        };
        self.open_panels.remove(open_ix);
        if let Some(entry) = self.panel_entries.get(panel_ix) {
            entry.panel.set_active(false, window, cx);
        }
        let was_focused = self.focused_panel == Some(panel_id);
        if was_focused {
            self.focused_panel = self
                .open_panels
                .get(open_ix.saturating_sub(1))
                .or_else(|| self.open_panels.first())
                .copied();
        }
        cx.notify();
        was_focused
    }

    /// Group toggle for the whole side: `false` closes every open panel and
    /// remembers the set, `true` restores that set (or the focused / first
    /// enabled panel when nothing was remembered). With panels already open,
    /// `true` is a no-op so siblings are never discarded.
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open == self.is_open() {
            return;
        }
        if open {
            let mut to_open: Vec<EntityId> = self
                .collapsed_panels
                .iter()
                .copied()
                .filter(|id| self.entry_index(*id).is_some())
                .collect();
            if to_open.is_empty() {
                let fallback = self
                    .focused_panel
                    .filter(|id| self.entry_index(*id).is_some())
                    .or_else(|| {
                        self.panel_entries
                            .iter()
                            .find(|entry| {
                                entry.panel.enabled(cx)
                                    && !Self::is_hidden(entry.panel.as_ref(), cx)
                            })
                            .map(|entry| entry.panel.panel_id())
                    });
                to_open.extend(fallback);
            }
            self.collapsed_panels.clear();
            for id in to_open {
                self.open_panel_id(id, window, cx);
            }
            if self
                .focused_panel
                .is_none_or(|id| !self.open_panels.contains(&id))
            {
                self.focused_panel = self.open_panels.first().copied();
            }
        } else {
            self.collapsed_panels = self.open_panels.clone();
            for id in std::mem::take(&mut self.open_panels) {
                if let Some(entry) = self.entry_for_id(id) {
                    entry.panel.set_active(false, window, cx);
                }
            }
        }
        cx.notify();
    }

    pub fn set_panel_zoomed(
        &mut self,
        panel: &AnyView,
        zoomed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for entry in &mut self.panel_entries {
            if entry.panel.panel_id() == panel.entity_id() {
                if zoomed != entry.panel.is_zoomed(window, cx) {
                    entry.panel.set_zoomed(zoomed, window, cx);
                }
            } else if entry.panel.is_zoomed(window, cx) {
                entry.panel.set_zoomed(false, window, cx);
            }
        }

        self.workspace
            .update(cx, |workspace, cx| {
                workspace.serialize_workspace(window, cx);
            })
            .ok();
        cx.notify();
    }

    pub fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for entry in &mut self.panel_entries {
            if entry.panel.is_zoomed(window, cx) {
                entry.panel.set_zoomed(false, window, cx);
            }
        }
    }

    pub(crate) fn add_panel<T: Panel>(
        &mut self,
        panel: Entity<T>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let subscriptions =
            [
                cx.observe(&panel, |_, _, cx| cx.notify()),
                cx.observe_global_in::<SettingsStore>(window, {
                    let workspace = workspace.clone();
                    let panel = panel.clone();

                    move |this, window, cx| {
                        // Hidden reconciles every live workspace immediately.
                        // The component's position is deliberately left alone so
                        // restoring it later returns it where it was.
                        if Self::is_hidden_key(T::panel_key(), cx)
                            && let Some(panel_ix) = this.entry_index(Entity::entity_id(&panel))
                        {
                            this.close_panel_at(panel_ix, window, cx);
                        }

                        let new_position = <Entity<T> as PanelHandle>::position(&panel, window, cx);
                        if new_position == this.position {
                            return;
                        }

                        let Ok(new_dock) = workspace.update(cx, |workspace, cx| {
                            if panel.is_zoomed(window, cx) {
                                workspace.zoomed_position = Some(new_position);
                            }
                            match new_position {
                                DockPosition::Left => &workspace.left_dock,
                                DockPosition::Bottom => &workspace.bottom_dock,
                                DockPosition::Right => &workspace.right_dock,
                            }
                            .clone()
                        }) else {
                            return;
                        };

                        let panel_id = Entity::entity_id(&panel);
                        let was_visible = this.is_panel_open(panel_id);
                        let size_state = this
                            .panel_entries
                            .iter()
                            .find(|entry| entry.panel.panel_id() == panel_id)
                            .map(|entry| entry.size_state)
                            .unwrap_or_default();

                        let previous_axis = this.position.axis();
                        let next_axis = new_position.axis();
                        let size_state = if previous_axis == next_axis {
                            size_state
                        } else {
                            PanelSizeState::default()
                        };

                        if !this.remove_panel(&panel, window, cx) {
                            // Panel was already moved from this dock
                            return;
                        }

                        new_dock.update(cx, |new_dock, cx| {
                            let index =
                                new_dock.add_panel(panel.clone(), workspace.clone(), window, cx);
                            if let Some(added_panel) = new_dock.panel_for_id(panel_id).cloned() {
                                new_dock.set_panel_size_state(added_panel.as_ref(), size_state, cx);
                            }
                            if was_visible {
                                new_dock.activate_panel(index, window, cx);
                            }
                        });

                        workspace
                            .update(cx, |workspace, cx| {
                                workspace.serialize_workspace(window, cx);
                            })
                            .ok();
                    }
                }),
                {
                    let panel = panel.clone();
                    let mut last_default_size = panel.read(cx).default_size(window, cx);

                    cx.observe_global_in::<SettingsStore>(window, move |this, window, cx| {
                        let default_size = panel.read(cx).default_size(window, cx);
                        if default_size == last_default_size {
                            return;
                        }
                        last_default_size = default_size;

                        let panel_id = Entity::entity_id(&panel);
                        if let Some(entry) = this
                            .panel_entries
                            .iter_mut()
                            .find(|entry| entry.panel.panel_id() == panel_id)
                        {
                            entry.size_state.size = None;
                            entry.panel.size_state_changed(window, cx);
                            cx.notify();
                        }
                    })
                },
                cx.subscribe_in(
                    &panel,
                    window,
                    move |this, panel, event, window, cx| match event {
                        PanelEvent::ZoomIn => {
                            this.set_panel_zoomed(&panel.to_any(), true, window, cx);
                            if !PanelHandle::panel_focus_handle(panel, cx)
                                .contains_focused(window, cx)
                            {
                                window.focus(&panel.read(cx).activation_focus_handle(cx), cx);
                            }
                            workspace
                                .update(cx, |workspace, cx| {
                                    workspace.zoomed = Some(panel.downgrade().into());
                                    workspace.zoomed_position = Some(
                                        <Entity<T> as PanelHandle>::position(&panel, window, cx),
                                    );
                                    cx.emit(Event::ZoomChanged);
                                })
                                .ok();
                        }
                        PanelEvent::ZoomOut => {
                            this.set_panel_zoomed(&panel.to_any(), false, window, cx);
                            workspace
                                .update(cx, |workspace, cx| {
                                    if workspace.zoomed_position == Some(this.position) {
                                        workspace.zoomed = None;
                                        workspace.zoomed_position = None;
                                        cx.emit(Event::ZoomChanged);
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }
                        PanelEvent::Activate => {
                            if let Some(ix) = this.panel_entries.iter().position(|entry| {
                                entry.panel.panel_id() == Entity::entity_id(panel)
                            }) {
                                this.set_open(true, window, cx);
                                this.activate_panel(ix, window, cx);
                                window.focus(&panel.read(cx).activation_focus_handle(cx), cx);
                            }
                        }
                        PanelEvent::Close => {
                            if let Some(ix) = this.panel_entries.iter().position(|entry| {
                                entry.panel.panel_id() == Entity::entity_id(panel)
                            }) {
                                this.close_panel_at(ix, window, cx);
                            }
                        }
                    },
                ),
            ];

        let index = match self
            .panel_entries
            .binary_search_by_key(&panel.read(cx).activation_priority(), |entry| {
                entry.panel.activation_priority(cx)
            }) {
            Ok(ix) => {
                if cfg!(debug_assertions) {
                    panic!(
                        "Panels `{}` and `{}` have the same activation priority. Each panel must have a unique priority so the status bar order is deterministic.",
                        T::panel_key(),
                        self.panel_entries[ix].panel.panel_key()
                    );
                }
                ix
            }
            Err(ix) => ix,
        };
        let size_state = panel.read(cx).initial_size_state(window, cx);

        self.panel_entries.insert(
            index,
            PanelEntry {
                panel: Arc::new(panel.clone()),
                size_state,
                _subscriptions: subscriptions,
            },
        );

        self.restore_state(window, cx);

        if panel.read(cx).starts_open(window, cx) {
            self.activate_panel(index, window, cx);
            self.set_open(true, window, cx);
        }

        cx.notify();
        index
    }

    /// Arms this dock with saved state for the next `restore_state`.
    ///
    /// Setting the fields directly would leave `serialized_dock_applied` stale
    /// and the dock-level half would never re-apply, so every caller goes here.
    pub(crate) fn set_serialized_state(
        &mut self,
        data: DockData,
        open_components: Option<Vec<String>>,
    ) {
        self.serialized_dock = Some(data);
        self.serialized_open_components = open_components;
        self.serialized_dock_applied = false;
    }

    pub fn restore_state(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(serialized) = self.serialized_dock.clone() {
            // Reopen the whole saved set before activating the focused panel, so
            // that panel takes emphasis without displacing the siblings it is
            // meant to sit beside.
            //
            // Each key is consumed as it is applied. `add_panel` calls this on
            // every registration — including when a panel moves between docks —
            // so a set that stayed armed would reopen components the user had
            // since closed, every time any panel moved.
            if serialized.visible {
                let pending = self.serialized_open_components.take().unwrap_or_default();
                let mut unresolved = Vec::new();
                for panel_key in pending {
                    match self.panel_id_for_panel_key(&panel_key) {
                        Some(panel_id) => {
                            self.open_panel_id(panel_id, window, cx);
                        }
                        // Panels register asynchronously; keep the key so a
                        // later registration can still restore it.
                        None => unresolved.push(panel_key),
                    }
                }
                if !unresolved.is_empty() {
                    self.serialized_open_components = Some(unresolved);
                }
            }

            // The dock-level half applies exactly once. `add_panel` re-runs this
            // on every registration, and `set_open(true)` on a closed dock falls
            // back to opening the first enabled panel — so repeating it reopens a
            // side the user has since closed, every time any panel moves.
            if !self.serialized_dock_applied {
                self.serialized_dock_applied = true;

                if let Some(active_panel) = serialized.active_panel.filter(|_| serialized.visible)
                    && let Some(idx) =
                        self.panel_index_for_persistent_name(active_panel.as_str(), cx)
                {
                    self.activate_panel(idx, window, cx);
                }

                if serialized.zoom
                    && let Some(panel) = self.active_panel()
                {
                    panel.set_zoomed(true, window, cx)
                }
                self.set_open(serialized.visible, window, cx);
            }
            return true;
        }
        false
    }

    pub fn remove_panel<T: Panel>(
        &mut self,
        panel: &Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(panel_ix) = self
            .panel_entries
            .iter()
            .position(|entry| entry.panel.panel_id() == Entity::entity_id(panel))
        {
            let panel_id = Entity::entity_id(panel);
            if self.open_panels.contains(&panel_id) {
                self.close_panel_at(panel_ix, window, cx);
            }
            self.collapsed_panels.retain(|id| *id != panel_id);
            if self.focused_panel == Some(panel_id) {
                self.focused_panel = self.open_panels.first().copied();
            }
            let _ = window;

            self.panel_entries.remove(panel_ix);
            cx.notify();

            true
        } else {
            false
        }
    }

    pub fn panels_len(&self) -> usize {
        self.panel_entries.len()
    }

    pub fn has_agent_panel(&self, cx: &App) -> bool {
        self.panel_entries
            .iter()
            .any(|entry| entry.panel.is_agent_panel(cx))
    }

    /// Opens the panel beside its siblings and gives it the emphasis.
    pub fn activate_panel(&mut self, panel_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel_id) = self
            .panel_entries
            .get(panel_ix)
            .map(|entry| entry.panel.panel_id())
        else {
            return;
        };
        let opened = self.open_panel_id(panel_id, window, cx);
        let refocused = self.focused_panel != Some(panel_id);
        self.focused_panel = Some(panel_id);
        if opened || refocused {
            cx.notify();
        }
    }

    pub fn visible_panel(&self) -> Option<&Arc<dyn PanelHandle>> {
        let entry = self.visible_entry()?;
        Some(&entry.panel)
    }

    pub fn active_panel(&self) -> Option<&Arc<dyn PanelHandle>> {
        let panel_entry = self.active_panel_entry()?;
        Some(&panel_entry.panel)
    }

    /// One representative open panel: the focused one, else the first open.
    fn visible_entry(&self) -> Option<&PanelEntry> {
        if !self.is_open() {
            return None;
        }
        self.active_panel_entry()
            .filter(|entry| self.open_panels.contains(&entry.panel.panel_id()))
            .or_else(|| self.open_entries().into_iter().next())
    }

    pub fn zoomed_panel(&self, window: &Window, cx: &App) -> Option<Arc<dyn PanelHandle>> {
        let entry = self.visible_entry()?;
        if entry.panel.is_zoomed(window, cx) {
            Some(entry.panel.clone())
        } else {
            None
        }
    }

    pub fn active_panel_size(&self) -> Option<PanelSizeState> {
        self.visible_entry().map(|entry| entry.size_state)
    }

    pub fn stored_panel_size(
        &self,
        panel: &dyn PanelHandle,
        window: &Window,
        cx: &App,
    ) -> Option<Pixels> {
        self.panel_entries
            .iter()
            .find(|entry| entry.panel.panel_id() == panel.panel_id())
            .map(|entry| {
                entry
                    .size_state
                    .size
                    .unwrap_or_else(|| entry.panel.default_size(window, cx))
            })
    }

    pub fn stored_panel_size_state(&self, panel: &dyn PanelHandle) -> Option<PanelSizeState> {
        self.panel_entries
            .iter()
            .find(|entry| entry.panel.panel_id() == panel.panel_id())
            .map(|entry| entry.size_state)
    }

    /// What this side claims of the window, for the *opposite* dock's resize
    /// ceiling. Every open column counts, a flexible one by its `default_size`
    /// because it has no stored width to report.
    ///
    /// `open_fixed_size` cannot stand in here. It drops flexible columns rather
    /// than estimating them, so a side holding only a flexible component answers
    /// zero — and `resize_left_dock` reads that as "nothing on the right", which
    /// lets the left dock be dragged clean over the right one. Upstream asks the
    /// active panel for `size.unwrap_or(default_size)`; this is that same formula
    /// generalised from one panel to every open one, which is what the
    /// multi-component model does everywhere else and what this spot missed.
    pub fn stored_active_panel_size(&self, window: &Window, cx: &App) -> Option<Pixels> {
        if !self.is_open() {
            return None;
        }
        match self.position.axis() {
            Axis::Horizontal => Some(
                self.open_entries()
                    .into_iter()
                    .map(|entry| {
                        entry
                            .size_state
                            .size
                            .unwrap_or_else(|| entry.panel.default_size(window, cx))
                    })
                    .fold(Pixels::ZERO, |sum, size| sum + size),
            ),
            Axis::Vertical => self.open_row_size(window, cx),
        }
    }

    pub fn set_panel_size_state(
        &mut self,
        panel: &dyn PanelHandle,
        size_state: PanelSizeState,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(entry) = self
            .panel_entries
            .iter_mut()
            .find(|entry| entry.panel.panel_id() == panel.panel_id())
        {
            entry.size_state = size_state;
            cx.notify();
            true
        } else {
            false
        }
    }

    pub fn toggle_panel_flexible_size(
        &mut self,
        panel: &dyn PanelHandle,
        current_size: Option<Pixels>,
        current_flex: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .panel_entries
            .iter_mut()
            .find(|entry| entry.panel.panel_id() == panel.panel_id())
        else {
            return;
        };
        let currently_flexible = entry.panel.has_flexible_size(window, cx);
        if currently_flexible {
            entry.size_state.size = current_size;
        } else {
            entry.size_state.flex = current_flex;
        }
        let panel_key = entry.panel.panel_key();
        let size_state = entry.size_state;
        let workspace = self.workspace.clone();
        entry
            .panel
            .set_flexible_size(!currently_flexible, window, cx);
        entry.panel.size_state_changed(window, cx);
        cx.defer(move |cx| {
            if let Some(workspace) = workspace.upgrade() {
                workspace.update(cx, |workspace, cx| {
                    workspace.persist_panel_size_state(panel_key, size_state, cx);
                });
            }
        });
        cx.notify();
    }

    /// The editor-facing divider resizes the whole side: `size` and `flex`
    /// describe the dock as one width, distributed across the open panels —
    /// fixed widths scale by the same ratio, flexible factors share `flex`
    /// by their current proportions. `None` resets every open panel.
    pub fn resize_active_panel(
        &mut self,
        size: Option<Pixels>,
        flex: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_open_panels(size, flex, window, cx);
    }

    pub fn resize_all_panels(
        &mut self,
        size: Option<Pixels>,
        flex: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_open_panels(size, flex, window, cx);
    }

    fn resize_open_panels(
        &mut self,
        size: Option<Pixels>,
        flex: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position = self.position;
        let open_indices: Vec<usize> = self
            .panel_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.open_panels.contains(&entry.panel.panel_id()))
            .map(|(ix, _)| ix)
            .collect();
        if open_indices.is_empty() {
            return;
        }

        let fixed_total = self.open_fixed_size(window, cx);
        let fixed_min_total = self.open_fixed_min_size(window, cx);
        let flex_total = self.open_flex(None, window, cx);
        let mut size_states_to_persist = Vec::new();
        for index in open_indices {
            let Some(entry) = self.panel_entries.get_mut(index) else {
                continue;
            };
            let uses_flex = panel_uses_flexible_width(position, entry.panel.as_ref(), window, cx);
            let (entry_size, entry_flex) = match (size, position.axis()) {
                (None, _) => (None, None),
                (Some(total), Axis::Vertical) => (Some(total), None),
                (Some(_), Axis::Horizontal) if uses_flex => {
                    let current = entry.size_state.flex.unwrap_or(1.0);
                    let share = match (flex, flex_total) {
                        (Some(flex), Some(flex_total)) if flex_total > 0.0 => {
                            Some(flex * current / flex_total)
                        }
                        (flex, _) => flex,
                    };
                    (None, share)
                }
                // A fixed column beside a flexible one holds its width: the
                // flexible column absorbs the whole change (Tom, 2026-09-03).
                // The 2026-08-31 "scales every column proportionally" ruling was
                // taken when a side was wholly fixed or wholly flexible, where
                // the two are indistinguishable. Fixed has to mean fixed once
                // they mix, or every drag silently rewrites a width the user
                // pinned — and the internal seam is already the deliberate way
                // to change one.
                (Some(_), Axis::Horizontal) if flex_total.is_some() => {
                    (entry.size_state.size, None)
                }
                (Some(total), Axis::Horizontal) => {
                    let current = entry
                        .size_state
                        .size
                        .unwrap_or_else(|| entry.panel.default_size(window, cx));
                    // Wholly fixed side: each column keeps its own minimum and
                    // shares only what is left over, in the proportions it had.
                    // Scaling the whole width proportionally instead lets a
                    // skewed pair squeeze one column under its minimum while the
                    // side as a whole sits on the floor — the sliver moves
                    // inward rather than closing.
                    let min = effective_min_width_at(position, entry.panel.as_ref(), window, cx);
                    let slack = (total - fixed_min_total).max(Pixels::ZERO);
                    let scaled = if fixed_total > Pixels::ZERO {
                        min + slack * (current / fixed_total)
                    } else {
                        total
                    };
                    (Some(scaled), None)
                }
            };
            size_states_to_persist.push(resize_panel_entry(
                position, entry, entry_size, entry_flex, window, cx,
            ));
        }

        let workspace = self.workspace.clone();
        cx.defer(move |cx| {
            if let Some(workspace) = workspace.upgrade() {
                workspace.update(cx, |workspace, cx| {
                    for (panel_key, size_state) in size_states_to_persist {
                        workspace.persist_panel_size_state(panel_key, size_state, cx);
                    }
                });
            }
        });

        cx.notify();
    }

    pub fn toggle_action(&self) -> Box<dyn Action> {
        match self.position {
            DockPosition::Left => crate::ToggleLeftDock.boxed_clone(),
            DockPosition::Bottom => crate::ToggleBottomDock.boxed_clone(),
            DockPosition::Right => crate::ToggleRightDock.boxed_clone(),
        }
    }

    fn dispatch_context() -> KeyContext {
        let mut dispatch_context = KeyContext::new_with_defaults();
        dispatch_context.add("Dock");

        dispatch_context
    }

    pub fn clamp_panel_size(&mut self, max_size: Pixels, window: &Window, cx: &mut Context<Self>) {
        let max_size = (max_size - RESIZE_HANDLE_SIZE).abs();
        let mut clamped = false;
        for entry in &mut self.panel_entries {
            let uses_flexible_width =
                panel_uses_flexible_width(self.position, entry.panel.as_ref(), window, cx);
            if uses_flexible_width {
                continue;
            }

            let size = entry
                .size_state
                .size
                .unwrap_or_else(|| entry.panel.default_size(window, cx));
            if size > max_size {
                entry.size_state.size = Some(max_size.max(RESIZE_HANDLE_SIZE));
                clamped = true;
            }
        }
        if clamped {
            cx.notify();
        }
    }

    pub(crate) fn load_persisted_size_state(
        workspace: &Workspace,
        panel_key: &'static str,
        cx: &App,
    ) -> Option<PanelSizeState> {
        let workspace_id = workspace
            .database_id()
            .map(|id| i64::from(id).to_string())
            .or(workspace.session_id())?;
        let kvp = KeyValueStore::global(cx);
        let scope = kvp.scoped(PANEL_SIZE_STATE_KEY);
        scope
            .read(&format!("{workspace_id}:{panel_key}"))
            .log_err()
            .flatten()
            .and_then(|json| serde_json::from_str::<PanelSizeState>(&json).log_err())
    }
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dispatch_context = Self::dispatch_context();
        let open_entries = self.open_entries();
        if open_entries.is_empty() {
            return div()
                .id("dock-panel")
                .key_context(dispatch_context)
                .track_focus(&self.focus_handle(cx));
        }
        let position = self.position;
        let border = cx.theme().colors().border;
        let create_resize_handle = || {
            let handle = div()
                .id("resize-handle")
                .on_drag(DraggedDock(position), |dock, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| dock.clone())
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|dock, e: &MouseUpEvent, window, cx| {
                        if e.click_count == 2 {
                            dock.resize_active_panel(None, None, window, cx);
                            dock.workspace
                                .update(cx, |workspace, cx| {
                                    workspace.serialize_workspace(window, cx);
                                })
                                .ok();
                            cx.stop_propagation();
                        }
                    }),
                )
                .occlude();
            match self.position() {
                DockPosition::Left => deferred(
                    handle
                        .absolute()
                        .right(-RESIZE_HANDLE_SIZE / 2.)
                        .top(px(0.))
                        .h_full()
                        .w(RESIZE_HANDLE_SIZE)
                        .cursor_col_resize(),
                ),
                DockPosition::Bottom => deferred(
                    handle
                        .absolute()
                        .top(-RESIZE_HANDLE_SIZE / 2.)
                        .left(px(0.))
                        .w_full()
                        .h(RESIZE_HANDLE_SIZE)
                        .cursor_row_resize(),
                ),
                DockPosition::Right => deferred(
                    handle
                        .absolute()
                        .top(px(0.))
                        .left(-RESIZE_HANDLE_SIZE / 2.)
                        .h_full()
                        .w(RESIZE_HANDLE_SIZE)
                        .cursor_col_resize(),
                ),
            }
        };

        // One handle per seam, straddling the border the column already draws.
        // `deferred` is what lets it sit half outside its own column despite the
        // `overflow_hidden` — the same reason the editor-facing handle uses it.
        // No `cx.listener` here: the map below borrows `cx` too, and nothing this
        // handle does needs `&mut Dock`.
        let seam_is_resizable: Vec<bool> = (0..open_entries.len())
            .map(|index| self.seam_is_resizable(index, window, cx))
            .collect();
        let create_seam_handle = move |index: usize| {
            deferred(
                div()
                    .id(("dock-seam-handle", index))
                    .on_drag(DraggedDockSeam { position, index }, |seam, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| seam.clone())
                    })
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    })
                    .occlude()
                    .absolute()
                    .left(-RESIZE_HANDLE_SIZE / 2.)
                    .top(px(0.))
                    .h_full()
                    .w(RESIZE_HANDLE_SIZE)
                    .cursor_col_resize(),
            )
        };

        // Flexbox distributes only `sum(flex_grow)` of the free space when that sum
        // is below 1, so passing a stored share straight through leaves the rest of
        // the side painted in dock background — 12% of it for two columns storing
        // 0.44 each. The shares are meaningful only relative to each other inside
        // this dock, so normalising them to sum to 1 keeps every column's proportion
        // and makes them claim the whole side (Tom, dogfooding: "when I make the
        // width smaller, the grey area grows").
        let total_flex: f32 = open_entries
            .iter()
            .filter(|entry| panel_uses_flexible_width(position, entry.panel.as_ref(), window, cx))
            .map(|entry| entry.size_state.flex.unwrap_or(1.0).max(0.001))
            .sum();

        let columns: Vec<AnyElement> = open_entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| {
                let panel = entry
                    .panel
                    .to_any()
                    .cached(StyleRefinement::default().v_flex().size_full());
                let mut column = div()
                    .debug_selector({
                        let panel_key = entry.panel.panel_key();
                        move || format!("DOCK-COLUMN-{panel_key}")
                    })
                    .h_full()
                    .overflow_hidden()
                    .child(panel);
                match position.axis() {
                    Axis::Horizontal => {
                        if panel_uses_flexible_width(position, entry.panel.as_ref(), window, cx) {
                            let flex = entry.size_state.flex.unwrap_or(1.0).max(0.001);
                            let style = column.style();
                            style.flex_grow = Some(if total_flex > 0. {
                                flex / total_flex
                            } else {
                                flex
                            });
                            style.flex_shrink = Some(1.0);
                            style.flex_basis = Some(relative(0.).into());
                        } else {
                            let size = entry
                                .size_state
                                .size
                                .unwrap_or_else(|| entry.panel.default_size(window, cx));
                            column = column.w(size).flex_none();
                        }
                        if let Some(min) = entry.panel.min_size(window, cx) {
                            column = column.min_w(min);
                        }
                    }
                    Axis::Vertical => {
                        column = column.flex_1().min_w_0();
                    }
                }
                if ix > 0 {
                    column = column.border_l_1().border_color(border);
                    if seam_is_resizable[ix] {
                        column = column.child(create_seam_handle(ix));
                    }
                }
                column.into_any_element()
            })
            .collect();

        div()
            .id("dock-panel")
            .debug_selector({
                let position = self.position;
                move || format!("DOCK-PANEL-{position:?}")
            })
            .key_context(dispatch_context)
            .track_focus(&self.focus_handle(cx))
            .focus_follows_mouse(self.focus_follows_mouse, cx)
            .flex()
            .flex_row()
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .border_color(border)
            .overflow_hidden()
            .map(|this| match position {
                DockPosition::Left => this.border_r_1(),
                DockPosition::Right => this.border_l_1(),
                DockPosition::Bottom => this.border_t_1(),
            })
            .children(columns)
            .when(self.resizable(cx), |this| {
                this.child(create_resize_handle())
            })
    }
}

impl PanelButtons {
    pub fn new(dock: Entity<Dock>, cx: &mut Context<Self>) -> Self {
        cx.observe(&dock, |_, _, cx| cx.notify()).detach();
        let settings_subscription = cx.observe_global::<SettingsStore>(|_, cx| cx.notify());
        Self {
            dock,
            _settings_subscription: settings_subscription,
        }
    }
}

impl Render for PanelButtons {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dock = self.dock.read(cx);
        let dock_position = dock.position;

        let (menu_anchor, menu_attach) = match dock.position {
            DockPosition::Left => (Anchor::BottomLeft, Anchor::TopLeft),
            DockPosition::Bottom | DockPosition::Right => (Anchor::BottomRight, Anchor::TopRight),
        };

        let dock_entity = self.dock.clone();
        let workspace = dock.workspace.clone();
        let mut buttons: Vec<_> = dock
            .panel_entries
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| {
                // Hidden takes the component and its icon together. Without this
                // the two come apart in the other direction from the legacy
                // `button` key: the component closes everywhere while its icon
                // stays in the bar, offering to open something that is hidden.
                if Dock::is_hidden(entry.panel.as_ref(), cx) {
                    return None;
                }
                let icon = entry.panel.icon(window, cx)?;
                let icon_tooltip = entry
                    .panel
                    .icon_tooltip(window, cx)
                    .ok_or_else(|| {
                        anyhow::anyhow!("can't render a panel button without an icon tooltip")
                    })
                    .log_err()?;
                let name = entry.panel.persistent_name();
                let panel = entry.panel.clone();
                let supports_flexible = panel.supports_flexible_size(cx);
                let currently_flexible = panel.has_flexible_size(window, cx);
                let dock_for_menu = dock_entity.clone();
                let workspace_for_menu = workspace.clone();
                let dock_for_click = dock_entity.clone();
                let workspace_for_click = workspace.clone();

                let panel_id = entry.panel.panel_id();
                let panel_key = entry.panel.panel_key();
                let is_pinned_button = Dock::is_pinned(entry.panel.as_ref(), cx);
                let is_open_button = dock.is_panel_open(panel_id);
                let is_focused_button = is_open_button && dock.focused_panel_id() == Some(panel_id);
                let action = entry.panel.toggle_action(window, cx);
                let tooltip: SharedString = if is_focused_button {
                    format!("Close {}", icon_tooltip).into()
                } else {
                    icon_tooltip.into()
                };

                let icon_label = entry.panel.icon_label(window, cx);

                Some(
                    right_click_menu(name)
                        .menu(move |window, cx| {
                            const POSITIONS: [DockPosition; 3] = [
                                DockPosition::Left,
                                DockPosition::Right,
                                DockPosition::Bottom,
                            ];

                            ContextMenu::build(window, cx, |mut menu, _, cx| {
                                // Every entry here writes through the shared
                                // workspace-layout service. The legacy
                                // `Panel::set_position` and `hide_button_setting`
                                // paths wrote per-panel `dock` and `button` keys
                                // that `effective_component_position` outranks, so
                                // a component with an explicit entry took the
                                // write and did not move — a menu that looked dead
                                // for exactly the components the user had already
                                // placed.
                                // Action entries focus this handle before they
                                // dispatch, and `CustomizeWorkspaceLayout` is
                                // registered on `Workspace` — so the workspace's
                                // own handle is the one that definitely has the
                                // handler above it. Deliberately not the panel's:
                                // this menu opens from an icon that is shown for
                                // closed panels too, and a closed panel's focus
                                // handle need not be in the dispatch tree at all.
                                if let Some(workspace) = workspace_for_menu.upgrade() {
                                    menu = menu.context(workspace.focus_handle(cx));
                                }
                                let mut has_position_entries = false;
                                if POSITIONS
                                    .iter()
                                    .any(|position| panel.position_is_valid(*position, cx))
                                {
                                    menu = menu.header("Workspace Position");
                                }
                                for position in POSITIONS {
                                    if panel.position_is_valid(position, cx) {
                                        let is_current = position == dock_position;
                                        let workspace_for_position = workspace_for_menu.clone();
                                        menu = menu.toggleable_entry(
                                            position.label(),
                                            is_current,
                                            IconPosition::Start,
                                            None,
                                            move |_, cx| {
                                                if is_current {
                                                    return;
                                                }
                                                let Some(workspace) =
                                                    workspace_for_position.upgrade()
                                                else {
                                                    return;
                                                };
                                                let fs = workspace.read(cx).app_state().fs.clone();
                                                set_component_position(
                                                    fs,
                                                    panel_key.to_string(),
                                                    position.into(),
                                                    cx,
                                                );
                                            },
                                        );
                                        has_position_entries = true;
                                    }
                                }
                                if has_position_entries {
                                    menu = menu.separator();
                                }
                                let workspace_for_pin = workspace_for_menu.clone();
                                menu = menu.toggleable_entry(
                                    if is_pinned_button {
                                        "Unpin Panel"
                                    } else {
                                        "Pin Panel"
                                    },
                                    is_pinned_button,
                                    IconPosition::Start,
                                    None,
                                    move |_, cx| {
                                        let Some(workspace) = workspace_for_pin.upgrade() else {
                                            return;
                                        };
                                        let fs = workspace.read(cx).app_state().fs.clone();
                                        // Pinning creates this component's first
                                        // entry, which ends its legacy dock's
                                        // authority. Write where it already is, so
                                        // pinning records a position rather than
                                        // moving the component to its registered one.
                                        seed_component_position(
                                            fs.clone(),
                                            panel_key.to_string(),
                                            dock_position.into(),
                                            cx,
                                        );
                                        set_component_pinned(
                                            fs,
                                            panel_key.to_string(),
                                            !is_pinned_button,
                                            cx,
                                        );
                                    },
                                );
                                if supports_flexible {
                                    menu = menu.separator();
                                    let panel_for_flex = panel.clone();
                                    let dock_for_flex = dock_for_menu.clone();
                                    let workspace_for_flex = workspace_for_menu.clone();
                                    menu = menu.toggleable_entry(
                                        "Flex Width",
                                        currently_flexible,
                                        IconPosition::Start,
                                        None,
                                        move |window, cx| {
                                            if !currently_flexible {
                                                if let Some(ws) = workspace_for_flex.upgrade() {
                                                    ws.update(cx, |workspace, cx| {
                                                        workspace.toggle_dock_panel_flexible_size(
                                                            &dock_for_flex,
                                                            panel_for_flex.as_ref(),
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }
                                            }
                                        },
                                    );
                                    let panel_for_fixed = panel.clone();
                                    let dock_for_fixed = dock_for_menu.clone();
                                    let workspace_for_fixed = workspace_for_menu.clone();
                                    menu = menu.toggleable_entry(
                                        "Fixed Width",
                                        !currently_flexible,
                                        IconPosition::Start,
                                        None,
                                        move |window, cx| {
                                            if currently_flexible {
                                                if let Some(ws) = workspace_for_fixed.upgrade() {
                                                    ws.update(cx, |workspace, cx| {
                                                        workspace.toggle_dock_panel_flexible_size(
                                                            &dock_for_fixed,
                                                            panel_for_fixed.as_ref(),
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }
                                            }
                                        },
                                    );
                                }
                                // `Hidden`, not the legacy `Hide Button`. That
                                // wrote the panel's own `button` key, which hid
                                // the icon while the component stayed open — the
                                // icon-independent-from-component model human
                                // Check 3 disproved. Hiding a component takes the
                                // component and its icon together, everywhere.
                                let workspace_for_hide = workspace_for_menu.clone();
                                menu = menu.separator().entry("Hidden", None, move |_, cx| {
                                    let Some(workspace) = workspace_for_hide.upgrade() else {
                                        return;
                                    };
                                    let fs = workspace.read(cx).app_state().fs.clone();
                                    // Same reason as pinning: hiding creates
                                    // the entry, so record the position first.
                                    seed_component_position(
                                        fs.clone(),
                                        panel_key.to_string(),
                                        dock_position.into(),
                                        cx,
                                    );
                                    set_component_hidden(fs, panel_key.to_string(), true, cx);
                                });
                                // The way back from `Hidden`. Once a component is
                                // hidden its icon is gone, so this menu is gone
                                // with it — the exit has to be offered before it
                                // is needed. Carries `panel_key` so the surface
                                // opens on this component's row rather than at the
                                // top of twenty-one.
                                //
                                // `.context` is what makes it reach: the action is
                                // registered on `Workspace`, and without a focus
                                // handle inside the workspace the menu would
                                // dispatch into its own tree and find no handler.
                                menu = menu.separator().action(
                                    "Customize Workspace Layout…",
                                    Box::new(CustomizeWorkspaceLayout {
                                        item_id: Some(panel_key.to_string()),
                                    }),
                                );
                                menu
                            })
                        })
                        .anchor(menu_anchor)
                        .attach(menu_attach)
                        .trigger(move |is_active, _window, cx| {
                            // Every state the button draws belongs in its element ID, or a
                            // cached element outlives the state that produced it.
                            let button_state = (is_open_button as u64)
                                + 2 * (is_focused_button as u64)
                                + 4 * (is_pinned_button as u64);
                            let button = IconButton::new((name, button_state), icon)
                                .icon_size(IconSize::Small)
                                .toggle_state(is_open_button)
                                .when(is_focused_button, |this| this.icon_color(Color::Accent))
                                .tab_index(0isize)
                                .aria_label(icon_tooltip)
                                .on_click({
                                    let dock = dock_for_click.clone();
                                    let workspace = workspace_for_click.clone();
                                    move |_, window, cx| {
                                        if let Some(workspace) = workspace.upgrade() {
                                            workspace.update(cx, |workspace, cx| {
                                                workspace
                                                    .handle_panel_icon_click(&dock, i, window, cx);
                                            });
                                        }
                                    }
                                })
                                .when(!is_active, |this| {
                                    this.tooltip(move |_window, cx| {
                                        Tooltip::for_action(tooltip.clone(), &*action, cx)
                                    })
                                });

                            div()
                                .relative()
                                .child(button)
                                // `Indicator` is deliberately not used for the pin: it draws a
                                // 6px disc inside a 10px knockout ring, 40% of a 14px glyph,
                                // which reads as a notification stuck onto the icon rather than
                                // a property of it. Accent already means focused, so colour
                                // alone cannot carry this — the mark has to be small.
                                .when(is_pinned_button, |this| {
                                    this.child(
                                        div()
                                            .absolute()
                                            .bottom(px(2.))
                                            .right(px(2.))
                                            .size(px(4.))
                                            .rounded_full()
                                            .bg(Color::Muted.color(cx)),
                                    )
                                })
                                .when_some(
                                    icon_label
                                        .clone()
                                        .filter(|_| !is_open_button)
                                        .and_then(|label| label.parse::<usize>().ok()),
                                    |this, count| this.child(CountBadge::new(count)),
                                )
                        }),
                )
            })
            .collect();

        if dock_position == DockPosition::Right {
            buttons.reverse();
        }

        let has_buttons = !buttons.is_empty();

        h_flex()
            .gap_1()
            .when(
                has_buttons
                    && (dock.position == DockPosition::Bottom
                        || dock.position == DockPosition::Right),
                |this| this.child(Divider::vertical().color(DividerColor::Border)),
            )
            .children(buttons)
            .when(has_buttons && dock.position == DockPosition::Left, |this| {
                this.child(Divider::vertical().color(DividerColor::Border))
            })
    }
}

impl StatusItemView for PanelButtons {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn crate::ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Nothing to do, panel buttons don't depend on the active center item
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        // Panel buttons are hidden on a per-panel basis through each panel
        // button's own context menu.
        None
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod test {
    use super::*;
    use gpui::{App, Context, Window, actions, div};

    pub struct TestPanel {
        pub position: DockPosition,
        pub zoomed: bool,
        pub active: bool,
        pub focus_handle: FocusHandle,
        pub activation_focus_handle: Option<FocusHandle>,
        pub default_size: Pixels,
        pub min_size: Option<Pixels>,
        pub flexible: bool,
        pub activation_priority: u32,
    }
    actions!(test_only, [ToggleTestPanel]);

    impl EventEmitter<PanelEvent> for TestPanel {}

    impl TestPanel {
        pub fn new(position: DockPosition, activation_priority: u32, cx: &mut App) -> Self {
            Self {
                position,
                zoomed: false,
                active: false,
                focus_handle: cx.focus_handle(),
                activation_focus_handle: None,
                default_size: px(300.),
                min_size: None,
                flexible: false,
                activation_priority,
            }
        }

        /// A panel that declares its own minimum, so a test can tell a declared
        /// value being read from the registry floor being applied to everything.
        pub fn new_with_min_size(
            position: DockPosition,
            activation_priority: u32,
            min_size: Pixels,
            cx: &mut App,
        ) -> Self {
            Self {
                min_size: Some(min_size),
                ..Self::new(position, activation_priority, cx)
            }
        }

        pub fn new_flexible(
            position: DockPosition,
            activation_priority: u32,
            cx: &mut App,
        ) -> Self {
            Self {
                flexible: true,
                ..Self::new(position, activation_priority, cx)
            }
        }

        pub fn new_with_activation_child(
            position: DockPosition,
            activation_priority: u32,
            cx: &mut App,
        ) -> Self {
            Self {
                activation_focus_handle: Some(cx.focus_handle()),
                ..Self::new(position, activation_priority, cx)
            }
        }
    }

    impl Render for TestPanel {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // `size_full` for the same reason as `TestPanelC`: so the element's
            // bounds report the space its column gave it rather than its own
            // empty content.
            div()
                .id("test")
                .size_full()
                .debug_selector(|| "TEST-PANEL-ROOT".to_string())
                .track_focus(&self.focus_handle(cx))
                .children(self.activation_focus_handle.iter().map(|focus_handle| {
                    div().id("test-activation-child").track_focus(focus_handle)
                }))
        }
    }

    impl Panel for TestPanel {
        fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
            self.activation_focus_handle
                .clone()
                .unwrap_or_else(|| self.focus_handle(cx))
        }

        fn persistent_name() -> &'static str {
            "TestPanel"
        }

        fn panel_key() -> &'static str {
            "TestPanel"
        }

        fn position(&self, _window: &Window, _: &App) -> super::DockPosition {
            self.position
        }

        fn position_is_valid(&self, _: super::DockPosition) -> bool {
            true
        }

        fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
            self.position = position;
            cx.update_global::<SettingsStore, _>(|_, _| {});
        }

        fn default_size(&self, _window: &Window, _: &App) -> Pixels {
            self.default_size
        }

        // Returned on every axis, unlike the production panels that declare one
        // only on a side. A bottom panel that does declare a minimum is what
        // shows the bottom dock ignoring it rather than having none to ignore.
        fn min_size(&self, _window: &Window, _: &App) -> Option<Pixels> {
            self.min_size
        }

        fn initial_size_state(&self, _window: &Window, _: &App) -> PanelSizeState {
            PanelSizeState {
                size: None,
                flex: None,
            }
        }

        fn supports_flexible_size(&self) -> bool {
            self.flexible
        }

        fn has_flexible_size(&self, _window: &Window, _: &App) -> bool {
            self.flexible
        }

        fn set_flexible_size(
            &mut self,
            flexible: bool,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) {
            self.flexible = flexible;
        }

        fn icon(&self, _window: &Window, _: &App) -> Option<ui::IconName> {
            None
        }

        fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
            None
        }

        fn toggle_action(&self) -> Box<dyn Action> {
            ToggleTestPanel.boxed_clone()
        }

        fn is_zoomed(&self, _window: &Window, _: &App) -> bool {
            self.zoomed
        }

        fn set_zoomed(&mut self, zoomed: bool, _window: &mut Window, _cx: &mut Context<Self>) {
            self.zoomed = zoomed;
        }

        fn set_active(&mut self, active: bool, _window: &mut Window, _cx: &mut Context<Self>) {
            self.active = active;
        }

        fn activation_priority(&self) -> u32 {
            self.activation_priority
        }
    }

    impl Focusable for TestPanel {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }

    /// A second panel type, distinct from [`TestPanel`] only in its identity.
    /// Pinning is keyed by panel type, so proving that a pinned panel is not
    /// displaced by an unpinned one needs two types rather than two instances.
    pub struct TestPanelB {
        pub position: DockPosition,
        pub focus_handle: FocusHandle,
        pub activation_priority: u32,
    }
    actions!(test_only, [ToggleTestPanelB]);

    impl EventEmitter<PanelEvent> for TestPanelB {}

    impl TestPanelB {
        pub fn new(position: DockPosition, activation_priority: u32, cx: &mut App) -> Self {
            Self {
                position,
                focus_handle: cx.focus_handle(),
                activation_priority,
            }
        }
    }

    impl Render for TestPanelB {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().id("test-b").track_focus(&self.focus_handle(cx))
        }
    }

    impl Panel for TestPanelB {
        // Deliberately unequal to `panel_key()`. In production the two identities
        // differ — Project Panel serializes as "Project Panel" and keys layout as
        // "ProjectPanel" — so a fixture where they match cannot tell a lookup
        // keyed on the wrong one from a correct one.
        fn persistent_name() -> &'static str {
            "Test Panel B"
        }

        fn panel_key() -> &'static str {
            "TestPanelB"
        }

        fn position(&self, _window: &Window, _: &App) -> DockPosition {
            self.position
        }

        fn position_is_valid(&self, _: DockPosition) -> bool {
            true
        }

        fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
            self.position = position;
            cx.update_global::<SettingsStore, _>(|_, _| {});
        }

        fn default_size(&self, _window: &Window, _: &App) -> Pixels {
            px(300.)
        }

        fn icon(&self, _window: &Window, _: &App) -> Option<ui::IconName> {
            None
        }

        fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
            None
        }

        fn toggle_action(&self) -> Box<dyn Action> {
            ToggleTestPanelB.boxed_clone()
        }

        fn activation_priority(&self) -> u32 {
            self.activation_priority
        }
    }

    impl Focusable for TestPanelB {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }

    /// A third panel type. Pinning and registration are keyed by panel type, so
    /// three columns on one side needs three types — and three is the smallest
    /// side on which a seam has any column before it to account for.
    pub struct TestPanelC {
        pub position: DockPosition,
        pub focus_handle: FocusHandle,
        pub activation_priority: u32,
        pub flexible: bool,
    }
    actions!(test_only, [ToggleTestPanelC]);

    impl EventEmitter<PanelEvent> for TestPanelC {}

    impl TestPanelC {
        pub fn new(position: DockPosition, activation_priority: u32, cx: &mut App) -> Self {
            Self {
                position,
                focus_handle: cx.focus_handle(),
                activation_priority,
                flexible: false,
            }
        }

        /// A second flexible type, so a side can hold two flexible columns — the
        /// case where changing the fixed total moves every flexible column at
        /// once, not just the one a seam touches.
        pub fn new_flexible(
            position: DockPosition,
            activation_priority: u32,
            cx: &mut App,
        ) -> Self {
            Self {
                flexible: true,
                ..Self::new(position, activation_priority, cx)
            }
        }
    }

    impl Render for TestPanelC {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // `size_full` so this element's bounds report the space the dock's
            // column actually handed the panel. A bare div sizes to its own
            // (empty) content and would report zero however wide the column got,
            // which is the one thing a fill witness must not do.
            div()
                .id("test-c")
                .size_full()
                .debug_selector(|| "TEST-PANEL-C-ROOT".to_string())
                .track_focus(&self.focus_handle(cx))
        }
    }

    impl Panel for TestPanelC {
        fn has_flexible_size(&self, _window: &Window, _: &App) -> bool {
            self.flexible
        }

        fn set_flexible_size(
            &mut self,
            flexible: bool,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) {
            self.flexible = flexible;
        }

        fn persistent_name() -> &'static str {
            "TestPanelC"
        }

        fn panel_key() -> &'static str {
            "TestPanelC"
        }

        fn position(&self, _window: &Window, _: &App) -> DockPosition {
            self.position
        }

        fn position_is_valid(&self, _: DockPosition) -> bool {
            true
        }

        fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
            self.position = position;
            cx.update_global::<SettingsStore, _>(|_, _| {});
        }

        fn default_size(&self, _window: &Window, _: &App) -> Pixels {
            px(300.)
        }

        fn icon(&self, _window: &Window, _: &App) -> Option<ui::IconName> {
            None
        }

        fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
            None
        }

        fn toggle_action(&self) -> Box<dyn Action> {
            ToggleTestPanelC.boxed_clone()
        }

        fn activation_priority(&self) -> u32 {
            self.activation_priority
        }
    }

    impl Focusable for TestPanelC {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }
}

#[cfg(test)]
mod component_position_tests {
    use super::*;

    /// A side-only component, like every panel but Terminal and Debug.
    fn sides_only(position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    #[test]
    fn a_valid_explicit_entry_beats_the_legacy_dock() {
        assert_eq!(
            resolve_component_position(
                Some(DockPosition::Left),
                true,
                Some(DockPosition::Right),
                DockPosition::Right,
                sides_only,
            ),
            DockPosition::Left
        );
    }

    #[test]
    fn an_invalid_explicit_entry_falls_to_the_registered_position_not_the_legacy_dock() {
        // The discriminating case, and the reason this function exists. Asking for
        // Bottom on a side-only component is unusable; falling through to the
        // legacy dock would resurrect a setting the user may have replaced long
        // ago, on the strength of the newer one being wrong.
        let resolved = resolve_component_position(
            Some(DockPosition::Bottom),
            true,
            Some(DockPosition::Right),
            DockPosition::Left,
            sides_only,
        );
        assert_eq!(resolved, DockPosition::Right, "registered position ignored");
        assert_ne!(
            resolved,
            DockPosition::Left,
            "an unusable explicit entry fell through to the legacy dock"
        );
    }

    #[test]
    fn an_entry_whose_position_did_not_parse_still_beats_the_legacy_dock() {
        // The defect this argument exists for. `#[with_fallible_options]` turns a
        // malformed `position` into `None` — the error is recorded and the file
        // still loads — so branching on the position rather than the entry sent a
        // typo down the legacy path and resurrected a setting the 2026-09-01
        // ruling retired. One character was enough, and no witness caught it: the
        // repair-path tests all use a well-formed *unsupported* value, which
        // arrives as `Some`.
        assert_eq!(
            resolve_component_position(
                None,
                true,
                Some(DockPosition::Right),
                DockPosition::Left,
                sides_only,
            ),
            DockPosition::Right,
            "an entry exists, so the registered position answers — not the legacy dock"
        );
    }

    #[test]
    fn with_no_entry_a_usable_legacy_dock_supplies_the_position() {
        assert_eq!(
            resolve_component_position(
                None,
                false,
                Some(DockPosition::Right),
                DockPosition::Left,
                sides_only,
            ),
            DockPosition::Left,
            "the registered position overrode a perfectly usable legacy dock"
        );
    }

    #[test]
    fn with_no_entry_an_unusable_legacy_dock_falls_to_the_registered_position() {
        // Reachable today: a component's own dock setting can name a position it
        // does not support, and the old resolver returned it unchecked.
        assert_eq!(
            resolve_component_position(
                None,
                false,
                Some(DockPosition::Right),
                DockPosition::Bottom,
                sides_only,
            ),
            DockPosition::Right
        );
    }

    #[test]
    fn a_registered_position_the_component_itself_rejects_is_not_used_either() {
        // Two sources of validity: `is_valid` is the live panel's answer, while
        // `registered` comes from the static table. A panel can narrow what it
        // accepts at runtime, and handing back a position it rejects is worse than
        // deferring to where it currently is. Every other fixture here has a valid
        // registered position, so without this case the guard is unwitnessed.
        assert_eq!(
            resolve_component_position(
                Some(DockPosition::Bottom),
                true,
                Some(DockPosition::Right),
                DockPosition::Left,
                |position| position == DockPosition::Left,
            ),
            DockPosition::Left
        );
    }

    #[test]
    fn an_unknown_component_keeps_its_own_position() {
        // The registry knows nothing about an extension's panel, so there is no
        // registered position to fall back to. Preserving what it asked for beats
        // inventing an answer.
        assert_eq!(
            resolve_component_position(
                Some(DockPosition::Bottom),
                true,
                None,
                DockPosition::Left,
                sides_only
            ),
            DockPosition::Left
        );
        assert_eq!(
            resolve_component_position(None, false, None, DockPosition::Bottom, sides_only),
            DockPosition::Bottom
        );
    }

    #[test]
    fn a_bottom_capable_component_keeps_an_explicit_bottom() {
        // Terminal and Debug are the two that accept it; the same entry that is a
        // repair case for a side-only component is simply honoured here.
        assert_eq!(
            resolve_component_position(
                Some(DockPosition::Bottom),
                true,
                Some(DockPosition::Bottom),
                DockPosition::Left,
                |_| true,
            ),
            DockPosition::Bottom
        );
    }

    #[test]
    fn the_registry_agrees_with_the_canonical_table() {
        assert_eq!(
            registered_component_position("ProjectPanel"),
            Some(DockPosition::Right)
        );
        assert_eq!(
            registered_component_position("TerminalPanel"),
            Some(DockPosition::Bottom)
        );
        assert_eq!(
            registered_component_position("agent_panel"),
            Some(DockPosition::Left)
        );
        assert_eq!(
            registered_component_position("cursor_position"),
            None,
            "a status control resolved to a dock"
        );
        assert_eq!(registered_component_position("not_a_component"), None);
    }
}
