#![allow(missing_docs)]

use crate::schema::{status_colors_refinement, syntax_overrides, theme_colors_refinement};
use crate::{merge_accent_colors, merge_player_colors};
use collections::HashMap;
use gpui::{
    App, Context, Font, FontFallbacks, FontStyle, Global, Pixels, SharedString, Subscription,
    Window, px,
};
use gpui_util::ResultExt as _;
use refineable::Refineable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub use settings::{FontFamilyName, IconThemeName, ThemeAppearanceMode, ThemeName};
use settings::{IntoGpui, RegisterSetting, Settings, SettingsContent};
use std::sync::Arc;
use theme::{Appearance, DEFAULT_ICON_THEME_NAME, SyntaxTheme, Theme, UiDensity};

const MIN_FONT_SIZE: Pixels = px(6.0);
const MAX_FONT_SIZE: Pixels = px(100.0);
const MIN_LINE_HEIGHT: f32 = 1.0;

pub(crate) fn ui_density_from_settings(val: settings::UiDensity) -> UiDensity {
    match val {
        settings::UiDensity::Compact => UiDensity::Compact,
        settings::UiDensity::Default => UiDensity::Default,
        settings::UiDensity::Comfortable => UiDensity::Comfortable,
    }
}

pub fn appearance_to_mode(appearance: Appearance) -> ThemeAppearanceMode {
    match appearance {
        Appearance::Light => ThemeAppearanceMode::Light,
        Appearance::Dark => ThemeAppearanceMode::Dark,
    }
}

/// Customizable settings for the UI and theme system.
#[derive(Clone, PartialEq, RegisterSetting)]
pub struct ThemeSettings {
    /// The UI font size. Determines the size of text in the UI,
    /// as well as the size of a [gpui::Rems] unit.
    ///
    /// Changing this will impact the size of all UI elements.
    ui_font_size: Pixels,
    /// The font used for UI elements.
    pub ui_font: Font,
    /// The font size used for buffers, and the terminal.
    ///
    /// The terminal font size can be overridden using it's own setting.
    buffer_font_size: Pixels,
    /// The font used for buffers, and the terminal.
    ///
    /// The terminal font family can be overridden using it's own setting.
    pub buffer_font: Font,
    /// The agent UI font family. Determines the family of response text in the agent panel.
    /// Falls back to the UI font family if unset.
    agent_ui_font_family: Option<SharedString>,
    /// The agent font size. Determines the size of text in the agent panel. Falls back to the UI font size if unset.
    agent_ui_font_size: Option<Pixels>,
    /// The agent buffer font family. Determines the family of user messages in the agent panel.
    /// Falls back to the buffer font family if unset.
    agent_buffer_font_family: Option<SharedString>,
    /// The agent buffer font size. Determines the size of user messages in the agent panel.
    agent_buffer_font_size: Option<Pixels>,
    git_commit_buffer_font_size: Option<Pixels>,
    /// The font family to use for rendering in the markdown preview.
    /// Falls back to the UI font family if unset.
    markdown_preview_font_family: Option<SharedString>,
    /// The font family to use for code in the markdown preview.
    /// Falls back to the buffer font family if unset.
    markdown_preview_code_font_family: Option<SharedString>,
    /// The font size to use for rendering in the markdown preview.
    /// Falls back to the UI font size if unset.
    markdown_preview_font_size: Option<Pixels>,
    /// The theme to use for the markdown preview.
    /// Falls back to the main editor theme if unset.
    pub markdown_preview_theme: Option<ThemeSelection>,
    /// The line height for buffers, and the terminal.
    ///
    /// Changing this may affect the spacing of some UI elements.
    ///
    /// The terminal font family can be overridden using it's own setting.
    pub buffer_line_height: BufferLineHeight,
    /// The current theme selection.
    pub theme: ThemeSelection,
    /// Manual overrides for the active theme.
    ///
    /// Note: This setting is still experimental. See [this tracking issue](https://github.com/zed-industries/zed/issues/18078)
    pub experimental_theme_overrides: Option<settings::ThemeStyleContent>,
    /// Manual overrides per theme
    pub theme_overrides: HashMap<String, settings::ThemeStyleContent>,
    /// The current icon theme selection.
    pub icon_theme: IconThemeSelection,
    /// The density of the UI.
    /// Note: This setting is still experimental. See [this tracking issue](
    pub ui_density: UiDensity,
    /// The amount of fading applied to unnecessary code.
    pub unnecessary_code_fade: f32,
    /// Named per-display font sizes, and which display gets which.
    ///
    /// Absent unless the user authors a `display_profiles` block, in which case every
    /// font size below is a fallback for displays that block does not assign.
    pub display_profiles: Option<settings::DisplayProfilesContent>,
}

/// Returns the name of the default theme for the given [`Appearance`].
pub fn default_theme(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Light => settings::DEFAULT_LIGHT_THEME,
        Appearance::Dark => settings::DEFAULT_DARK_THEME,
    }
}

#[derive(Default)]
struct BufferFontSize(Pixels);

impl Global for BufferFontSize {}

#[derive(Default)]
pub(crate) struct UiFontSize(Pixels);

impl Global for UiFontSize {}

/// In-memory override for the UI font size in the agent panel.
#[derive(Default)]
pub struct AgentUiFontSize(Pixels);

impl Global for AgentUiFontSize {}

