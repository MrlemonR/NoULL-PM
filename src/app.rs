//! Application state and key handling.
//!
//! The remote search (`yay -Ss`) takes ~1.2s and hits the network, and so does
//! fetching details for a package that is not installed. Both run on background
//! threads; the UI never blocks. Late replies are discarded by comparing the
//! name or query they were issued for.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use ratatui::widgets::ListState;

use crate::ops::{self, MegaPlan};
use crate::pacman::{self, Pkg};
use crate::theme;

/// How long after the last keystroke the remote search fires.
const DEBOUNCE: Duration = Duration::from_millis(280);

/// How often the theme files are checked for changes. Two `stat` calls.
const THEME_POLL: Duration = Duration::from_millis(1000);

pub enum Msg {
    Search(String, Vec<Pkg>),
    Details(String, Box<Pkg>),
    Files(String, Vec<String>),
    Plan(String, Box<MegaPlan>),
}

/// The two tabs above the search bar.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Search the installed packages — local, instant, no network
    Downloaded,
    /// Search the repos and the AUR (yay -Ss)
    Remote,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Downloaded => "Downloaded",
            Tab::Remote => "AUR + pacman",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Tab::Downloaded => Tab::Remote,
            Tab::Remote => Tab::Downloaded,
        }
    }
}

pub enum Overlay {
    None,
    /// The two choices D opens on the installed tab
    DeleteChoice { cursor: usize },
    /// Confirmation before a mega delete. Scrollable: what it would remove is
    /// routinely longer than the screen.
    MegaConfirm {
        plan: Box<MegaPlan>,
        scroll: u16,
        /// 0 = Cancel, 1 = Continue
        button: usize,
        /// Lines the last render produced, so scrolling can be clamped
        content_lines: u16,
        /// Rows the last render had room for
        view_lines: u16,
    },
    /// After an install: what landed where
    Installed {
        names: Vec<String>,
        files: Vec<String>,
        scroll: u16,
    },
    /// While a plan is being worked out
    Busy(String),
}

pub struct App {
    pub installed: Vec<Pkg>,
    pub installed_names: HashSet<String>,

    pub query: String,
    /// Remote search results (AUR + pacman tab)
    pub results: Vec<Pkg>,
    /// Installed packages filtered by the query (Downloaded tab). Unused while
    /// the query is empty; `installed` is shown directly then so 1000+ records
    /// are not cloned for nothing.
    pub filtered: Vec<Pkg>,
    pub tab: Tab,
    pub search_pending: bool,

    pub state: ListState,
    pub marked: HashSet<String>,
    /// Where the last plain-space mark happened, for Shift+Space range marking
    anchor: Option<usize>,

    /// Full record for the package shown in the right pane
    pub details: Option<Pkg>,
    pub files: Vec<String>,
    pub detail_scroll: u16,

    pub overlay: Overlay,
    pub status: String,

    last_key: Instant,
    last_theme_check: Instant,
    last_searched: String,
    /// Package whose details were already requested, so we ask only once
    detail_requested: String,

    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    quit: bool,
}

impl App {
    /// Names that count as installed: real package names plus the virtual names
    /// they provide.
    ///
    /// NOTE: without `Provides`, 7zip's `sh` dependency showed as missing — `sh`
    /// is provided by bash, not a package of its own.
    fn name_set(pkgs: &[Pkg]) -> HashSet<String> {
        let mut set = HashSet::new();
        for p in pkgs {
            set.insert(p.name.clone());
            for prov in &p.provides {
                set.insert(pacman::dep_name(prov).to_string());
            }
        }
        set
    }

    pub fn new() -> Self {
        let installed = pacman::installed();
        let installed_names = Self::name_set(&installed);
        let (tx, rx) = channel();

        let mut state = ListState::default();
        if !installed.is_empty() {
            state.select(Some(0));
        }

        Self {
            status: format!("{} packages installed", installed.len()),
            installed,
            installed_names,
            query: String::new(),
            results: Vec::new(),
            filtered: Vec::new(),
            tab: Tab::Downloaded,
            search_pending: false,
            state,
            marked: HashSet::new(),
            anchor: None,
            details: None,
            files: Vec::new(),
            detail_scroll: 0,
            overlay: Overlay::None,
            last_key: Instant::now(),
            last_theme_check: Instant::now(),
            last_searched: String::new(),
            detail_requested: String::new(),
            tx,
            rx,
            quit: false,
        }
    }

