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
    ParentElement, PromptLevel, Render, SharedString, Styled, Subscription, Task, WeakEntity,
    Window,
};
use picker::{Picker, PickerDelegate};
use settings::{Settings as _, SettingsStore, StatusBarItemLocation};
use std::sync::Arc;
use ui::{
    Icon, Label, ListItem, ListItemSpacing, ToggleButtonGroup, ToggleButtonGroupStyle,
    ToggleButtonSimple, prelude::*,
};
use workspace::{
    CustomizeWorkspaceLayout, ModalView, Workspace,
    dock::{
        DockPosition, seed_component_position, set_component_hidden, set_component_pinned,
        set_component_position,
    },
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
    /// The warning for a saved position the component will not take. `None` for a
    /// status control, which accepts every zone it can be given.
    unsupported: Option<SharedString>,
}

/// The row message for a saved position the component cannot take.
///
/// `None` when nothing is saved, or when what is saved is usable. The settings
/// file keeps the value either way — the resolver falls back rather than
/// rewriting it, so this row is the only thing that says the value is being
/// ignored.
fn unsupported_saved_position(
    saved: Option<DockPosition>,
    accepts: impl Fn(DockPosition) -> bool,
    used: DockPosition,
) -> Option<SharedString> {
    let saved = saved?;
    if accepts(saved) {
        return None;
    }
    Some(
        format!(
            "Unsupported saved position; using {}",
            Placement::Zone(layout_position(used)).label()
        )
        .into(),
    )
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
            RowState {
                choices,
                current,
                unsupported: None,
            }
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
            let unsupported = adapter.as_ref().and_then(|adapter| {
                unsupported_saved_position(
                    workspace::WorkspaceLayoutSettings::get_global(cx)
                        .component(item.id)
                        .position
                        .map(DockPosition::from),
                    |position| adapter.accepts(position, cx),
                    adapter.position(window, cx),
                )
            });
            RowState {
                choices,
                current,
                unsupported,
            }
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
fn apply(
    item: &LayoutItem,
    placement: Placement,
    current: Option<Placement>,
    window: &mut Window,
    cx: &mut App,
) {
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
            set_component_hidden(fs.clone(), item.id.to_string(), false, window, cx);
            set_component_position(fs, item.id.to_string(), position.into(), window, cx);
        }
        (LayoutItemKind::WorkspaceComponent, Placement::Hidden) => {
            let fs = <dyn fs::Fs>::global(cx);
            // Hiding creates this component's first entry if it has none, and an
            // entry ends its legacy dock's authority. Record where it is now, so
            // un-hiding later — including by hand-editing `hidden` back to false —
            // returns it where it was rather than to its registered position.
            if let Some(Placement::Zone(zone)) = current
                && let Some(position) = dock_position(zone)
            {
                seed_component_position(fs.clone(), item.id.to_string(), position.into(), cx);
            }
            set_component_hidden(fs, item.id.to_string(), true, window, cx);
        }
    }
}

/// Puts every item back to what it registered with — position and hidden state
/// together, so the result is the factory layout rather than a maximally visible
/// one that never existed.
/// The one destructive click in this surface. It rewrites every registered item's
/// position, hidden state and pinned state at once, and the writes are sequential
/// by decision (2026-09-04) — so there is no transaction to roll back and a misclick
/// is unwindable only by hand. The prompt is the whole guard.
fn confirm_then_restore_defaults(window: &mut Window, cx: &mut App) {
    let answer = window.prompt(
        PromptLevel::Warning,
        "Restore default positions?",
        Some(
            "Every workspace component and status control returns to where it shipped, \
             including its hidden and pinned state. This cannot be undone.",
        ),
        &["Restore Defaults", "Cancel"],
        cx,
    );
    window
        .spawn(cx, async move |cx| {
            if answer.await.ok() == Some(0) {
                cx.update(|window, cx| restore_defaults(window, cx)).ok();
            }
        })
        .detach();
}

