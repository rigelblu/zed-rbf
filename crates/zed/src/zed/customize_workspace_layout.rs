//! `Customize Workspace Layout` — the one place every placeable item is visible
//! at once, and the only way to put a hidden one back.
//!
//! Both right-click menus can hide an item in a click, and a hidden item renders
//! nowhere, so there is nothing left to right-click. This surface is the exit.
//! That is why it lists items from the static `layout_registry` rather than from
//! what is currently on screen: a list built from live items could never offer
//! the ones that are missing.

use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Task, WeakEntity, Window,
};
use picker::{Picker, PickerDelegate};
use settings::{Settings as _, SettingsStore, StatusBarItemLocation};
use std::sync::Arc;
use ui::{
    Icon, Label, ListItem, ListItemSpacing, ToggleButtonGroup, ToggleButtonGroupStyle,
    ToggleButtonSimple, prelude::*,
};
use workspace::{
    CustomizeWorkspaceLayout, ModalView, StatusBarSettings, Workspace,
    dock::{DockPosition, set_component_hidden, set_component_position},
    layout_registry::{self, LayoutItem, LayoutItemKind, LayoutPosition},
    set_status_control_location,
};

pub(crate) fn init(cx: &mut App) {
    cx.observe_new(
        |workspace: &mut Workspace, _window: Option<&mut Window>, _cx| {
            workspace.register_action(
                |workspace, action: &CustomizeWorkspaceLayout, window, cx| {
                    let initial = action.item_id.clone();
                    let handle = cx.weak_entity();
                    workspace.toggle_modal(window, cx, move |window, cx| {
                        CustomizeWorkspaceLayoutModal::new(handle, initial, window, cx)
                    });
                },
            );
        },
    )
    .detach();
}

/// One row's worth of choices.
///
/// A status control takes all four bar zones. A component takes only the
/// positions its live panel accepts, plus Hidden — so the surface never offers a
/// placement the component would then refuse. `Hidden` is last in both, because
/// it is the destructive one and reads as the end of a scale.
#[derive(Clone, Copy, PartialEq)]
enum Placement {
    Zone(LayoutPosition),
    Hidden,
}

impl Placement {
    fn label(self) -> &'static str {
        match self {
            Placement::Zone(LayoutPosition::Left) => "Left",
            Placement::Zone(LayoutPosition::Middle) => "Middle",
            Placement::Zone(LayoutPosition::Right) => "Right",
            Placement::Zone(LayoutPosition::Bottom) => "Bottom",
            Placement::Hidden => "Hidden",
        }
    }
}

/// What a row offers, and which of those it is currently on.
///
/// Assembled per render rather than cached: the settings observer rebuilds the
/// list on every change, and a cached placement would be a second source for the
/// state the bar is already showing.
struct RowState {
    choices: Vec<Placement>,
    current: Option<usize>,
}

fn row_state(item: &LayoutItem, workspace: &Workspace, window: &Window, cx: &App) -> RowState {
    match item.kind {
        LayoutItemKind::StatusControl => {
            let choices = vec![
                Placement::Zone(LayoutPosition::Left),
                Placement::Zone(LayoutPosition::Middle),
                Placement::Zone(LayoutPosition::Right),
                Placement::Hidden,
            ];
            // Asked of the bar, never re-derived. `status_control_location`
            // deliberately returns a zone and never `Hidden`, so deriving the
            // row's state from it shows `Left` selected for an item whose legacy
            // setting keeps it out of the bar entirely — two answers for one
            // visible state.
            let current = workspace
                .status_bar()
                .read(cx)
                .effective_location_for(item.id, cx)
                .map(|location| match location {
                    StatusBarItemLocation::Left => 0,
                    StatusBarItemLocation::Middle => 1,
                    StatusBarItemLocation::Right => 2,
                    StatusBarItemLocation::Hidden => 3,
                });
            RowState { choices, current }
        }
        LayoutItemKind::WorkspaceComponent => {
            let adapter = workspace.component_adapter(item.id, cx);
            let mut choices: Vec<Placement> = item
                .valid_positions
                .iter()
                .copied()
                // The live panel's answer narrows the static table's, never
                // widens it: a panel can refuse a position at runtime, and
                // offering one it will refuse is worse than offering fewer.
                .filter(
                    |position| match (adapter.as_ref(), dock_position(*position)) {
                        (Some(adapter), Some(dock)) => adapter.accepts(dock, cx),
                        (None, _) => true,
                        (_, None) => false,
                    },
                )
                .map(Placement::Zone)
                .collect();
            choices.push(Placement::Hidden);

            let hidden = workspace::WorkspaceLayoutSettings::get_global(cx)
                .component(item.id)
                .hidden;
            let current = if hidden {
                choices
                    .iter()
                    .position(|choice| *choice == Placement::Hidden)
            } else {
                adapter.as_ref().and_then(|adapter| {
                    let position = layout_position(adapter.position(window, cx));
                    choices
                        .iter()
                        .position(|choice| *choice == Placement::Zone(position))
                })
            };
            RowState { choices, current }
        }
    }
}