/// In-memory override for the buffer font size in the agent panel.
#[derive(Default)]
pub struct AgentBufferFontSize(Pixels);

impl Global for AgentBufferFontSize {}

#[derive(Default)]
pub struct GitCommitBufferFontSize(Pixels);

impl Global for GitCommitBufferFontSize {}

/// In-memory override for the markdown preview font size.
#[derive(Default)]
pub struct MarkdownPreviewFontSize(Pixels);

impl Global for MarkdownPreviewFontSize {}

/// The display profile each connected display resolves to, by display id.
///
/// Kept as a precomputed map rather than resolved on demand because the alternative puts
/// [`App::displays`] — a `CGGetActiveDisplayList` syscall and one allocation per connected
/// display — on the render path, which reads font sizes many times per frame. This is
/// rebuilt only when the display configuration or the `display_profiles` settings change.
///
/// Public so surfaces that apply font size imperatively rather than at render — the agent
/// panel's diff editors are the one such case — can observe it and re-apply when a display
/// change re-resolves the assignments.
#[derive(Default)]
pub struct DisplayProfileAssignments(HashMap<gpui::DisplayId, SharedString>);

impl Global for DisplayProfileAssignments {}

/// Represents the selection of a theme, which can be either static or dynamic.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum ThemeSelection {
    /// A static theme selection, represented by a single theme name.
    Static(ThemeName),
    /// A dynamic theme selection, which can change based the [ThemeMode].
    Dynamic {
        /// The mode used to determine which theme to use.
        #[serde(default)]
        mode: ThemeAppearanceMode,
        /// The theme to use for light mode.
        light: ThemeName,
        /// The theme to use for dark mode.
        dark: ThemeName,
    },
}

impl From<settings::ThemeSelection> for ThemeSelection {
    fn from(selection: settings::ThemeSelection) -> Self {
        match selection {
            settings::ThemeSelection::Static(theme) => ThemeSelection::Static(theme),
            settings::ThemeSelection::Dynamic { mode, light, dark } => {
                ThemeSelection::Dynamic { mode, light, dark }
            }
        }
    }
}

impl ThemeSelection {
    /// Returns the theme name for the selected [ThemeMode].
    pub fn name(&self, system_appearance: Appearance) -> ThemeName {
        match self {
            Self::Static(theme) => theme.clone(),
            Self::Dynamic { mode, light, dark } => match mode {
                ThemeAppearanceMode::Light => light.clone(),
                ThemeAppearanceMode::Dark => dark.clone(),
                ThemeAppearanceMode::System => match system_appearance {
                    Appearance::Light => light.clone(),
                    Appearance::Dark => dark.clone(),
                },
            },
        }
    }

    /// Returns the [ThemeMode] for the [ThemeSelection].
    pub fn mode(&self) -> Option<ThemeAppearanceMode> {
        match self {
            ThemeSelection::Static(_) => None,
            ThemeSelection::Dynamic { mode, .. } => Some(*mode),
        }
    }
}

/// Represents the selection of an icon theme, which can be either static or dynamic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconThemeSelection {
    /// A static icon theme selection, represented by a single icon theme name.
    Static(IconThemeName),
    /// A dynamic icon theme selection, which can change based on the [`ThemeMode`].
    Dynamic {
        /// The mode used to determine which theme to use.
        mode: ThemeAppearanceMode,
        /// The icon theme to use for light mode.
        light: IconThemeName,
        /// The icon theme to use for dark mode.
        dark: IconThemeName,
    },
}

impl From<settings::IconThemeSelection> for IconThemeSelection {
    fn from(selection: settings::IconThemeSelection) -> Self {
        match selection {
            settings::IconThemeSelection::Static(theme) => IconThemeSelection::Static(theme),
            settings::IconThemeSelection::Dynamic { mode, light, dark } => {
                IconThemeSelection::Dynamic { mode, light, dark }
            }
        }
    }
}

impl IconThemeSelection {
    /// Returns the icon theme name based on the given [`Appearance`].
    pub fn name(&self, system_appearance: Appearance) -> IconThemeName {
        match self {
            Self::Static(theme) => theme.clone(),
            Self::Dynamic { mode, light, dark } => match mode {
                ThemeAppearanceMode::Light => light.clone(),
                ThemeAppearanceMode::Dark => dark.clone(),
                ThemeAppearanceMode::System => match system_appearance {
                    Appearance::Light => light.clone(),
                    Appearance::Dark => dark.clone(),
                },
            },
        }
    }

    /// Returns the [`ThemeMode`] for the [`IconThemeSelection`].
    pub fn mode(&self) -> Option<ThemeAppearanceMode> {
        match self {
            IconThemeSelection::Static(_) => None,
            IconThemeSelection::Dynamic { mode, .. } => Some(*mode),
        }
    }
}