fn restore_defaults(window: &mut Window, cx: &mut App) {
    // The icon sequence is part of "where it shipped". Leaving it standing would
    // let the one action that promises the factory layout return every component
    // to its registered side and still draw the strips in the user's order.
    workspace::dock::set_component_order(
        <dyn fs::Fs>::global(cx),
        None,
        "Failed to restore the icon order".into(),
        window,
        cx,
    );
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
                set_component_hidden(
                    fs.clone(),
                    item.id.to_string(),
                    item.default_hidden,
                    window,
                    cx,
                );
                set_component_pinned(
                    fs.clone(),
                    item.id.to_string(),
                    item.default_pinned,
                    window,
                    cx,
                );
                if let Some(position) = dock_position(item.registered_position) {
                    set_component_position(fs, item.id.to_string(), position.into(), window, cx);
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

        // `Picker` never scrolls to the index a delegate starts with:
        // `render_element_container` builds the list from `match_count` alone and
        // consults `selected_index` nowhere, so only `set_selected_index` moves the
        // viewport. Without this, arriving from an item's own menu would select a
        // row below the fold and show the top of the list — the surface would open
        // looking exactly as if no item had been named, which is the whole reason
        // the entry points pass one.
        let initial_index = picker.read(cx).delegate.selected_index;
        picker.update(cx, |picker, cx| {
            picker.set_selected_index(initial_index, None, true, window, cx);
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
                        .on_click(|_, window, cx| confirm_then_restore_defaults(window, cx)),
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
                })
                .when_some(state.unsupported.clone(), |this, unsupported| {
                    this.child(
                        Label::new(unsupported)
                            .size(LabelSize::Small)
                            .color(Color::Warning)
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
    // The group must be told which slot is current, rather than each button being
    // told whether it is: `ToggleButtonGroup` styles a button when
    // `entry_index == self.selected_index || selected`, and `selected_index`
    // defaults to 0. Driving this per button leaves that default in place, so slot
    // 0 lights on every row *in addition to* the real answer — which only looks
    // intermittent because the rows whose answer is slot 0 then appear correct.
    //
    // `usize::MAX` is how "nothing is current" is expressed: the comparison is a
    // plain `==`, so no slot matches and no button lights. A row can genuinely have
    // no current placement — `effective_location_for` returns `None` for a
    // registered control that this workspace's bar never instantiated.
    let selected = state.current.unwrap_or(usize::MAX);
    let current = state
        .current
        .and_then(|slot| state.choices.get(slot).copied());
    let button = |slot: usize| {
        let placement = state.choices[slot];
        ToggleButtonSimple::new(placement.label(), move |_, window, cx| {
            apply(item, placement, current, window, cx)
        })
    };

    match state.choices.len() {
        3 => ToggleButtonGroup::single_row(id, [button(0), button(1), button(2)])
            .selected_index(selected)
            .style(ToggleButtonGroupStyle::Outlined)
            .width(rems(19.))
            .into_any_element(),
        4 => ToggleButtonGroup::single_row(id, [button(0), button(1), button(2), button(3)])
            .selected_index(selected)
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

    /// A saved position the component cannot take used to be swallowed whole: the
    /// resolver fell back, the file kept the value, and nothing anywhere said so.
    /// A token that does not parse at all is a different case, already reported by
    /// the settings machinery as `Invalid user settings file`.
    #[test]
    fn a_saved_position_the_component_refuses_says_what_is_used_instead() {
        let sides_only = |position| position != DockPosition::Bottom;

        assert_eq!(
            unsupported_saved_position(Some(DockPosition::Bottom), sides_only, DockPosition::Left)
                .as_deref(),
            Some("Unsupported saved position; using Left"),
        );
        assert_eq!(
            unsupported_saved_position(Some(DockPosition::Right), sides_only, DockPosition::Right),
            None,
            "a saved position the component accepts is not a problem to report"
        );
        assert_eq!(
            unsupported_saved_position(None, sides_only, DockPosition::Left),
            None,
            "and a component with nothing saved has nothing to warn about"
        );
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        // The empty state has to be reachable, or `no_matches_text` is dead code
        // that never renders.
        assert!(matching_indices("zzzz-no-such-item").is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use workspace::WorkspaceLayoutSettings;

    /// Seeding is what makes the entry-presence rule safe. Once a component has an
    /// entry, its legacy `<panel>.dock` no longer applies — so a first pin or hide
    /// that created a bare entry would have relocated the component to its
    /// registered position. Writing its current position down first turns that move
    /// into a record. The second half matters as much: seeding must not overwrite a
    /// position the user already chose.
    #[gpui::test]
    async fn test_seeding_records_a_position_once_and_never_overwrites_one(
        cx: &mut TestAppContext,
    ) {
        let app_state = crate::zed::tests::init_test(cx);
        let fs = app_state.fs.clone();
        let key = workspace::layout_registry::ids::PROJECT_PANEL;

        cx.update(|cx| {
            assert!(
                !workspace::WorkspaceLayoutSettings::get_global(cx).has_component(key),
                "the fixture must start with no entry, or this witness proves nothing"
            );
            workspace::dock::seed_component_position(
                fs.clone(),
                key.to_string(),
                settings::DockPosition::Left,
                cx,
            );
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                workspace::WorkspaceLayoutSettings::get_global(cx)
                    .component(key)
                    .position,
                Some(settings::DockPosition::Left),
                "seeding records the position it was given"
            );
            workspace::dock::seed_component_position(
                fs.clone(),
                key.to_string(),
                settings::DockPosition::Right,
                cx,
            );
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                workspace::WorkspaceLayoutSettings::get_global(cx)
                    .component(key)
                    .position,
                Some(settings::DockPosition::Left),
                "and leaves an existing entry alone"
            );
        });
    }

    /// Same gap, one field later. `Restore Default Positions…` promises the
    /// layout each item shipped with, and an icon sequence is part of where a
    /// component sits — a reset that returned every component to its registered
    /// side while the strips kept the user's order would be the factory layout in
    /// name only. Written with the reset, not after it, because the pin above is
    /// what this class of defect looks like when it is found late.
    #[gpui::test]
    async fn test_restoring_defaults_clears_the_icon_order(cx: &mut TestAppContext) {
        let app_state = crate::zed::tests::init_test(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        let fs = app_state.fs.clone();

        window
            .update(cx, |_, window, cx| {
                workspace::dock::set_component_order(
                    fs.clone(),
                    Some(vec![
                        workspace::layout_registry::ids::DEBUG_PANEL.to_string(),
                        workspace::layout_registry::ids::AGENT_PANEL.to_string(),
                    ]),
                    "Failed to move".into(),
                    window,
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                WorkspaceLayoutSettings::get_global(cx)
                    .component_order
                    .len(),
                2,
                "the fixture must actually set an order, or this witness proves nothing"
            );
        });

        window
            .update(cx, |_, window, cx| restore_defaults(window, cx))
            .unwrap();
        cx.run_until_parked();

        cx.update(|cx| {
            assert!(
                WorkspaceLayoutSettings::get_global(cx)
                    .component_order
                    .is_empty(),
                "the reset hands every strip back to its registered order"
            );
        });
    }

    /// `Restore Default Positions…` promised position, hidden state *and* pinned
    /// state, and delivered the first two. `set_component_pinned` existed the whole
    /// time and was never reached from this surface, so a reset left every pin
    /// standing — the one thing a user pins deliberately and would most expect a
    /// reset to clear. Dogfood check B4-6 marked reset PASS with nothing pinned on
    /// screen, which is why the gap survived a human check as well as the suite.
    #[gpui::test]
    async fn test_restoring_defaults_clears_a_pin(cx: &mut TestAppContext) {
        let app_state = crate::zed::tests::init_test(cx);
        let window = cx.add_window(|_, _| gpui::Empty);
        let fs = app_state.fs.clone();

        window
            .update(cx, |_, window, cx| {
                workspace::dock::set_component_pinned(
                    fs.clone(),
                    workspace::layout_registry::ids::PROJECT_PANEL.to_string(),
                    true,
                    window,
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(
                WorkspaceLayoutSettings::get_global(cx)
                    .component(workspace::layout_registry::ids::PROJECT_PANEL)
                    .pinned,
                "the fixture must actually pin something, or this witness proves nothing"
            );
        });

        window
            .update(cx, |_, window, cx| restore_defaults(window, cx))
            .unwrap();
        cx.run_until_parked();

        cx.update(|cx| {
            assert!(
                !WorkspaceLayoutSettings::get_global(cx)
                    .component(workspace::layout_registry::ids::PROJECT_PANEL)
                    .pinned,
                "the reset clears the pin"
            );
        });
    }
}
