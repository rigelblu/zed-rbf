use gpui::{
    App, Context, Empty, EventEmitter, IntoElement, ParentElement, Render, SharedString, Window,
};
use ui::{Button, Tooltip, prelude::*};
use util::paths::PathStyle;

use crate::{HideStatusItem, StatusItemView, item::ItemHandle};

pub struct ActiveFileName {
    project_path: Option<SharedString>,
    full_path: Option<SharedString>,
}

impl ActiveFileName {
    pub fn new() -> Self {
        Self {
            project_path: None,
            full_path: None,
        }
    }
}

impl Render for ActiveFileName {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Deliberately does not consult `show_active_file`. `StatusBar` already
        // decides whether this item draws, through
        // `resolved_status_control_location`, which makes an explicit placement
        // outrank the legacy boolean and resolves to `Hidden` when there is no
        // placement and the boolean is off — so the default is unchanged and a
        // placed item is no longer suppressed. Reading the boolean here too made
        // "is it showing" a fact with two answers, and this one won by drawing
        // nothing: placing the item moved it into a zone where it rendered an
        // empty element. Observed in dogfood B4-6.
        let Some(project_path) = self.project_path.clone() else {
            return Empty.into_any_element();
        };

        let tooltip_text = self
            .full_path
            .clone()
            .unwrap_or_else(|| project_path.clone());

        div()
            .child(
                Button::new("active-file-name-button", project_path)
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text(tooltip_text)),
            )
            .into_any_element()
    }
}

impl EventEmitter<crate::ToolbarItemEvent> for ActiveFileName {}

impl StatusItemView for ActiveFileName {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(item) = active_pane_item {
            self.project_path = item
                .project_path(cx)
                .map(|path| path.path.display(PathStyle::local()).into_owned().into());
            self.full_path = item.tab_tooltip_text(cx);
        } else {
            self.project_path = None;
            self.full_path = None;
        }
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        Some(HideStatusItem::new(|settings| {
            settings.status_bar.get_or_insert_default().show_active_file = Some(false);
        }))
    }
}