/// Sets the theme for the given appearance to the theme with the specified name.
///
/// The caller should make sure that the [`Appearance`] matches the theme associated with the name.
///
/// If the current [`ThemeAppearanceMode`] is set to [`System`] and the user's system [`Appearance`]
/// is different than the new theme's [`Appearance`], this function will update the
/// [`ThemeAppearanceMode`] to the new theme's appearance in order to display the new theme.
///
/// [`System`]: ThemeAppearanceMode::System
pub fn set_theme(
    current: &mut SettingsContent,
    theme_name: impl Into<Arc<str>>,
    theme_appearance: Appearance,
    system_appearance: Appearance,
) {
    let theme_name = ThemeName(theme_name.into());

    let Some(selection) = current.theme.theme.as_mut() else {
        current.theme.theme = Some(settings::ThemeSelection::Static(theme_name));
        return;
    };

    match selection {
        settings::ThemeSelection::Static(theme) => {
            *theme = theme_name;
        }
        settings::ThemeSelection::Dynamic { mode, light, dark } => {
            match theme_appearance {
                Appearance::Light => *light = theme_name,
                Appearance::Dark => *dark = theme_name,
            }

            let should_update_mode =
                !(mode == &ThemeAppearanceMode::System && theme_appearance == system_appearance);

            if should_update_mode {
                *mode = appearance_to_mode(theme_appearance);
            }
        }
    }
}

/// Sets the icon theme for the given appearance to the icon theme with the specified name.
pub fn set_icon_theme(
    current: &mut SettingsContent,
    icon_theme_name: IconThemeName,
    appearance: Appearance,
) {
    if let Some(selection) = current.theme.icon_theme.as_mut() {
        let icon_theme_to_update = match selection {
            settings::IconThemeSelection::Static(theme) => theme,
            settings::IconThemeSelection::Dynamic { mode, light, dark } => match mode {
                ThemeAppearanceMode::Light => light,
                ThemeAppearanceMode::Dark => dark,
                ThemeAppearanceMode::System => match appearance {
                    Appearance::Light => light,
                    Appearance::Dark => dark,
                },
            },
        };

        *icon_theme_to_update = icon_theme_name;
    } else {
        current.theme.icon_theme = Some(settings::IconThemeSelection::Static(icon_theme_name));
    }
}

/// Sets the mode for the theme.
pub fn set_mode(content: &mut SettingsContent, mode: ThemeAppearanceMode) {
    let theme = content.theme.as_mut();

    if let Some(selection) = theme.theme.as_mut() {
        match selection {
            settings::ThemeSelection::Static(_) => {
                *selection = settings::ThemeSelection::Dynamic {
                    mode: ThemeAppearanceMode::System,
                    light: ThemeName(settings::DEFAULT_LIGHT_THEME.into()),
                    dark: ThemeName(settings::DEFAULT_DARK_THEME.into()),
                };
            }
            settings::ThemeSelection::Dynamic {
                mode: mode_to_update,
                ..
            } => *mode_to_update = mode,
        }
    } else {
        theme.theme = Some(settings::ThemeSelection::Dynamic {
            mode,
            light: ThemeName(settings::DEFAULT_LIGHT_THEME.into()),
            dark: ThemeName(settings::DEFAULT_DARK_THEME.into()),
        });
    }

    if let Some(selection) = theme.icon_theme.as_mut() {
        match selection {
            settings::IconThemeSelection::Static(icon_theme) => {
                *selection = settings::IconThemeSelection::Dynamic {
                    mode,
                    light: icon_theme.clone(),
                    dark: icon_theme.clone(),
                };
            }
            settings::IconThemeSelection::Dynamic {
                mode: mode_to_update,
                ..
            } => *mode_to_update = mode,
        }
    } else {
        theme.icon_theme = Some(settings::IconThemeSelection::Static(IconThemeName(
            DEFAULT_ICON_THEME_NAME.into(),
        )));
    }
}

/// The buffer's line height.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum BufferLineHeight {
    /// A less dense line height.
    #[default]
    Comfortable,
    /// The default line height.
    Standard,
    /// A custom line height, where 1.0 is the font's height. Must be at least 1.0.
    Custom(f32),
}

impl From<settings::BufferLineHeight> for BufferLineHeight {
    fn from(value: settings::BufferLineHeight) -> Self {
        match value {
            settings::BufferLineHeight::Comfortable => BufferLineHeight::Comfortable,
            settings::BufferLineHeight::Standard => BufferLineHeight::Standard,
            settings::BufferLineHeight::Custom(line_height) => {
                BufferLineHeight::Custom(line_height)
            }
        }
    }
}

impl BufferLineHeight {
    /// Returns the value of the line height.
    pub fn value(&self) -> f32 {
        match self {
            BufferLineHeight::Comfortable => 1.618,
            BufferLineHeight::Standard => 1.3,
            BufferLineHeight::Custom(line_height) => *line_height,
        }
    }
}

impl ThemeSettings {
    /// Returns the buffer font size.
    pub fn buffer_font_size(&self, cx: &App) -> Pixels {
        let font_size = cx
            .try_global::<BufferFontSize>()
            .map(|size| size.0)
            .unwrap_or(self.buffer_font_size);
        clamp_font_size(font_size)
    }

    /// Returns the UI font size.
    pub fn ui_font_size(&self, cx: &App) -> Pixels {
        let font_size = cx
            .try_global::<UiFontSize>()
            .map(|size| size.0)
            .unwrap_or(self.ui_font_size);
        clamp_font_size(font_size)
    }