    /// The list on screen, per the active tab.
    pub fn list(&self) -> &[Pkg] {
        match self.tab {
            Tab::Downloaded => {
                if self.query.is_empty() {
                    &self.installed
                } else {
                    &self.filtered
                }
            }
            Tab::Remote => &self.results,
        }
    }

    pub fn current(&self) -> Option<&Pkg> {
        self.state.selected().and_then(|i| self.list().get(i))
    }

    /// Filter the installed packages by the query. Local work, no network.
    fn refilter(&mut self) {
        if self.tab != Tab::Downloaded || self.query.is_empty() {
            return;
        }
        let q = self.query.to_lowercase();
        self.filtered = self
            .installed
            .iter()
            .filter(|p| {
                p.name.to_lowercase().contains(&q) || p.description.to_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        self.status = format!("{} installed packages match", self.filtered.len());
    }

    /// Switch tabs. The query survives, so a term typed once can be looked up
    /// in the other source with a single keypress.
    fn switch_tab(&mut self) {
        self.tab = self.tab.other();
        self.marked.clear();
        self.anchor = None;
        self.refilter();
        if self.tab == Tab::Remote {
            self.last_key = Instant::now();
        }
        let len = self.list().len();
        self.state.select(if len == 0 { None } else { Some(0) });
        self.reset_detail();
        if self.tab == Tab::Downloaded && self.query.is_empty() {
            self.status = format!("{} packages installed", self.installed.len());
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.quit {
            terminal.draw(|f| crate::ui::draw(f, self))?;

            while let Ok(msg) = self.rx.try_recv() {
                self.handle_msg(msg);
            }

            self.maybe_search();
            self.maybe_load_details();
            self.maybe_reload_theme();

            if event::poll(Duration::from_millis(80))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.on_key(key, terminal);
                    }
                }
            }
        }
        Ok(())
    }

    /// Pick up a desktop theme switch while the window is open.
    fn maybe_reload_theme(&mut self) {
        if self.last_theme_check.elapsed() < THEME_POLL {
            return;
        }
        self.last_theme_check = Instant::now();
        if theme::reload_if_changed() {
            self.status = format!("theme: {}", theme::theme().name);
        }
    }

    fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Search(q, results) => {
                // Stale if the query or the tab moved on in the meantime
                if q != self.query || self.tab != Tab::Remote {
                    return;
                }
                self.search_pending = false;
                self.status = format!("{} results for \"{}\"", results.len(), q);
                self.results = results;
                self.state
                    .select(if self.results.is_empty() { None } else { Some(0) });
                self.reset_detail();
            }
            Msg::Details(name, pkg) => {
                if self.current().map(|p| p.name.as_str()) == Some(name.as_str()) {
                    self.details = Some(*pkg);
                }
            }
            Msg::Files(name, files) => {
                if self.current().map(|p| p.name.as_str()) == Some(name.as_str()) {
                    self.files = files;
                }
            }
            Msg::Plan(key, plan) => {
                // If the targets changed while the plan was being computed, drop
                // it — a confirmation screen for the wrong package ends badly.
                if self.targets().join(",") == key {
                    self.overlay = Overlay::MegaConfirm {
                        plan,
                        scroll: 0,
                        button: 0,
                        content_lines: 0,
                        view_lines: 0,
                    };
                } else {
                    self.overlay = Overlay::None;
                    self.status = "selection changed, mega delete cancelled".into();
                }
            }
        }
    }

    fn reset_detail(&mut self) {
        self.details = None;
        self.files.clear();
        self.detail_requested.clear();
        self.detail_scroll = 0;
    }

    /// Fire the remote search once the query settled.
    fn maybe_search(&mut self) {
        if self.tab != Tab::Remote || self.query.is_empty() || self.query == self.last_searched {
            return;
        }
        if self.last_key.elapsed() < DEBOUNCE {
            return;
        }

        self.last_searched = self.query.clone();
        self.search_pending = true;
        self.status = format!("searching {}…", self.query);

        let query = self.query.clone();
        let names = self.installed_names.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let results = pacman::search(&query, &names);
            let _ = tx.send(Msg::Search(query, results));
        });
    }

    /// Details for the selected package. Installed ones are already in memory
    /// (one `pacman -Qi` at startup), so only the file list costs a process.
    /// For anything else the details come over the network.
    fn maybe_load_details(&mut self) {
        let Some(pkg) = self.current().cloned() else { return };
        if pkg.name == self.detail_requested {
            return;
        }
        self.detail_requested = pkg.name.clone();
        self.details = None;
        self.files.clear();
        self.detail_scroll = 0;

        if pkg.installed {
            let full = self
                .installed
                .iter()
                .find(|p| p.name == pkg.name)
                .cloned()
                .unwrap_or_else(|| pkg.clone());
            self.details = Some(full);

            let name = pkg.name.clone();
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let files = pacman::files(&name);
                let _ = tx.send(Msg::Files(name, files));
            });
        } else {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                if let Some(full) = pacman::remote_details(&pkg) {
                    let _ = tx.send(Msg::Details(pkg.name.clone(), Box::new(full)));
                }
            });
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = self.list().len();
        if len == 0 {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, len as isize - 1);
        self.state.select(Some(next as usize));
    }

    /// Marked packages, or the one under the cursor when nothing is marked.
    fn targets(&self) -> Vec<String> {
        if !self.marked.is_empty() {
            let mut v: Vec<String> = self.marked.iter().cloned().collect();
            v.sort();
            return v;
        }
        self.current()
            .map(|p| vec![p.name.clone()])
            .unwrap_or_default()
    }

    /// Label for the delete dialog: the marked set, or the row under the cursor.
    pub fn delete_targets_label(&self) -> String {
        let t = self.targets();
        match t.len() {
            0 => String::new(),
            1 => t[0].clone(),
            n if n <= 3 => t.join(", "),
            n => format!("{}, {} … ({n} packages)", t[0], t[1]),
        }
    }

    fn toggle_mark(&mut self) {
        let Some(index) = self.state.selected() else { return };
        let Some(name) = self.list().get(index).map(|p| p.name.clone()) else {
            return;
        };
        if !self.marked.remove(&name) {
            self.marked.insert(name);
        }
        self.anchor = Some(index);
    }

    /// Shift+Space: mark everything between the last marked row and this one.
    fn mark_range(&mut self) {
        let Some(index) = self.state.selected() else { return };
        let Some(anchor) = self.anchor else {
            // No anchor yet — behave like a plain mark so the first press is
            // never a no-op.
            self.toggle_mark();
            return;
        };

        let (lo, hi) = if anchor <= index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        let names: Vec<String> = self.list()[lo..=hi].iter().map(|p| p.name.clone()).collect();
        let added = names.len();
        for n in names {
            self.marked.insert(n);
        }
        self.anchor = Some(index);
        self.status = format!("{added} marked in range · {} total", self.marked.len());
    }

    fn on_key(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.quit = true;
            return;
        }

        if !matches!(self.overlay, Overlay::None) {
            self.on_overlay_key(key, terminal);
            return;
        }

        match key.code {
            KeyCode::Esc => {
                if self.query.is_empty() {
                    self.quit = true;
                } else {
                    self.query.clear();
                    self.results.clear();
                    self.filtered.clear();
                    self.last_searched.clear();
                    self.on_query_changed();
                    self.status = format!("{} packages installed", self.installed.len());
                }
            }
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::PageDown => self.move_cursor(10),
            // Scroll the right pane; file lists run long
            KeyCode::Left => self.detail_scroll = self.detail_scroll.saturating_sub(5),
            KeyCode::Right => self.detail_scroll = self.detail_scroll.saturating_add(5),

            KeyCode::Tab | KeyCode::BackTab => self.switch_tab(),

            KeyCode::Backspace => {
                self.query.pop();
                self.last_key = Instant::now();
                self.on_query_changed();
            }

            // NOTE: the kitty keyboard protocol reports a shifted key as the
            // unshifted char plus SHIFT (Shift+d -> 'd' + SHIFT), while a
            // legacy terminal sends 'D' with no modifier. Uppercasing here
            // makes both paths land on the same branch.
            KeyCode::Char(ch) => {
                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let ch = if shift { ch.to_ascii_uppercase() } else { ch };

                match ch {
                    // Space marks (asked-for behaviour), so a space cannot be
                    // typed into the search box. yay -Ss takes a single term
                    // anyway. Shift+Space extends the mark from the last one;
                    // Ctrl+Space does the same on terminals that cannot report
                    // shift on a space.
                    ' ' if shift || ctrl => self.mark_range(),
                    ' ' => self.toggle_mark(),
                    'D' => self.on_d(terminal),
                    _ => {
                        self.query.push(ch);
                        self.last_key = Instant::now();
                        self.on_query_changed();
                    }
                }
            }
            _ => {}
        }
    }

    fn on_query_changed(&mut self) {
        self.refilter();
        if self.tab == Tab::Remote && self.query.is_empty() {
            self.results.clear();
            self.last_searched.clear();
        }
        self.marked.clear();
        self.anchor = None;
        let len = self.list().len();
        self.state.select(if len == 0 { None } else { Some(0) });
        self.reset_detail();
    }

    /// D: installs on the remote tab, opens the delete choices on the
    /// installed one.
    fn on_d(&mut self, terminal: &mut DefaultTerminal) {
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }

        if self.tab == Tab::Remote {
            let to_install: Vec<String> = targets
                .into_iter()
                .filter(|n| !self.installed_names.contains(n))
                .collect();
            if to_install.is_empty() {
                self.status = "already installed".into();
                return;
            }
            ops::install(&to_install);
            *terminal = crate::init_terminal();
            self.after_change();

            // Show where everything landed
            let mut files = Vec::new();
            let installed_now: Vec<String> = to_install
                .iter()
                .filter(|n| self.installed_names.contains(*n))
                .cloned()
                .collect();
            for n in &installed_now {
                files.push(format!("── {n}"));
                files.extend(pacman::files(n));
            }
            self.overlay = Overlay::Installed {
                names: installed_now,
                files,
                scroll: 0,
            };
        } else {
            self.overlay = Overlay::DeleteChoice { cursor: 0 };
        }
    }

    fn start_mega(&mut self, targets: Vec<String>) {
        // The plan shells out to pacman for the cascade and file lists, so it
        // runs off the UI thread.
        self.overlay = Overlay::Busy(format!("{}: working out the plan…", targets.join(", ")));
        let tx = self.tx.clone();
        let key = targets.join(",");
        std::thread::spawn(move || {
            let plan = ops::plan_mega(&targets);
            let _ = tx.send(Msg::Plan(key, Box::new(plan)));
        });
    }

    fn on_overlay_key(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) {
        match &mut self.overlay {
            Overlay::DeleteChoice { cursor } => match key.code {
                KeyCode::Esc => self.overlay = Overlay::None,
                KeyCode::Up | KeyCode::Down => *cursor = 1 - *cursor,
                KeyCode::Enter => {
                    let mega = *cursor == 1;
                    let targets = self.targets();
                    if targets.is_empty() {
                        self.overlay = Overlay::None;
                        return;
                    }
                    if mega {
                        self.start_mega(targets);
                    } else {
                        self.overlay = Overlay::None;
                        ops::remove_normal(&targets);
                        *terminal = crate::init_terminal();
                        self.after_change();
                    }
                }
                _ => {}
            },

            Overlay::MegaConfirm {
                plan,
                scroll,
                button,
                content_lines,
                view_lines,
            } => {
                let page = (*view_lines).max(1);
                let max_scroll = content_lines.saturating_sub(*view_lines);
                match key.code {
                    KeyCode::Esc => self.overlay = Overlay::None,
                    KeyCode::Up => *scroll = scroll.saturating_sub(1),
                    KeyCode::Down => *scroll = (*scroll + 1).min(max_scroll),
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(page),
                    KeyCode::PageDown => *scroll = (*scroll + page).min(max_scroll),
                    KeyCode::Home => *scroll = 0,
                    KeyCode::End => *scroll = max_scroll,
                    KeyCode::Left | KeyCode::Right | KeyCode::Tab => *button = 1 - *button,
                    KeyCode::Enter => {
                        if *button == 1 && plan.error.is_none() {
                            let plan = std::mem::take(plan);
                            self.overlay = Overlay::None;
                            ops::remove_mega(&plan);
                            *terminal = crate::init_terminal();
                            self.after_change();
                        } else {
                            self.overlay = Overlay::None;
                        }
                    }
                    _ => {}
                }
            }

            Overlay::Installed { scroll, .. } => match key.code {
                KeyCode::Esc | KeyCode::Enter => self.overlay = Overlay::None,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll = scroll.saturating_add(1),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(15),
                KeyCode::PageDown => *scroll = scroll.saturating_add(15),
                _ => {}
            },

            Overlay::Busy(_) => {
                if key.code == KeyCode::Esc {
                    self.overlay = Overlay::None;
                }
            }

            Overlay::None => {}
        }
    }

    /// Refresh the installed list after an install or a removal.
    fn after_change(&mut self) {
        self.installed = pacman::installed();
        self.installed_names = Self::name_set(&self.installed);
        self.marked.clear();
        self.anchor = None;
        self.refilter();
        self.reset_detail();

        let len = self.list().len();
        if len == 0 {
            self.state.select(None);
        } else if self.state.selected().unwrap_or(0) >= len {
            self.state.select(Some(len - 1));
        }
        self.status = format!("{} packages installed", self.installed.len());
    }
}
