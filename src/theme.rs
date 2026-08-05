//! Colours and the theme config.
//!
//! Config file: ~/.config/noull-pm/theme.conf — written on first run with
//! every palette the rice already ships.
//!
//! `theme = auto` (the default) follows the system theme by reading the active
//! name from ~/.config/quickshell/theme.txt, which is what `qs-theme` writes.
//! So switching the desktop theme switches this too, without touching anything
//! here. Naming a palette explicitly pins it instead.
//!
//! Both files are re-read while running, so a theme switch lands on an already
//! open window — `reload_if_changed` is called from the event loop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use ratatui::style::Color;

pub struct Theme {
    pub name: String,
    pub base: Color,
    pub mantle: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub overlay0: Color,
    pub subtext0: Color,
    pub text: Color,
    pub mauve: Color,
    pub blue: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub peach: Color,
    pub hover: Color,
    pub selected: Color,
}

impl Default for Theme {
    /// Used when the config is missing or unreadable, so the UI is never
    /// colourless.
    fn default() -> Self {
        Self {
            name: "catppuccin-mocha".into(),
            base: rgb("#1e1e2e"),
            mantle: rgb("#181825"),
            surface0: rgb("#313244"),
            surface1: rgb("#45475a"),
            overlay0: rgb("#6c7086"),
            subtext0: rgb("#a6adc8"),
            text: rgb("#cdd6f4"),
            mauve: rgb("#cba6f7"),
            blue: rgb("#89b4fa"),
            red: rgb("#f38ba8"),
            green: rgb("#a6e3a1"),
            yellow: rgb("#f9e2af"),
            peach: rgb("#fab387"),
            hover: rgb("#232336"),
            selected: rgb("#2a2b3d"),
        }
    }
}

static THEME: RwLock<Option<Arc<Theme>>> = RwLock::new(None);
static STAMPS: RwLock<Option<(Option<SystemTime>, Option<SystemTime>)>> = RwLock::new(None);

/// The active theme.
///
/// Returns an `Arc` rather than a `&'static` because the theme can change
/// while running; call sites keep working unchanged thanks to `Deref`.
pub fn theme() -> Arc<Theme> {
    if let Some(t) = THEME.read().ok().and_then(|g| g.clone()) {
        return t;
    }
    reload()
}

fn reload() -> Arc<Theme> {
    let t = Arc::new(load());
    if let Ok(mut g) = THEME.write() {
        *g = Some(t.clone());
    }
    if let Ok(mut g) = STAMPS.write() {
        *g = Some(stamps());
    }
    t
}

/// Re-read the config if either file changed on disk. Cheap: two `stat` calls.
/// Returns true when the theme actually changed, so the caller can redraw.
pub fn reload_if_changed() -> bool {
    let now = stamps();
    let previous = STAMPS.read().ok().and_then(|g| *g);
    if previous == Some(now) {
        return false;
    }
    let before = theme().name.clone();
    let after = reload();
    before != after.name
}

fn stamps() -> (Option<SystemTime>, Option<SystemTime>) {
    let mtime = |p: PathBuf| std::fs::metadata(p).ok().and_then(|m| m.modified().ok());
    (mtime(config_path()), mtime(system_theme_path()))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub fn config_path() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".config"))
        .join("noull-pm")
        .join("theme.conf")
}

/// Where `qs-theme` records the active desktop theme.
fn system_theme_path() -> PathBuf {
    home().join(".config").join("quickshell").join("theme.txt")
}

fn rgb(hex: &str) -> Color {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return Color::Reset;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
    Color::Rgb(p(0), p(2), p(4))
}

fn load() -> Theme {
    let path = config_path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, DEFAULT_CONFIG);
    }

    let Ok(text) = std::fs::read_to_string(&path) else {
        return Theme::default();
    };
    parse(&text)
}