    /// Returns the agent panel font size. Falls back to the UI font size if unset.
    pub fn agent_ui_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<AgentUiFontSize>()
            .map(|size| size.0)
            .or(self.agent_ui_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| self.ui_font_size(cx))
    }

    pub fn agent_ui_font_family(&self) -> &SharedString {
        self.agent_ui_font_family
            .as_ref()
            .unwrap_or(&self.ui_font.family)
    }

    /// Returns the agent panel buffer font size.
    pub fn agent_buffer_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<AgentBufferFontSize>()
            .map(|size| size.0)
            .or(self.agent_buffer_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| self.buffer_font_size(cx))
    }

    pub fn agent_buffer_font_family(&self) -> &SharedString {
        self.agent_buffer_font_family
            .as_ref()
            .unwrap_or(&self.buffer_font.family)
    }

    /// Returns the display profile that applies to `window`, if the display it is on is
    /// assigned one that exists.
    ///
    /// Resolution is two map lookups against the precomputed
    /// [`DisplayProfileAssignments`], so this is cheap enough to call while rendering.
    pub fn display_profile_for(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<&settings::DisplayProfileContent> {
        let display_id = window.display_id()?;
        let profile_name = cx
            .try_global::<DisplayProfileAssignments>()?
            .0
            .get(&display_id)?;
        self.display_profiles
            .as_ref()?
            .profiles
            .as_ref()?
            .get(profile_name.as_ref())
    }

    /// The size a display profile sets for `window`, before any manual adjustment.
    fn ui_font_size_base(&self, window: &Window, cx: &App) -> Pixels {
        self.display_profile_for(window, cx)
            .and_then(|profile| profile.ui_font_size)
            .map(|size| size.into_gpui())
            .unwrap_or(self.ui_font_size)
    }

    /// The size a display profile sets for `window`, before any manual adjustment.
    fn buffer_font_size_base(&self, window: &Window, cx: &App) -> Pixels {
        self.display_profile_for(window, cx)
            .and_then(|profile| profile.buffer_font_size)
            .map(|size| size.into_gpui())
            .unwrap_or(self.buffer_font_size)
    }

    /// Returns the UI font size for `window`, resolved through its display's profile.
    ///
    /// A manual `cmd +` / `cmd -` adjustment applies as a **delta on top of** whatever this
    /// window resolved, not as an absolute that replaces it. The globals holding those
    /// adjustments are app-wide, so treating one as absolute would collapse every window to
    /// a single size the moment the user pressed `cmd +` — which is exactly the promise this
    /// feature exists to keep. As a delta, two windows on two displays each step from their
    /// own profile and stay correctly differentiated.
    pub fn ui_font_size_for(&self, window: &Window, cx: &App) -> Pixels {
        let adjustment = cx
            .try_global::<UiFontSize>()
            .map_or(px(0.), |adjusted| adjusted.0 - self.ui_font_size);
        clamp_font_size(self.ui_font_size_base(window, cx) + adjustment)
    }

    /// Returns the buffer font size for `window`, resolved through its display's profile.
    ///
    /// A manual adjustment applies as a delta; see [`Self::ui_font_size_for`].
    pub fn buffer_font_size_for(&self, window: &Window, cx: &App) -> Pixels {
        let adjustment = cx
            .try_global::<BufferFontSize>()
            .map_or(px(0.), |adjusted| adjusted.0 - self.buffer_font_size);
        clamp_font_size(self.buffer_font_size_base(window, cx) + adjustment)
    }

    /// Returns the agent panel response font size for `window`, resolved through its
    /// display's profile. Falls back to the window's UI font size if unset everywhere.
    pub fn agent_ui_font_size_for(&self, window: &Window, cx: &App) -> Pixels {
        let base = self
            .display_profile_for(window, cx)
            .and_then(|profile| profile.agent_ui_font_size)
            .map(|size| size.into_gpui())
            .or(self.agent_ui_font_size)
            .unwrap_or_else(|| self.ui_font_size_base(window, cx));
        let settings_baseline = self.agent_ui_font_size.unwrap_or(self.ui_font_size);
        let adjustment = cx
            .try_global::<AgentUiFontSize>()
            .map_or(px(0.), |adjusted| adjusted.0 - settings_baseline);
        clamp_font_size(base + adjustment)
    }

    /// Returns the agent panel user-message font size for `window`, resolved through its
    /// display's profile. Falls back to the window's buffer font size if unset everywhere.
    pub fn agent_buffer_font_size_for(&self, window: &Window, cx: &App) -> Pixels {
        let base = self
            .display_profile_for(window, cx)
            .and_then(|profile| profile.agent_buffer_font_size)
            .map(|size| size.into_gpui())
            .or(self.agent_buffer_font_size)
            .unwrap_or_else(|| self.buffer_font_size_base(window, cx));
        let settings_baseline = self.agent_buffer_font_size.unwrap_or(self.buffer_font_size);
        let adjustment = cx
            .try_global::<AgentBufferFontSize>()
            .map_or(px(0.), |adjusted| adjusted.0 - settings_baseline);
        clamp_font_size(base + adjustment)
    }

    /// Returns the terminal font size for `window` when its display's profile sets one.
    ///
    /// Terminal font size lives in `TerminalSettings` rather than here, so this reports
    /// only the profile's override and leaves the fallback chain to the caller.
    pub fn terminal_font_size_for(&self, window: &Window, cx: &App) -> Option<Pixels> {
        self.display_profile_for(window, cx)
            .and_then(|profile| profile.terminal.as_ref())
            .and_then(|terminal| terminal.font_size)
            .map(|size| clamp_font_size(size.into_gpui()))
    }

    pub fn git_commit_buffer_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<GitCommitBufferFontSize>()
            .map(|size| size.0)
            .or(self.git_commit_buffer_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| self.buffer_font_size(cx))
    }

    /// Returns the font family to use in the markdown preview,
    /// falling back to the UI font family when unset.
    pub fn markdown_preview_font_family(&self) -> &SharedString {
        self.markdown_preview_font_family
            .as_ref()
            .unwrap_or(&self.ui_font.family)
    }

    /// Returns the font family to use for code in the markdown preview,
    /// falling back to the buffer font family when unset.
    pub fn markdown_preview_code_font_family(&self) -> &SharedString {
        self.markdown_preview_code_font_family
            .as_ref()
            .unwrap_or(&self.buffer_font.family)
    }

    /// Returns the markdown preview font size.
    ///
    /// Note: the fallback deliberately uses `self.ui_font_size` instead of `ui_font_size(cx)`,
    /// so that temporary UI zoom does not also resize the markdown preview.
    pub fn markdown_preview_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<MarkdownPreviewFontSize>()
            .map(|size| size.0)
            .or(self.markdown_preview_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| clamp_font_size(self.ui_font_size))
    }

    /// Returns the buffer font size, read from the settings.
    ///
    /// The real buffer font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::buffer_font_size`] to get the real font size.
    pub fn buffer_font_size_settings(&self) -> Pixels {
        self.buffer_font_size
    }

    /// Returns the UI font size, read from the settings.
    ///
    /// The real UI font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::ui_font_size`] to get the real font size.
    pub fn ui_font_size_settings(&self) -> Pixels {
        self.ui_font_size
    }

    /// Returns the agent font size, read from the settings.
    ///
    /// The real agent font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::agent_ui_font_size`] to get the real font size.
    pub fn agent_ui_font_size_settings(&self) -> Option<Pixels> {
        self.agent_ui_font_size
    }

    /// Returns the agent buffer font size, read from the settings.
    ///
    /// The real agent buffer font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::agent_buffer_font_size`] to get the real font size.
    pub fn agent_buffer_font_size_settings(&self) -> Option<Pixels> {
        self.agent_buffer_font_size
    }

    pub fn git_commit_buffer_font_size_settings(&self) -> Option<Pixels> {
        self.git_commit_buffer_font_size
    }

    /// Returns the markdown preview font size, read from the settings.
    ///
    /// The real markdown preview font size is stored in-memory, to support temporary
    /// font size changes. Use [`Self::markdown_preview_font_size`] to get the real font size.
    pub fn markdown_preview_font_size_settings(&self) -> Option<Pixels> {
        self.markdown_preview_font_size
    }

    /// Returns the buffer's line height.
    pub fn line_height(&self) -> f32 {
        f32::max(self.buffer_line_height.value(), MIN_LINE_HEIGHT)
    }

    /// Applies the theme overrides, if there are any, to the current theme.
    pub fn apply_theme_overrides(&self, mut arc_theme: Arc<Theme>) -> Arc<Theme> {
        if let Some(experimental_theme_overrides) = &self.experimental_theme_overrides {
            let mut theme = (*arc_theme).clone();
            ThemeSettings::modify_theme(&mut theme, experimental_theme_overrides);
            arc_theme = Arc::new(theme);
        }

        if let Some(theme_overrides) = self.theme_overrides.get(arc_theme.name.as_ref()) {
            let mut theme = (*arc_theme).clone();
            ThemeSettings::modify_theme(&mut theme, theme_overrides);
            arc_theme = Arc::new(theme);
        }

        arc_theme
    }

    fn modify_theme(base_theme: &mut Theme, theme_overrides: &settings::ThemeStyleContent) {
        if let Some(window_background_appearance) = theme_overrides.window_background_appearance {
            base_theme.styles.window_background_appearance =
                window_background_appearance.into_gpui();
        }
        let status_color_refinement = status_colors_refinement(&theme_overrides.status);

        let theme_color_refinement = theme_colors_refinement(
            &theme_overrides.colors,
            &status_color_refinement,
            base_theme.appearance.is_light(),
        );
        base_theme.styles.colors.refine(&theme_color_refinement);
        base_theme.styles.status.refine(&status_color_refinement);
        merge_player_colors(&mut base_theme.styles.player, &theme_overrides.players);
        merge_accent_colors(&mut base_theme.styles.accents, &theme_overrides.accents);
        base_theme.styles.syntax = SyntaxTheme::merge(
            base_theme.styles.syntax.clone(),
            syntax_overrides(theme_overrides),
        );
    }
}

