//! The canonical inventory of items `Customize Workspace Layout` can place.
//!
//! Two things about the shape here are easy to get backwards.
//!
//! **The table is static rather than populated by registration**, unlike the three
//! registries in `workspace.rs` (`ProjectItemRegistry`, `FollowableViewRegistry`,
//! `SerializableItemRegistry`). Panels are added asynchronously, and a *hidden*
//! component is by definition not open, so a surface driven only by registration
//! could never offer to un-hide one — the recovery path the feature exists for.
//!
//! **It lives here rather than in `zed` beside the surface that renders it**, because
//! it also supplies the per-area minimum widths that `Dock`'s resize path consumes.
//! Nothing in the table needs a panel type: ids are the `Panel::panel_key()` strings
//! the settings already store, and `IconName` is its only dependency.
//!
//! Live state — the panel handle, its toggle action, whether it is open or focused —
//! is deliberately absent. That belongs to the adapter, is bound at runtime, and will
//! follow the registration idiom this table does not.

use gpui::{Pixels, px};
use ui::IconName;

/// A component's effective minimum is its `Panel::min_size()` when that is `Some`;
/// these apply when it is `None`. Without them `None` means the 6px sliver
/// `resize_panel_entry` currently allows, which the multi-component model cannot
/// inherit — a column that narrow is unreadable rather than merely small.
/// Dogfood-tunable.
pub const SIDE_COLUMN_MIN_WIDTH: Pixels = px(200.);
pub const BOTTOM_SEGMENT_MIN_WIDTH: Pixels = px(160.);

/// The width below which the editor stops being an editor. Held by the flex item
/// that sits between the side docks, because that is the item flexbox shrinks.
///
/// The brief said twice not to add this — Q8 (*"this slice introduces no editor
/// minimum width"*) and Flow 4 (*"do not add an editor minimum"*) — and dogfooding
/// on 2026-09-05 falsified both. Every side column carries `min_w`; the editor
/// carried nothing and its chain sets `min-width: 0` through `overflow_hidden`, so
/// flexbox drained the editor to a sliver to pay for the columns, and when that ran
/// out the dock overflowed the window and its columns rendered off-screen. Tom,
/// looking at it: *"we don't apply the MIN 200 on the editor pane. I'm guessing we
/// should."* Per-area scrolling needs a stable viewport to scroll inside, and there
/// is no stable viewport while one neighbour can be squeezed to nothing.
pub const CENTER_MIN_WIDTH: Pixels = px(200.);

/// The height below which the editor stops being an editor. Held by the flex item
/// that holds the centre, because that is the item flexbox shrinks when the bottom
/// dock grows.
///
/// **`CENTER_MIN_WIDTH` on the other axis, and it exists for the same reason.**
/// Dogfooding on 2026-09-05 found the editor with no minimum width while every side
/// column had one, so flexbox drained the editor to pay for them. The fix went onto
/// the four `bottom_dock_layout` geometries as `min_w`, and the bottom dock resizes
/// on *height* — so the identical hole stayed open on the axis that session never
/// touched. A drag of the bottom dock's divider could take the editor to nothing.
///
/// **Derived from the editor's own chrome, not from the constant below it.** A tab
/// bar is `Tab::container_height`, which is `DynamicSpacing::Base32` — 32px at
/// default density — and four lines at default line height is roughly 80px more.
/// That it lands on the same figure as `BOTTOM_DOCK_MIN_HEIGHT` is a coincidence of
/// two independent sums, not one value copied: the trap that constant's own comment
/// names is picking a number because it sits nearby, and 120 here would be just as
/// wrong if the arithmetic said otherwise.
///
/// Dogfood-tunable, like the other three.
pub const CENTER_MIN_HEIGHT: Pixels = px(120.);

/// The height below which the bottom dock stops being usable — a tab strip and a
/// couple of terminal rows, and nothing more.
///
/// **Its own constant, and not derived from anything.** The bottom dock resizes on
/// height while `BOTTOM_SEGMENT_MIN_WIDTH` is a segment *width* governing the row's
/// internal dividers; reaching for that number because it sits nearby would be
/// picking a value by its name rather than its meaning. Three figures were
/// defensible on 2026-09-02 and the arithmetic was never the question — 160px
/// matches the segment width, but that symmetry is coincidence; 260px keeps the
/// ratio the 200px side floor has to the 240px side default. Tom took ~120px: the
/// floor closes the trapdoor and does nothing else.
///
/// **No per-panel override, unlike the side floors.** `Panel::min_size` is one
/// number with no axis, and it is already read as a width at Bottom by
/// `effective_min_width_at`. Agent's implementation says as much itself — it
/// returns `Some(MIN_PANEL_WIDTH)` at the sides and `None` at Bottom. A bottom
/// component that wants a taller floor has no way to ask for one today.
///
/// Dogfood-tunable, like the other two.
pub const BOTTOM_DOCK_MIN_HEIGHT: Pixels = px(120.);