/// Simple sectioned config:
///
///     theme = nord
///     [nord]
///     base = #2e3440
///
/// Too small a job to pull in a TOML parser, and easier to hand-edit this way.
fn parse(text: &str) -> Theme {
    let mut requested = String::new();
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();

    for raw in text.lines() {
        // NOTE: '#' starts a comment, but colours are '#rrggbb' too. Cutting at
        // '#' anywhere blanked every colour value, which left the theme name
        // reading correctly while nothing was actually recoloured. Comments
        // only count at the start of a line.
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.trim().to_string();
            sections.entry(current.clone()).or_default();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_string(), v.trim().to_string());

        if current.is_empty() {
            if k == "theme" {
                requested = v;
            }
        } else {
            sections.entry(current.clone()).or_default().insert(k, v);
        }
    }

    // "auto" follows the desktop theme. If that name has no section here, fall
    // back to the first one defined rather than losing all colour.
    let mut active = requested.clone();
    if active.is_empty() || active == "auto" {
        active = system_theme_name().unwrap_or_default();
    }
    if !sections.contains_key(&active) {
        if let Some(first) = sections.keys().next() {
            active = first.clone();
        }
    }

    let fallback = Theme::default();
    let Some(pal) = sections.get(&active) else {
        return fallback;
    };

    let pick = |key: &str, default: Color| pal.get(key).map(|h| rgb(h)).unwrap_or(default);

    Theme {
        name: active.clone(),
        base: pick("base", fallback.base),
        mantle: pick("mantle", fallback.mantle),
        surface0: pick("surface0", fallback.surface0),
        surface1: pick("surface1", fallback.surface1),
        overlay0: pick("overlay0", fallback.overlay0),
        subtext0: pick("subtext0", fallback.subtext0),
        text: pick("text", fallback.text),
        mauve: pick("mauve", fallback.mauve),
        blue: pick("blue", fallback.blue),
        red: pick("red", fallback.red),
        green: pick("green", fallback.green),
        yellow: pick("yellow", fallback.yellow),
        peach: pick("peach", fallback.peach),
        hover: pick("hover", fallback.hover),
        selected: pick("selected", fallback.selected),
    }
}

fn system_theme_name() -> Option<String> {
    let name = std::fs::read_to_string(system_theme_path()).ok()?;
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Written on first run. Ships the rice's palettes so the tool matches the
/// desktop out of the box.
pub const DEFAULT_CONFIG: &str = r#"# NoULL' PM theme config
#
# "auto" follows the desktop theme: the active name is read from
# ~/.config/quickshell/theme.txt, which is what qs-theme writes. Switching the
# desktop theme switches this too, even while the app is open. Put a palette
# name here instead to pin it.
#
# To add your own, open a new [name] section and fill in the same keys; any key
# you leave out falls back to Catppuccin Mocha. Key names match
# ~/.config/quickshell/palettes.json so colours can be copied straight across.
#
# Colours are #rrggbb. Where each is used:
#   base      background
#   mantle    list row background
#   surface0  borders
#   surface1  thin separators, secondary fills
#   overlay0  dim text (file paths, help line)
#   subtext0  secondary text
#   text      primary text
#   mauve     accent: titles, active tab, cursor
#   blue      section headings, repo badge
#   red       danger: MEGA delete, missing dependency
#   green     positive: installed, marked
#   yellow    orphaned packages
#   peach     AUR badge
#   hover     hovered row
#   selected  selected row

theme = auto

[catppuccin-mocha]
base     = #1e1e2e
mantle   = #181825
surface0 = #313244
surface1 = #45475a
overlay0 = #6c7086
subtext0 = #a6adc8
text     = #cdd6f4
mauve    = #cba6f7
blue     = #89b4fa
red      = #f38ba8
green    = #a6e3a1
yellow   = #f9e2af
peach    = #fab387
hover    = #232336
selected = #2a2b3d

[monochrome]
base     = #0f0f0f
mantle   = #0a0a0a
surface0 = #1e1e1e
surface1 = #2e2e2e
overlay0 = #6e6e6e
subtext0 = #9a9a9a
text     = #ebebeb
mauve    = #e0e0e0
blue     = #a8a8a8
red      = #f5f5f5
green    = #c2c2c2
yellow   = #d6d6d6
peach    = #e8e8e8
hover    = #171717
selected = #262626

[gruvbox]
base     = #282828
mantle   = #1d2021
surface0 = #3c3836
surface1 = #504945
overlay0 = #7c6f64
subtext0 = #a89984
text     = #ebdbb2
mauve    = #d3869b
blue     = #83a598
red      = #fb4934
green    = #b8bb26
yellow   = #fabd2f
peach    = #fe8019
hover    = #32302f
selected = #3c3836

[nord]
base     = #2e3440
mantle   = #272c36
surface0 = #3b4252
surface1 = #434c5e
overlay0 = #616e88
subtext0 = #c8ced9
text     = #eceff4
mauve    = #88c0d0
blue     = #81a1c1
red      = #bf616a
green    = #a3be8c
yellow   = #ebcb8b
peach    = #d08770
hover    = #353b47
selected = #3b4252

[everforest]
base     = #2d353b
mantle   = #272e33
surface0 = #343f44
surface1 = #3d484d
overlay0 = #7a8478
subtext0 = #9da9a0
text     = #d3c6aa
mauve    = #a7c080
blue     = #7fbbb3
red      = #e67e80
green    = #a7c080
yellow   = #dbbc7f
peach    = #e69875
hover    = #323c41
selected = #3d484d
"#;