/// Observe changes to the adjusted buffer font size.
pub fn observe_buffer_font_size_adjustment<V: 'static>(
    cx: &mut Context<V>,
    f: impl 'static + Fn(&mut V, &mut Context<V>),
) -> Subscription {
    cx.observe_global::<BufferFontSize>(f)
}

/// Gets the font size, adjusted by the difference between the current buffer font size and the one set in the settings.
pub fn adjusted_font_size(size: Pixels, cx: &App) -> Pixels {
    let adjusted_font_size =
        if let Some(BufferFontSize(adjusted_size)) = cx.try_global::<BufferFontSize>() {
            let buffer_font_size = ThemeSettings::get_global(cx).buffer_font_size;
            let delta = *adjusted_size - buffer_font_size;
            size + delta
        } else {
            size
        };
    clamp_font_size(adjusted_font_size)
}

/// Adjusts the buffer font size, without persisting the result in the settings.
/// This will be effective until the app is restarted.
pub fn adjust_buffer_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let buffer_font_size = ThemeSettings::get_global(cx).buffer_font_size;
    let adjusted_size = cx
        .try_global::<BufferFontSize>()
        .map_or(buffer_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(BufferFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the buffer font size to the default value.
pub fn reset_buffer_font_size(cx: &mut App) {
    if cx.has_global::<BufferFontSize>() {
        cx.remove_global::<BufferFontSize>();
        cx.refresh_windows();
    }
}

#[allow(missing_docs)]
pub fn setup_ui_font(window: &mut Window, cx: &mut App) -> gpui::Font {
    let (ui_font, ui_font_size) = {
        let theme_settings = ThemeSettings::get_global(cx);
        let font = theme_settings.ui_font.clone();
        (font, theme_settings.ui_font_size_for(window, cx))
    };

    window.set_rem_size(ui_font_size);
    ui_font
}

/// Recomputes which display profile each connected display resolves to.
///
/// A display matches on its name first and its UUID second: names are what people write in
/// settings, and a UUID is what tells apart two identical monitors reporting one name. A
/// display that matches nothing is simply absent from the map, and windows on it keep the
/// existing global font settings.
///
/// Call this whenever the connected displays change or `display_profiles` is edited — it
/// reads every connected display, so it must not run on a render or window-drag path.
pub fn refresh_display_profile_assignments(cx: &mut App) {
    let Some(display_profiles) = ThemeSettings::get_global(cx).display_profiles.clone() else {
        cx.set_global(DisplayProfileAssignments::default());
        return;
    };
    let assign = display_profiles.assign.unwrap_or_default();
    let profiles = display_profiles.profiles.unwrap_or_default();

    let mut assignments = HashMap::default();
    for display in cx.displays() {
        // UUID is checked first so it can do the job it exists for. Two identical monitors
        // report one name, and singling one out means writing its UUID — which only works
        // if a UUID entry outranks the name entry that also matches it.
        let matched = display
            .uuid()
            .log_err()
            .and_then(|uuid| assign.get_key_value(&uuid.to_string()))
            .or_else(|| display.name().and_then(|name| assign.get_key_value(&name)));
        let Some((display_key, profile_name)) = matched else {
            continue;
        };

        // An unmatched display is a normal state — a monitor can simply be unplugged — but
        // an assignment naming a profile that does not exist is always a typo.
        if !profiles.contains_key(profile_name) {
            log::warn!(
                "display_profiles: display {display_key:?} is assigned profile \
                 {profile_name:?}, which no profile defines; that display keeps the global \
                 font settings"
            );
            continue;
        }

        assignments.insert(display.id(), SharedString::from(profile_name.clone()));
    }

    cx.set_global(DisplayProfileAssignments(assignments));
}

/// Sets the adjusted UI font size.
///
/// Steps in settings space, not in any one window's resolved space: the resolvers read this
/// global as a *delta* from the settings value and add it to whatever each window's display
/// profile gave it. Stepping from a window's resolved size here would fold that window's
/// profile into the delta and shift every other window by it.
pub fn adjust_ui_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let ui_font_size = ThemeSettings::get_global(cx).ui_font_size;
    let adjusted_size = cx
        .try_global::<UiFontSize>()
        .map_or(ui_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(UiFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the UI font size to the default value.
pub fn reset_ui_font_size(cx: &mut App) {
    if cx.has_global::<UiFontSize>() {
        cx.remove_global::<UiFontSize>();
        cx.refresh_windows();
    }
}

/// Sets the adjusted font size of agent responses in the agent panel.
pub fn adjust_agent_ui_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let settings = ThemeSettings::get_global(cx);
    let agent_ui_font_size = settings.agent_ui_font_size.unwrap_or(settings.ui_font_size);
    let adjusted_size = cx
        .try_global::<AgentUiFontSize>()
        .map_or(agent_ui_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(AgentUiFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the agent response font size in the agent panel to the default value.
pub fn reset_agent_ui_font_size(cx: &mut App) {
    if cx.has_global::<AgentUiFontSize>() {
        cx.remove_global::<AgentUiFontSize>();
        cx.refresh_windows();
    }
}

/// Sets the adjusted font size of user messages in the agent panel.
pub fn adjust_agent_buffer_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let settings = ThemeSettings::get_global(cx);
    let agent_buffer_font_size = settings
        .agent_buffer_font_size
        .unwrap_or(settings.buffer_font_size);
    let adjusted_size = cx
        .try_global::<AgentBufferFontSize>()
        .map_or(agent_buffer_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(AgentBufferFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the user message font size in the agent panel to the default value.
pub fn reset_agent_buffer_font_size(cx: &mut App) {
    if cx.has_global::<AgentBufferFontSize>() {
        cx.remove_global::<AgentBufferFontSize>();
        cx.refresh_windows();
    }
}

pub fn adjust_git_commit_buffer_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let git_commit_buffer_font_size = ThemeSettings::get_global(cx).git_commit_buffer_font_size(cx);
    let adjusted_size = cx
        .try_global::<GitCommitBufferFontSize>()
        .map_or(git_commit_buffer_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(GitCommitBufferFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

pub fn reset_git_commit_buffer_font_size(cx: &mut App) {
    if cx.has_global::<GitCommitBufferFontSize>() {
        cx.remove_global::<GitCommitBufferFontSize>();
        cx.refresh_windows();
    }
}

/// Sets the adjusted font size of the markdown preview.
pub fn adjust_markdown_preview_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let markdown_preview_font_size = ThemeSettings::get_global(cx).markdown_preview_font_size(cx);
    let adjusted_size = cx
        .try_global::<MarkdownPreviewFontSize>()
        .map_or(markdown_preview_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(MarkdownPreviewFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the markdown preview font size to the default value.
pub fn reset_markdown_preview_font_size(cx: &mut App) {
    if cx.has_global::<MarkdownPreviewFontSize>() {
        cx.remove_global::<MarkdownPreviewFontSize>();
        cx.refresh_windows();
    }
}

/// Ensures font size is within the valid range.
pub fn clamp_font_size(size: Pixels) -> Pixels {
    size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

fn font_fallbacks_from_settings(
    fallbacks: Option<Vec<settings::FontFamilyName>>,
) -> Option<FontFallbacks> {
    fallbacks.map(|fallbacks| {
        FontFallbacks::from_fonts(
            fallbacks
                .into_iter()
                .map(|font_family| font_family.0.to_string())
                .collect(),
        )
    })
}

impl settings::Settings for ThemeSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let display_profiles = content.display_profiles.clone();
        let content = &content.theme;
        let theme_selection: ThemeSelection = content.theme.clone().unwrap().into();
        let icon_theme_selection: IconThemeSelection = content.icon_theme.clone().unwrap().into();
        Self {
            ui_font_size: clamp_font_size(content.ui_font_size.unwrap().into_gpui()),
            ui_font: Font {
                family: content.ui_font_family.as_ref().unwrap().0.clone().into(),
                features: content.ui_font_features.clone().unwrap().into_gpui(),
                fallbacks: font_fallbacks_from_settings(content.ui_font_fallbacks.clone()),
                weight: content.ui_font_weight.unwrap().into_gpui(),
                style: Default::default(),
            },
            buffer_font: Font {
                family: content
                    .buffer_font_family
                    .as_ref()
                    .unwrap()
                    .0
                    .clone()
                    .into(),
                features: content.buffer_font_features.clone().unwrap().into_gpui(),
                fallbacks: font_fallbacks_from_settings(content.buffer_font_fallbacks.clone()),
                weight: content.buffer_font_weight.unwrap().into_gpui(),
                style: FontStyle::default(),
            },
            buffer_font_size: clamp_font_size(content.buffer_font_size.unwrap().into_gpui()),
            buffer_line_height: content.buffer_line_height.unwrap().into(),
            agent_ui_font_family: content
                .agent_ui_font_family
                .as_ref()
                .map(|font| font.0.clone().into()),
            agent_ui_font_size: content.agent_ui_font_size.map(|s| s.into_gpui()),
            agent_buffer_font_family: content
                .agent_buffer_font_family
                .as_ref()
                .map(|font| font.0.clone().into()),
            agent_buffer_font_size: content.agent_buffer_font_size.map(|s| s.into_gpui()),
            git_commit_buffer_font_size: content.git_commit_buffer_font_size.map(|s| s.into_gpui()),
            markdown_preview_font_family: content
                .markdown_preview_font_family
                .as_ref()
                .map(|f| f.0.clone().into()),
            markdown_preview_code_font_family: content
                .markdown_preview_code_font_family
                .as_ref()
                .map(|f| f.0.clone().into()),
            markdown_preview_font_size: content.markdown_preview_font_size.map(|s| s.into_gpui()),
            markdown_preview_theme: content
                .markdown_preview_theme
                .clone()
                .map(ThemeSelection::from),
            theme: theme_selection,
            experimental_theme_overrides: content.experimental_theme_overrides.clone(),
            theme_overrides: content.theme_overrides.clone(),
            icon_theme: icon_theme_selection,
            ui_density: ui_density_from_settings(content.ui_density.unwrap_or_default()),
            unnecessary_code_fade: content.unnecessary_code_fade.unwrap().0.clamp(0.0, 0.9),
            display_profiles,
        }
    }
}

#[cfg(test)]
mod display_profile_tests {
    use super::*;
    use gpui::{AppContext as _, DisplayId, Empty, TestAppContext, TestDisplay, WindowOptions};
    use settings::{DisplayProfileContent, DisplayProfilesContent, FontSize, SettingsStore};
    use std::rc::Rc;
    use uuid::Uuid;

    const LAPTOP_UUID: &str = "00000000-0000-0000-0000-0000000000a1";
    const STUDIO_UUID: &str = "00000000-0000-0000-0000-0000000000b2";

    fn display(id: u64, name: Option<&str>, uuid: &str) -> Rc<TestDisplay> {
        Rc::new(TestDisplay::new_with(
            DisplayId::new(id),
            name.map(str::to_owned),
            Uuid::parse_str(uuid).unwrap(),
        ))
    }

    /// Installs the settings store and the theme settings that read from it.
    ///
    /// `SettingsStore::test` *returns* a store rather than installing one, so the global
    /// has to be set explicitly; `theme_settings::init` is what registers `ThemeSettings`
    /// and seeds the assignment map.
    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            crate::init(theme::LoadThemes::JustBase, cx);
        });
    }

    /// Writes a `display_profiles` block whose profiles each set only `ui_font_size`, then
    /// recomputes the assignments the way the real display-change path does.
    fn write_profiles(cx: &mut TestAppContext, profiles: &[(&str, f32)], assign: &[(&str, &str)]) {
        cx.update_global::<SettingsStore, _>(|store, cx| {
            store.update_user_settings(cx, |content| {
                content.display_profiles = Some(DisplayProfilesContent {
                    profiles: Some(
                        profiles
                            .iter()
                            .map(|(name, size)| {
                                (
                                    name.to_string(),
                                    DisplayProfileContent {
                                        ui_font_size: Some(FontSize(*size)),
                                        ..Default::default()
                                    },
                                )
                            })
                            .collect(),
                    ),
                    assign: Some(
                        assign
                            .iter()
                            .map(|(display, profile)| (display.to_string(), profile.to_string()))
                            .collect(),
                    ),
                });
            });
        });
        cx.update(refresh_display_profile_assignments);
    }

    /// The UI font size a window opened on `display_id` resolves to.
    fn ui_font_size_on(cx: &mut TestAppContext, display_id: u64) -> Pixels {
        let window = cx
            .update(|cx| {
                cx.open_window(
                    WindowOptions {
                        display_id: Some(DisplayId::new(display_id)),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| Empty),
                )
            })
            .unwrap();
        window
            .update(cx, |_, window, cx| {
                ThemeSettings::get_global(cx).ui_font_size_for(window, cx)
            })
            .unwrap()
    }

    /// The UI font size with no profile applied — the value every fallback lands on.
    fn global_ui_font_size(cx: &mut TestAppContext) -> Pixels {
        cx.update(|cx| ThemeSettings::get_global(cx).ui_font_size(cx))
    }

    #[gpui::test]
    fn display_profile_resolves_by_name(cx: &mut TestAppContext) {
        init_test(cx);
        cx.set_displays(vec![display(1, Some("Test Display A"), LAPTOP_UUID)]);
        write_profiles(cx, &[("laptop", 21.)], &[("Test Display A", "laptop")]);

        assert_eq!(ui_font_size_on(cx, 1), px(21.));
    }

    #[gpui::test]
    fn display_profile_uuid_outranks_name(cx: &mut TestAppContext) {
        init_test(cx);
        // Two displays reporting one name is the duplicate-hardware case: the name entry
        // covers both, and a uuid entry is the only way to tell one of them apart.
        cx.set_displays(vec![
            display(1, Some("Duplicate Display"), LAPTOP_UUID),
            display(2, Some("Duplicate Display"), STUDIO_UUID),
        ]);
        write_profiles(
            cx,
            &[("shared", 15.), ("singled-out", 23.)],
            &[
                ("Duplicate Display", "shared"),
                (STUDIO_UUID, "singled-out"),
            ],
        );

        assert_eq!(ui_font_size_on(cx, 1), px(15.));
        assert_eq!(ui_font_size_on(cx, 2), px(23.));
    }

    #[gpui::test]
    fn manual_adjustment_layers_on_each_display_profile(cx: &mut TestAppContext) {
        init_test(cx);
        cx.set_displays(vec![
            display(1, Some("Small Display"), LAPTOP_UUID),
            display(2, Some("Big Display"), STUDIO_UUID),
        ]);
        write_profiles(
            cx,
            &[("small", 13.), ("big", 25.)],
            &[("Small Display", "small"), ("Big Display", "big")],
        );

        assert_eq!(ui_font_size_on(cx, 1), px(13.));
        assert_eq!(ui_font_size_on(cx, 2), px(25.));

        // One `cmd +`. It must move both windows by one step relative to their own
        // profile — not pin both to a single absolute size, and not be swallowed
        // entirely by the profile, which is the defect this guards.
        cx.update(|cx| adjust_ui_font_size(cx, |size| size + px(1.0)));

        assert_eq!(ui_font_size_on(cx, 1), px(14.));
        assert_eq!(ui_font_size_on(cx, 2), px(26.));
    }

    #[gpui::test]
    fn display_profile_fallback_when_display_is_unassigned(cx: &mut TestAppContext) {
        init_test(cx);
        cx.set_displays(vec![
            display(1, Some("Assigned Display"), LAPTOP_UUID),
            display(2, Some("Unassigned Display"), STUDIO_UUID),
        ]);
        write_profiles(cx, &[("laptop", 21.)], &[("Assigned Display", "laptop")]);

        assert_eq!(ui_font_size_on(cx, 2), global_ui_font_size(cx));
    }

    #[gpui::test]
    fn display_profile_fallback_when_assignment_names_a_missing_profile(cx: &mut TestAppContext) {
        init_test(cx);
        cx.set_displays(vec![display(1, Some("Test Display A"), LAPTOP_UUID)]);
        write_profiles(cx, &[("laptop", 21.)], &[("Test Display A", "typo")]);

        assert_eq!(ui_font_size_on(cx, 1), global_ui_font_size(cx));
    }

    #[gpui::test]
    fn display_profile_fallback_when_block_is_absent(cx: &mut TestAppContext) {
        init_test(cx);
        cx.set_displays(vec![display(1, Some("Test Display A"), LAPTOP_UUID)]);
        cx.update(refresh_display_profile_assignments);

        assert_eq!(ui_font_size_on(cx, 1), global_ui_font_size(cx));
    }
}