fn dock_position(position: LayoutPosition) -> Option<DockPosition> {
    match position {
        LayoutPosition::Left => Some(DockPosition::Left),
        LayoutPosition::Right => Some(DockPosition::Right),
        LayoutPosition::Bottom => Some(DockPosition::Bottom),
        // Middle is a bar zone. No component sits there, and the registry never
        // lists it among a component's valid positions.
        LayoutPosition::Middle => None,
    }
}

fn layout_position(position: DockPosition) -> LayoutPosition {
    match position {
        DockPosition::Left => LayoutPosition::Left,
        DockPosition::Right => LayoutPosition::Right,
        DockPosition::Bottom => LayoutPosition::Bottom,
    }
}

/// Applies a row's choice through the writer that owns that item kind.
///
/// Components and status controls persist to different settings keys by design,
/// and neither writer touches a legacy key. Un-hiding a component runs the open-row
/// scrub before the settings write, which is what stops a workspace that was
/// dormant across the hide from resurrecting it.
fn apply(item: &LayoutItem, placement: Placement, window: &mut Window, cx: &mut App) {
    match (item.kind, placement) {
        (LayoutItemKind::StatusControl, Placement::Zone(zone)) => {
            let location = match zone {
                LayoutPosition::Left => StatusBarItemLocation::Left,
                LayoutPosition::Middle => StatusBarItemLocation::Middle,
                LayoutPosition::Right => StatusBarItemLocation::Right,
                LayoutPosition::Bottom => return,
            };
            set_status_control_location(item.id, location, window, cx);
        }
        (LayoutItemKind::StatusControl, Placement::Hidden) => {
            set_status_control_location(item.id, StatusBarItemLocation::Hidden, window, cx);
        }
        (LayoutItemKind::WorkspaceComponent, Placement::Zone(zone)) => {
            let Some(position) = dock_position(zone) else {
                return;
            };
            let fs = <dyn fs::Fs>::global(cx);
            // Choosing a position for a hidden component restores it: the
            // placement is what the user asked for, and leaving it hidden would
            // make the choice do nothing visible.
            set_component_hidden(fs.clone(), item.id.to_string(), false, cx);
            set_component_position(fs, item.id.to_string(), position.into(), cx);
        }
        (LayoutItemKind::WorkspaceComponent, Placement::Hidden) => {
            let fs = <dyn fs::Fs>::global(cx);
            set_component_hidden(fs, item.id.to_string(), true, cx);
        }
    }
}

/// Puts every item back to what it registered with — position and hidden state
/// together, so the result is the factory layout rather than a maximally visible
/// one that never existed.
fn restore_defaults(window: &mut Window, cx: &mut App) {
    for item in layout_registry::all() {
        match item.kind {
            LayoutItemKind::StatusControl => {
                let location = if item.default_hidden {
                    StatusBarItemLocation::Hidden
                } else {
                    match item.registered_position {
                        LayoutPosition::Left => StatusBarItemLocation::Left,
                        LayoutPosition::Middle => StatusBarItemLocation::Middle,
                        LayoutPosition::Right => StatusBarItemLocation::Right,
                        LayoutPosition::Bottom => continue,
                    }
                };
                set_status_control_location(item.id, location, window, cx);
            }
            LayoutItemKind::WorkspaceComponent => {
                let fs = <dyn fs::Fs>::global(cx);
                set_component_hidden(fs.clone(), item.id.to_string(), item.default_hidden, cx);
                if let Some(position) = dock_position(item.registered_position) {
                    set_component_position(fs, item.id.to_string(), position.into(), cx);
                }
            }
        }
    }
}

pub struct CustomizeWorkspaceLayoutModal {
    picker: Entity<Picker<CustomizeWorkspaceLayoutDelegate>>,
    _settings_subscription: Subscription,
}

impl CustomizeWorkspaceLayoutModal {
    fn new(
        workspace: WeakEntity<Workspace>,
        initial_item_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = CustomizeWorkspaceLayoutDelegate::new(workspace, initial_item_id);
        let picker = cx.new(|cx| {
            Picker::list(delegate, window, cx)
                .initial_width(rems(52.))
                .embedded()
                .show_scrollbar(true)
        });

        // Rows read their placement from settings, so a write from any source —
        // this surface, either right-click menu, or a hand edit — has to redraw
        // them or the surface becomes the stale second opinion.
        let picker_for_settings = picker.clone();
        let settings_subscription =
            cx.observe_global_in::<SettingsStore>(window, move |_, window, cx| {
                picker_for_settings.update(cx, |picker, cx| {
                    let query = picker.query(cx);
                    picker.delegate.refresh(&query, cx);
                    cx.notify();
                });
                let _ = window;
            });

        Self {
            picker,
            _settings_subscription: settings_subscription,
        }
    }
}

impl Focusable for CustomizeWorkspaceLayoutModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for CustomizeWorkspaceLayoutModal {}
impl ModalView for CustomizeWorkspaceLayoutModal {}

impl Render for CustomizeWorkspaceLayoutModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `elevation_3` is what gives a modal its own opaque surface. Without it
        // the picker draws straight onto the editor behind it.
        v_flex()
            .elevation_3(cx)
            .w(rems(52.))
            .child(self.picker.clone())
    }
}

pub struct CustomizeWorkspaceLayoutDelegate {
    workspace: WeakEntity<Workspace>,
    /// Indices into `layout_registry::all()`, in display order: components
    /// first, then status controls, each alphabetical.
    matches: Vec<usize>,
    selected_index: usize,
}

impl CustomizeWorkspaceLayoutDelegate {
    fn new(workspace: WeakEntity<Workspace>, initial_item_id: Option<String>) -> Self {
        let matches = matching_indices("");
        // Arriving from an item's own menu lands on that item, so the surface
        // opens on the thing the user was already looking at rather than at the
        // top of a list of twenty-two.
        let selected_index = initial_item_id
            .as_deref()
            .and_then(|wanted| {
                matches
                    .iter()
                    .position(|index| layout_registry::all()[*index].id == wanted)
            })
            .unwrap_or(0);
        Self {
            workspace,
            matches,
            selected_index,
        }
    }

    fn refresh(&mut self, query: &str, _cx: &mut App) {
        self.matches = matching_indices(query);
        if self.selected_index >= self.matches.len() {
            self.selected_index = self.matches.len().saturating_sub(1);
        }
    }

    fn item_at(&self, index: usize) -> Option<&'static LayoutItem> {
        let registry_index = *self.matches.get(index)?;
        layout_registry::all().get(registry_index)
    }
}

/// Components first, then status controls, each alphabetical by label.
///
/// Not source-rank order: this list is read by someone hunting for one item by
/// name, where alphabetical is the only ordering that helps. Source rank governs
/// what the bar renders, which is a different question.
fn matching_indices(query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut indices: Vec<usize> = layout_registry::all()
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            query.is_empty()
                || item
                    .search_terms()
                    .any(|term| term.to_lowercase().contains(&query))
        })
        .map(|(index, _)| index)
        .collect();
    indices.sort_by_key(|index| {
        let item = &layout_registry::all()[*index];
        let kind_rank = match item.kind {
            LayoutItemKind::WorkspaceComponent => 0,
            LayoutItemKind::StatusControl => 1,
        };
        (kind_rank, item.label.to_lowercase())
    });
    indices
}

impl PickerDelegate for CustomizeWorkspaceLayoutDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "CustomizeWorkspaceLayout"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Search components and status controls…".into()
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        index: usize,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = index;
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.matches = matching_indices(&query);
        if self.selected_index >= self.matches.len() {
            self.selected_index = self.matches.len().saturating_sub(1);
        }
        cx.notify();
        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, _window: &mut Window, _cx: &mut Context<Picker<Self>>) {
        // Placement is chosen from a row's own buttons, so Enter has nothing to
        // confirm. Left deliberately inert rather than made to guess a choice.
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some("No components or status controls match.".into())
    }

    fn render_footer(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<AnyElement> {
        Some(
            h_flex()
                .w_full()
                .p_2()
                .justify_end()
                .border_t_1()
                .border_color(_cx.theme().colors().border)
                .child(
                    ui::Button::new("restore-default-positions", "Restore Default Positions…")
                        .on_click(|_, window, cx| restore_defaults(window, cx)),
                )
                .into_any_element(),
        )
    }

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let item = self.item_at(index)?;
        let workspace = self.workspace.upgrade()?;
        let state = workspace.read_with(cx, |workspace, cx| row_state(item, workspace, window, cx));

        let mut label = h_flex().min_w_0().flex_1().gap_2();
        if let Some(icon) = item.icon {
            label = label.child(Icon::new(icon).size(IconSize::Small));
        }
        label = label.child(
            v_flex()
                .min_w_0()
                .flex_1()
                .child(Label::new(item.label).truncate())
                .when_some(item.clarification, |this, clarification| {
                    this.child(
                        Label::new(clarification)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    )
                }),
        );

        Some(
            ListItem::new(index)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(label)
                .end_slot(render_placement_choices(item, &state)),
        )
    }
}