/// The stable id of every registered item, so a call site and the table cannot
/// disagree: the table below is written in terms of these, so each id exists in
/// exactly one place. Name items through these constants rather than by literal —
/// an unknown id is preserved rather than rejected, by design, which makes a typo
/// indistinguishable from an item whose crate has not loaded.
pub mod ids {
    pub const AGENT_PANEL: &str = "agent_panel";
    /// The threads sidebar, which is not a `Panel` and so has no `panel_key()` to
    /// transcribe — this literal is the id, and `v0.45.1` is where it acquires one.
    pub const THREADS_SIDEBAR: &str = "threads_sidebar";
    pub const PROJECT_PANEL: &str = "ProjectPanel";
    pub const TERMINAL_PANEL: &str = "TerminalPanel";
    pub const GIT_PANEL: &str = "GitPanel";
    pub const COLLABORATION_PANEL: &str = "CollaborationPanel";
    pub const OUTLINE_PANEL: &str = "OutlinePanel";
    pub const DEBUG_PANEL: &str = "DebugPanel";
    pub const PROJECT_SEARCH: &str = "project_search";
    pub const LSP_SERVERS: &str = "lsp_servers";
    pub const DIAGNOSTICS: &str = "diagnostics";
    pub const ACTIVE_FILE: &str = "active_file";
    pub const GIT_BLAME: &str = "git_blame";
    pub const MERGE_CONFLICTS: &str = "merge_conflicts";
    pub const ACTIVITY: &str = "activity";
    pub const EDIT_PREDICTION: &str = "edit_prediction";
    pub const ACTIVE_ENCODING: &str = "active_encoding";
    pub const ACTIVE_LANGUAGE: &str = "active_language";
    pub const ACTIVE_TOOLCHAIN: &str = "active_toolchain";
    pub const VIM_MODE: &str = "vim_mode";
    pub const CURSOR_POSITION: &str = "cursor_position";
    pub const IMAGE_INFO: &str = "image_info";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutPosition {
    Left,
    Middle,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutItemKind {
    WorkspaceComponent,
    StatusControl,
}

/// Where an item sits in the canonical order. Panels come before status controls,
/// and each keeps the ordering it already had: panels their activation priorities,
/// status controls their registration order. The gap at `Panel(4)` is real — it is
/// preserved so a later reader does not "fix" the sequence by renumbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceRank {
    Panel(u16),
    StatusControl(u16),
}

/// Opens the layout surface — the one place a user can see every placeable item
/// and put a hidden one back.
///
/// `item_id` focuses the row that opened it, so arriving from an item's own menu
/// lands on that item rather than at the top of a list of twenty-two.
#[derive(Clone, Default, PartialEq, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = workspace)]
#[serde(deny_unknown_fields)]
pub struct CustomizeWorkspaceLayout {
    #[serde(default)]
    pub item_id: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutItem {
    /// For a workspace component this is its `Panel::panel_key()`. Keys are not
    /// uniformly cased — `agent_panel` is snake_case where every other component
    /// is PascalCase — so they are transcribed from the source, never derived.
    pub id: &'static str,
    pub kind: LayoutItemKind,
    /// Whether the component's surface is a dock column, so `PanelButtons` draws
    /// its icon and `component_order` sequences it.
    ///
    /// Not every workspace component is dock-hosted: Threads Sidebar occupies the
    /// window-level sidebar slot, and its bar icon is a zone item ordered by
    /// `status_bar.item_order` like any other. Putting it in the dock sequence
    /// would list an id there that draws nothing in that strip.
    ///
    /// Meaningless for a status control, which hosts nothing — the same shape as
    /// `default_pinned` and an empty `valid_positions`. `false` is honest for one
    /// rather than a placeholder.
    pub dock_hosted: bool,
    pub label: &'static str,
    /// Absent where the item renders as text and has no icon of its own. For the
    /// items whose live icon varies with state, this is the representative one the
    /// settings list shows; the item keeps ownership of what it actually renders.
    pub icon: Option<IconName>,
    /// Alternative terms that should match this item in search, beyond its label.
    pub aliases: &'static [&'static str],
    /// Shown beside the label where the label alone is ambiguous.
    pub clarification: Option<&'static str>,
    pub registered_position: LayoutPosition,
    pub default_hidden: bool,
    /// Meaningless for a status control, which cannot be pinned — the same shape
    /// as `valid_positions` being empty for one. It exists so `Restore Default
    /// Positions…` restores pinning from data rather than assuming every
    /// component ships unpinned; nothing does today, and the day one does, this
    /// row is the single place that has to change.
    pub default_pinned: bool,
    pub source_rank: SourceRank,
    /// Empty for a status control: every one of them accepts the three bar zones.
    /// Hidden is not a position here — it is a separate flag, matching the shape
    /// `WorkspaceLayoutSettings` already stores.
    pub valid_positions: &'static [LayoutPosition],
}

const SIDES: &[LayoutPosition] = &[LayoutPosition::Left, LayoutPosition::Right];
const SIDES_AND_BOTTOM: &[LayoutPosition] = &[
    LayoutPosition::Left,
    LayoutPosition::Bottom,
    LayoutPosition::Right,
];
const BAR_ZONES: &[LayoutPosition] = &[
    LayoutPosition::Left,
    LayoutPosition::Middle,
    LayoutPosition::Right,
];

const ITEMS: &[LayoutItem] = &[
    LayoutItem {
        id: ids::AGENT_PANEL,
        kind: LayoutItemKind::WorkspaceComponent,
        dock_hosted: true,
        label: "Agent Panel",
        icon: Some(IconName::ZedAssistant),
        aliases: &["assistant", "ai", "chat"],
        clarification: None,
        registered_position: LayoutPosition::Left,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::Panel(0),
        valid_positions: SIDES,
    },
    LayoutItem {
        id: ids::PROJECT_PANEL,
        kind: LayoutItemKind::WorkspaceComponent,
        dock_hosted: true,
        label: "Project Panel",
        icon: Some(IconName::FileTree),
        aliases: &["files", "file tree", "explorer", "sidebar"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::Panel(1),
        valid_positions: SIDES,
    },
    LayoutItem {
        id: ids::TERMINAL_PANEL,
        kind: LayoutItemKind::WorkspaceComponent,
        dock_hosted: true,
        label: "Terminal Panel",
        icon: Some(IconName::TerminalAlt),
        aliases: &["shell", "console"],
        clarification: None,
        registered_position: LayoutPosition::Bottom,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::Panel(2),
        valid_positions: SIDES_AND_BOTTOM,
    },
    LayoutItem {
        id: ids::GIT_PANEL,
        kind: LayoutItemKind::WorkspaceComponent,
        dock_hosted: true,
        label: "Git Panel",
        icon: Some(IconName::GitBranch),
        aliases: &["source control", "version control", "vcs"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::Panel(3),
        valid_positions: SIDES,
    },
    LayoutItem {
        id: ids::COLLABORATION_PANEL,
        kind: LayoutItemKind::WorkspaceComponent,
        dock_hosted: true,
        label: "Collaboration Panel",
        icon: Some(IconName::UserGroup),
        aliases: &["collab", "contacts", "channels"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::Panel(5),
        valid_positions: SIDES,
    },
    LayoutItem {
        id: ids::OUTLINE_PANEL,
        kind: LayoutItemKind::WorkspaceComponent,
        dock_hosted: true,
        label: "Outline Panel",
        icon: Some(IconName::ListTree),
        aliases: &["symbols", "structure"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::Panel(6),
        valid_positions: SIDES,
    },
    LayoutItem {
        id: ids::DEBUG_PANEL,
        kind: LayoutItemKind::WorkspaceComponent,
        dock_hosted: true,
        label: "Debug Panel",
        icon: Some(IconName::Debug),
        aliases: &["debugger", "breakpoints"],
        clarification: None,
        registered_position: LayoutPosition::Bottom,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::Panel(7),
        valid_positions: SIDES_AND_BOTTOM,
    },
    LayoutItem {
        id: ids::THREADS_SIDEBAR,
        kind: LayoutItemKind::WorkspaceComponent,
        dock_hosted: false,
        label: "Threads Sidebar",
        icon: Some(IconName::ThreadsSidebarLeftClosed),
        aliases: &["threads", "agent", "ai", "conversations"],
        clarification: None,
        registered_position: LayoutPosition::Left,
        default_hidden: false,
        default_pinned: false,
        // `Panel(8)` rather than a `StatusControl` rank: the variant encodes what
        // an item *is*, not which render path draws it, and the table's own
        // ordering law is components before controls. It takes 8 rather than the
        // gap at 4 — that gap transcribes upstream's activation priorities and
        // filling it would misreport them. `add_panel` sorts `panel_entries` by
        // this and never sees this item, which lives in the window-level sidebar
        // slot; the number is inert there and load-bearing only for the bar.
        source_rank: SourceRank::Panel(8),
        valid_positions: SIDES,
    },
    LayoutItem {
        id: ids::PROJECT_SEARCH,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Project Search",
        icon: Some(IconName::MagnifyingGlass),
        aliases: &["find", "search"],
        clarification: None,
        registered_position: LayoutPosition::Left,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(0),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::LSP_SERVERS,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Language Servers",
        icon: Some(IconName::BoltOutlined),
        aliases: &["lsp", "language server"],
        clarification: None,
        registered_position: LayoutPosition::Left,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(1),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::DIAGNOSTICS,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Diagnostics",
        icon: Some(IconName::Warning),
        aliases: &["errors", "warnings", "problems"],
        clarification: None,
        registered_position: LayoutPosition::Left,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(2),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::ACTIVE_FILE,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Active File Name",
        icon: None,
        aliases: &["filename", "current file", "breadcrumb"],
        clarification: None,
        registered_position: LayoutPosition::Left,
        default_hidden: true,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(3),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::GIT_BLAME,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Git Blame",
        icon: None,
        aliases: &["blame", "authorship"],
        clarification: None,
        registered_position: LayoutPosition::Left,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(4),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::MERGE_CONFLICTS,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Merge Conflicts",
        icon: Some(IconName::GitMergeConflict),
        aliases: &["conflicts", "merge"],
        clarification: None,
        registered_position: LayoutPosition::Left,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(5),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::ACTIVITY,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Activity",
        icon: None,
        aliases: &["progress", "indexing", "downloads"],
        clarification: None,
        registered_position: LayoutPosition::Left,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(6),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::EDIT_PREDICTION,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Edit Prediction",
        icon: None,
        aliases: &["copilot", "completion", "inline prediction"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(7),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::ACTIVE_ENCODING,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Active Encoding",
        icon: None,
        aliases: &["charset", "utf-8", "encoding"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(8),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::ACTIVE_LANGUAGE,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Active Language",
        icon: None,
        aliases: &["syntax", "language mode"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(9),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::ACTIVE_TOOLCHAIN,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Active Toolchain",
        icon: None,
        aliases: &["interpreter", "sdk", "virtualenv"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(10),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::VIM_MODE,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Vim Mode",
        icon: None,
        aliases: &["vim", "modal"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(12),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::CURSOR_POSITION,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Cursor Position",
        icon: None,
        aliases: &["line", "column", "caret"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(13),
        valid_positions: BAR_ZONES,
    },
    LayoutItem {
        id: ids::IMAGE_INFO,
        kind: LayoutItemKind::StatusControl,
        dock_hosted: false,
        label: "Image Information",
        icon: None,
        aliases: &["dimensions", "image size"],
        clarification: None,
        registered_position: LayoutPosition::Right,
        default_hidden: false,
        default_pinned: false,
        source_rank: SourceRank::StatusControl(14),
        valid_positions: BAR_ZONES,
    },
];

/// Every registered item, in canonical order.
pub fn all() -> &'static [LayoutItem] {
    ITEMS
}

pub fn get(id: &str) -> Option<&'static LayoutItem> {
    ITEMS.iter().find(|item| item.id == id)
}

pub fn of_kind(kind: LayoutItemKind) -> impl Iterator<Item = &'static LayoutItem> {
    ITEMS.iter().filter(move |item| item.kind == kind)
}

impl LayoutPosition {
    /// The floor a component at this position falls back to when it declares no
    /// `min_size()` of its own. `Middle` is a bar zone that holds no component, so
    /// it takes the bottom row's floor — the area its components actually land in.
    pub fn fallback_min_width(self) -> Pixels {
        match self {
            LayoutPosition::Left | LayoutPosition::Right => SIDE_COLUMN_MIN_WIDTH,
            LayoutPosition::Bottom | LayoutPosition::Middle => BOTTOM_SEGMENT_MIN_WIDTH,
        }
    }

    /// A panel's declared `min_size()` wins wherever it has one — larger or smaller
    /// than the floor. The floor exists for the panels that declare nothing, not as
    /// a clamp on the ones that do.
    pub fn effective_min_width(self, declared: Option<Pixels>) -> Pixels {
        declared.unwrap_or_else(|| self.fallback_min_width())
    }
}

impl LayoutItem {
    pub fn accepts(&self, position: LayoutPosition) -> bool {
        self.valid_positions.contains(&position)
    }

    /// The terms search matches against, label included.
    pub fn search_terms(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.label)
            .chain(self.aliases.iter().copied())
            .chain(self.clarification)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use collections::HashSet;

    #[test]
    fn every_item_answers_its_metadata_without_a_live_panel() {
        // The whole reason this table is static: the layout surface has to render
        // a complete list before any panel has registered, or a hidden component
        // could never be recovered.
        assert_eq!(all().len(), 22);
        for item in all() {
            assert!(!item.id.is_empty(), "{} has no id", item.label);
            assert!(!item.label.is_empty(), "{} has no label", item.id);
            assert!(
                !item.valid_positions.is_empty(),
                "{} accepts no position, so the surface could offer no choice",
                item.id
            );
            assert!(
                item.accepts(item.registered_position),
                "{} defaults to a position it rejects",
                item.id
            );
        }
    }

    #[test]
    fn ids_and_source_ranks_are_unique() {
        let ids: HashSet<_> = all().iter().map(|item| item.id).collect();
        assert_eq!(ids.len(), all().len(), "duplicate id in the registry");

        let ranks: HashSet<_> = all().iter().map(|item| item.source_rank).collect();
        assert_eq!(ranks.len(), all().len(), "duplicate source rank");
    }

    #[test]
    fn only_workspace_components_are_dock_hosted_and_the_slot_occupant_is_not() {
        // The sequence at `materialized_order` orders dock icons. An id in it that
        // `PanelButtons` never draws is the shape this feature has been bitten by
        // more than once — a sequence and a strip disagreeing about what exists.
        for item in all() {
            if item.kind == LayoutItemKind::StatusControl {
                assert!(!item.dock_hosted, "{} hosts nothing", item.id);
            }
        }
        assert!(
            !get(ids::THREADS_SIDEBAR)
                .expect("threads sidebar has a row")
                .dock_hosted,
            "the sidebar-slot occupant claimed a dock column"
        );
        assert!(
            get(ids::GIT_PANEL)
                .expect("git panel has a row")
                .dock_hosted,
            "a dock panel stopped claiming its dock"
        );
    }

    #[test]
    fn canonical_order_puts_every_component_before_every_status_control() {
        let mut sorted: Vec<_> = all().iter().collect();
        sorted.sort_by_key(|item| item.source_rank);

        let ordered_ids: Vec<_> = sorted.iter().map(|item| item.id).collect();
        let table_ids: Vec<_> = all().iter().map(|item| item.id).collect();
        assert_eq!(
            ordered_ids, table_ids,
            "the table is written out of source-rank order"
        );

        let first_status = sorted
            .iter()
            .position(|item| item.kind == LayoutItemKind::StatusControl)
            .expect("no status controls registered");
        assert!(
            sorted[first_status..]
                .iter()
                .all(|item| item.kind == LayoutItemKind::StatusControl),
            "a workspace component sorts after a status control"
        );
    }

    #[test]
    fn active_file_is_the_only_factory_hidden_item() {
        // Reset must reproduce factory state, not a maximally-visible layout that
        // never existed, so this is load-bearing rather than incidental.
        let hidden: Vec<_> = all()
            .iter()
            .filter(|item| item.default_hidden)
            .map(|item| item.id)
            .collect();
        assert_eq!(hidden, vec!["active_file"]);
    }

    #[test]
    fn line_endings_is_not_a_placeable_item() {
        // Deliberately absent. `LineEndingIndicator` gates its own render on
        // `status_bar.line_endings_button`, and that check is upstream code we do
        // not carry a patch against. A registry row here would be a second answer
        // to "is it showing" that the row could never make true: placing it would
        // move the item into a zone while the gate kept it drawing nothing, which
        // is exactly what dogfood B4-6 observed before the row was removed.
        assert!(get("line_endings").is_none());
    }

    #[test]
    fn only_terminal_and_debug_accept_the_bottom() {
        let bottom: Vec<_> = of_kind(LayoutItemKind::WorkspaceComponent)
            .filter(|item| item.accepts(LayoutPosition::Bottom))
            .map(|item| item.id)
            .collect();
        assert_eq!(bottom, vec!["TerminalPanel", "DebugPanel"]);
    }

    #[test]
    fn no_workspace_component_accepts_a_bar_only_zone() {
        // Middle is an icon zone, not a dock. A component offered Middle would be
        // a placement its own `position_is_valid` would then refuse.
        for item in of_kind(LayoutItemKind::WorkspaceComponent) {
            assert!(
                !item.accepts(LayoutPosition::Middle),
                "{} offers Middle, which no dock can satisfy",
                item.id
            );
        }
    }

    #[test]
    fn component_ids_are_transcribed_not_derived() {
        // `agent_panel` is snake_case where every other component key is
        // PascalCase. An unknown key fails silently by design, so a "corrected"
        // id here would read as a component that simply has not loaded.
        assert!(get("agent_panel").is_some());
        assert!(get("AgentPanel").is_none());
        assert!(get("ProjectPanel").is_some());
        assert!(get("project_panel").is_none());
    }

    #[test]
    fn every_id_constant_still_names_a_registered_item() {
        // The table is written in terms of these, so a const and a row cannot hold
        // different strings. What they can do is outlive each other: delete a row
        // and its const keeps compiling at every call site, resolving to nothing.
        let named = [
            ids::AGENT_PANEL,
            ids::PROJECT_PANEL,
            ids::TERMINAL_PANEL,
            ids::GIT_PANEL,
            ids::COLLABORATION_PANEL,
            ids::OUTLINE_PANEL,
            ids::DEBUG_PANEL,
            ids::THREADS_SIDEBAR,
            ids::PROJECT_SEARCH,
            ids::LSP_SERVERS,
            ids::DIAGNOSTICS,
            ids::ACTIVE_FILE,
            ids::GIT_BLAME,
            ids::MERGE_CONFLICTS,
            ids::ACTIVITY,
            ids::EDIT_PREDICTION,
            ids::ACTIVE_ENCODING,
            ids::ACTIVE_LANGUAGE,
            ids::ACTIVE_TOOLCHAIN,
            ids::VIM_MODE,
            ids::CURSOR_POSITION,
            ids::IMAGE_INFO,
        ];
        for id in named {
            assert!(get(id).is_some(), "{id} has a constant but no row");
        }
        assert_eq!(
            named.len(),
            all().len(),
            "a row was added without a constant, so call sites would have to spell it"
        );
    }

    #[test]
    fn search_matches_a_term_the_label_does_not_contain() {
        let project_panel = get("ProjectPanel").expect("ProjectPanel is registered");
        let terms: Vec<_> = project_panel.search_terms().collect();
        assert!(terms.contains(&"Project Panel"), "label is not searchable");
        assert!(
            terms.contains(&"explorer"),
            "an alias that shares no word with the label is not searchable"
        );
    }

    #[test]
    fn a_declared_minimum_beats_the_registry_floor_in_both_directions() {
        // Agent declares 300px, above the 200px side floor, so `max` and `override`
        // agree there and that case alone would pass on the wrong implementation.
        // The 120px case is the discriminating one.
        assert_eq!(
            LayoutPosition::Left.effective_min_width(Some(px(300.))),
            px(300.)
        );
        assert_eq!(
            LayoutPosition::Left.effective_min_width(Some(px(120.))),
            px(120.),
            "the floor clamped a smaller declared minimum instead of yielding to it"
        );
    }

    #[test]
    fn an_undeclared_minimum_takes_the_floor_for_its_area() {
        assert_eq!(
            LayoutPosition::Left.effective_min_width(None),
            SIDE_COLUMN_MIN_WIDTH
        );
        assert_eq!(
            LayoutPosition::Bottom.effective_min_width(None),
            BOTTOM_SEGMENT_MIN_WIDTH
        );
        assert_ne!(
            SIDE_COLUMN_MIN_WIDTH, BOTTOM_SEGMENT_MIN_WIDTH,
            "one floor for both areas makes the previous assertions vacuous"
        );
    }
}
