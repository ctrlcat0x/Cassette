use gpui_kit::component::{Icon, Theme, ThemeMode};
use gpui_kit::*;
use std::borrow::Cow;

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = bundled(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

pub fn icon(name: &str) -> Icon {
    Icon::default().path(format!("raag/icons/{name}.svg"))
}

#[derive(Clone, serde::Deserialize)]
#[non_exhaustive]
pub struct Palette {
    pub id: String,
    pub label: String,
    pub light: bool,
    pub colors: std::collections::HashMap<String, u32>,
}
impl Palette {
    /// Keep the shell behind the page distinct even when a source theme uses
    /// the same background for both roles. Preserve the palette's hue.
    pub fn shell_color(&self) -> u32 {
        let shell = self.color("sidebar");
        let surface = self.color("background");
        if shell != surface {
            return shell;
        }
        let channel = |shift: u32| {
            let value = ((surface >> shift) & 255u32) as f32;
            (if self.light {
                value + (255. - value) * 0.35
            } else {
                value * 0.78
            })
            .round() as u32
        };
        (channel(16) << 16) | (channel(8) << 8) | channel(0)
    }
    pub fn color(&self, token: &str) -> u32 {
        self.colors
            .get(token)
            .copied()
            .unwrap_or(self.colors["foreground"])
    }
}
pub fn palettes() -> &'static [Palette] {
    static PALETTES: std::sync::OnceLock<Vec<Palette>> = std::sync::OnceLock::new();
    PALETTES.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/themes.json"))
            .expect("Bundled theme definitions are valid")
    })
}
pub fn selected(library: &raag::library::Library) -> &'static Palette {
    let id = library
        .theme
        .as_deref()
        .unwrap_or(if library.light { "light" } else { "dark" });
    palettes()
        .iter()
        .find(|palette| palette.id == id)
        .unwrap_or(&palettes()[1])
}
pub const ACCENTS: [(u32, &str); 8] = [
    (0xe11d48, "Rose"),
    (0xea580c, "Orange"),
    (0xca8a04, "Amber"),
    (0x16a34a, "Green"),
    (0x0d9488, "Teal"),
    (0x2563eb, "Blue"),
    (0x7c3aed, "Violet"),
    (0xdb2777, "Pink"),
];
pub fn apply(
    library: &raag::library::Library,
    artwork: Option<raag::artwork::ArtworkColors>,
    cx: &mut App,
) {
    let palette = selected(library);
    let customizable = matches!(palette.id.as_str(), "light" | "dark");
    let primary = if customizable && library.magic_accent {
        artwork
            .map(|colors| colors.accent)
            .unwrap_or(if palette.light { 0 } else { 0xffffff })
    } else if customizable {
        library.accent.unwrap_or(palette.color("primary"))
    } else {
        palette.color("primary")
    };
    let primary = if customizable {
        raag::artwork::readable_accent(primary, palette.color("background"))
    } else {
        primary
    };
    let on_primary = if customizable {
        raag::artwork::on_accent(primary)
    } else {
        palette.color("primary-foreground")
    };
    Theme::change(
        if palette.light {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        },
        None,
        cx,
    );
    Theme::update(cx, |theme| {
        theme.font_size = px(16.);
        theme.radius = px(8.);
        theme.radius_lg = px(20.);
        theme.focus_ring = true;
        theme.font_family = "Inter".into();
        let color = |token| -> Hsla { rgb(palette.color(token)).into() };
        let c = &mut theme.colors;
        c.background = color("background");
        c.foreground = color("foreground");
        c.sidebar = rgb(palette.shell_color()).into();
        c.sidebar_foreground = color("sidebar-foreground");
        c.sidebar_accent = color("sidebar-accent");
        c.sidebar_accent_foreground = color("sidebar-accent-foreground");
        c.sidebar_border = color("sidebar-border");
        c.sidebar_primary = color("sidebar-primary");
        c.sidebar_primary_foreground = color("sidebar-primary-foreground");
        c.muted = color("muted");
        c.muted_foreground = color("muted-foreground");
        c.secondary = color("secondary");
        c.secondary_foreground = color("secondary-foreground");
        c.border = color("border");
        c.input = color("input");
        c.accent = color("accent");
        c.accent_foreground = color("accent-foreground");
        c.popover = color("popover");
        c.popover_foreground = color("popover-foreground");
        c.danger = color("destructive");

        c.danger_foreground = rgb(raag::artwork::on_accent(palette.color("destructive"))).into();
        c.danger_hover = c.danger;
        c.danger_active = c.danger;
        c.button_danger = c.danger;
        c.button_danger_foreground = c.danger_foreground;
        c.button_danger_hover = c.danger;
        c.button_danger_active = c.danger;
        c.primary = rgb(primary).into();
        c.primary_foreground = rgb(on_primary).into();
        c.ring = c.primary;
        c.primary_hover = c.primary;
        c.primary_active = c.primary;
        c.progress_bar = c.primary;
        c.button_primary = c.primary;
        c.button_primary_foreground = c.primary_foreground;
        c.button_primary_hover = c.primary;
        c.button_primary_active = c.primary;
        c.button = color("card");
        c.button_foreground = color("card-foreground");
        c.button_hover = c.muted;
        c.button_active = c.accent;
        c.button_secondary = c.secondary;
        c.button_secondary_foreground = c.secondary_foreground;
        c.button_secondary_hover = c.accent;
        c.button_secondary_active = c.accent;
        c.secondary_hover = c.accent;
        c.secondary_active = c.accent;
        c.accordion = c.background;
        c.list = c.background;
        c.list_hover = c.muted;
        c.list_active = c.accent;
        c.list_active_border = c.primary;
        c.list_head = c.muted;
        c.tab = c.background;
        c.tab_bar = c.background;
        c.tab_foreground = c.muted_foreground;
        c.tab_active = c.accent;
        c.tab_active_foreground = c.foreground;
        c.table = c.background;
        c.table_head = c.muted;
        c.table_head_foreground = c.foreground;
        c.table_hover = c.muted;
        c.table_active = c.accent;
        c.table_row_border = c.border;
        c.title_bar = c.sidebar;
        c.title_bar_border = c.sidebar_border;
        c.slider_bar = c.primary;
        c.slider_thumb = c.foreground;
        c.switch = c.primary;
        c.switch_thumb = c.primary_foreground;
        c.selection = c.primary.opacity(0.25);
        c.link = c.primary;
        c.link_hover = c.primary;
        c.link_active = c.primary;
        c.scrollbar = c.background;
        c.scrollbar_thumb = c.muted_foreground.opacity(0.3);
        c.scrollbar_thumb_hover = c.muted_foreground.opacity(0.5);
        c.chart_1 = color("chart-1");
        c.chart_2 = color("chart-2");
        c.chart_3 = color("chart-3");
        c.chart_4 = color("chart-4");
        c.chart_5 = color("chart-5");
        c.chart_grid = c.border;
    });
}