/// The choice buttons for one row.
///
/// Both arms use one width. Sizing each group to its own button count
/// right-aligns them to a common edge but leaves their left edges ragged, which
/// reads as misalignment down a list of twenty-two rows.
///
/// `ToggleButtonGroup` is const-generic over its column count, and a component's
/// choices vary — five panels take Left/Right, two also take Bottom — so the
/// arms below are the real shapes rather than padding every row to four.
fn render_placement_choices(item: &'static LayoutItem, state: &RowState) -> AnyElement {
    let id: SharedString = format!("layout-placement-{}", item.id).into();
    let selected = state.current.unwrap_or(usize::MAX);
    let button = |slot: usize| {
        let placement = state.choices[slot];
        ToggleButtonSimple::new(placement.label(), move |_, window, cx| {
            apply(item, placement, window, cx)
        })
        .selected(slot == selected)
    };

    match state.choices.len() {
        3 => ToggleButtonGroup::single_row(id, [button(0), button(1), button(2)])
            .style(ToggleButtonGroupStyle::Outlined)
            .width(rems(19.))
            .into_any_element(),
        4 => ToggleButtonGroup::single_row(id, [button(0), button(1), button(2), button(3)])
            .style(ToggleButtonGroupStyle::Outlined)
            .width(rems(19.))
            .into_any_element(),
        // Every registered item lands on 3 or 4 today: a status control always
        // has four, a component two or three positions plus Hidden. A new item
        // with a different count renders nothing rather than a wrong row, and
        // the registry test that pins every valid-position set is what stops it
        // reaching here unnoticed.
        _ => div().into_any_element(),
    }
}

#[cfg(test)]
mod list_tests {
    use super::*;

    fn labels(query: &str) -> Vec<&'static str> {
        matching_indices(query)
            .into_iter()
            .map(|index| layout_registry::all()[index].label)
            .collect()
    }

    fn kinds(query: &str) -> Vec<LayoutItemKind> {
        matching_indices(query)
            .into_iter()
            .map(|index| layout_registry::all()[index].kind)
            .collect()
    }

    #[test]
    fn an_empty_query_lists_every_registered_item() {
        // The surface exists to show what is missing, so anything that filters
        // by default defeats it. This is also the control for the tests below:
        // a `matching_indices` that returned nothing would satisfy every
        // ordering assertion vacuously.
        assert_eq!(matching_indices("").len(), layout_registry::all().len());
    }

    #[test]
    fn components_come_before_status_controls() {
        let kinds = kinds("");
        let first_control = kinds
            .iter()
            .position(|kind| *kind == LayoutItemKind::StatusControl)
            .expect("the registry holds status controls");
        assert!(
            kinds[..first_control]
                .iter()
                .all(|kind| *kind == LayoutItemKind::WorkspaceComponent),
            "a status control appeared above a component"
        );
        assert!(
            kinds[first_control..]
                .iter()
                .all(|kind| *kind == LayoutItemKind::StatusControl),
            "a component appeared below a status control"
        );
    }

    #[test]
    fn each_group_is_alphabetical() {
        // Not source rank. Source rank governs what the bar renders; this list is
        // read by someone hunting one item by name, where alphabetical is the
        // only ordering that helps.
        for kind in [
            LayoutItemKind::WorkspaceComponent,
            LayoutItemKind::StatusControl,
        ] {
            let group: Vec<String> = matching_indices("")
                .into_iter()
                .map(|index| &layout_registry::all()[index])
                .filter(|item| item.kind == kind)
                .map(|item| item.label.to_lowercase())
                .collect();
            let mut sorted = group.clone();
            sorted.sort();
            assert_eq!(group, sorted, "{kind:?} rows are not alphabetical");
        }
    }

    #[test]
    fn search_reaches_an_alias_the_label_does_not_contain() {
        // The discriminating case. A `matching_indices` that compared only labels
        // passes every other test here — every fixture query above is a label.
        // "vcs" appears in no label in the registry.
        assert_eq!(labels("vcs"), vec!["Git Panel"]);
        assert!(
            !"Git Panel".to_lowercase().contains("vcs"),
            "this fixture only discriminates while the term is absent from the label"
        );
    }

    #[test]
    fn search_ignores_case_and_surrounding_space() {
        assert_eq!(labels("  TERMINAL  "), labels("terminal"));
        assert!(!labels("terminal").is_empty());
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        // The empty state has to be reachable, or `no_matches_text` is dead code
        // that never renders.
        assert!(matching_indices("zzzz-no-such-item").is_empty());
    }
}
