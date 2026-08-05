//! NoULL' PM — a terminal package manager for pacman and the AUR.
//!
//! Two tabs, switched with Tab: Downloaded filters the installed packages
//! locally, AUR + pacman searches the repos and the AUR. Space marks,
//! Shift+Space marks a range, D installs on the remote tab and deletes on the
//! installed one. The right pane shows the description, what a package
//! requires (and which of those are already installed), what requires it, and
//! the full paths of the files it owns.

mod app;
mod ops;
mod pacman;
mod theme;
mod ui;

use std::io::stdout;

use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui::DefaultTerminal;

/// Enter the TUI.
///
/// Also asks for the kitty keyboard protocol, because without it a terminal
/// sends a bare space for Shift+Space and range marking cannot be told apart
/// from a plain mark.
///
/// NOTE: DISAMBIGUATE_ESCAPE_CODES alone is not enough. It only escapes keys
/// that would otherwise be ambiguous, and space is a plain-text key — the
/// shift modifier never arrived. REPORT_ALL_KEYS_AS_ESCAPE_CODES is what puts
/// every key, space included, through CSI-u with its modifiers.
///
/// That changes how shifted letters arrive too: the protocol reports Shift+d
/// as `Char('d')` plus SHIFT rather than `Char('D')`, which `App::on_key`
/// normalises. Ctrl+Space works as a fallback on terminals without any of
/// this.
pub fn init_terminal() -> DefaultTerminal {
    let terminal = ratatui::init();
    if matches!(crossterm::terminal::supports_keyboard_enhancement(), Ok(true)) {
        let _ = execute!(
            stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            )
        );
    }
    terminal
}

fn restore_terminal() {
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    ratatui::restore();
}

fn main() -> std::io::Result<()> {
    // Stopping here beats a screen full of empty lists later on
    if pacman::run(&["pacman", "-V"]).is_empty() {
        eprintln!("noull-pm: pacman not found — this tool is for Arch-based systems.");
        std::process::exit(1);
    }
    if pacman::run(&["yay", "--version"]).is_empty() {
        eprintln!("noull-pm: yay not found — the AUR side depends on it.");
        std::process::exit(1);
    }

    // Read the theme config early so the file exists even for --plan runs
    let _ = theme::theme();

    // Preview without the TUI: print what a mega delete would do and exit,
    // touching nothing. Useful from scripts, and answers "what goes if I
    // remove this?" without opening the interface.
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--plan" {
        let targets: Vec<String> = args[2..].to_vec();
        let plan = ops::plan_mega(&targets);
        if let Some(err) = plan.error {
            eprintln!("cannot remove:\n{err}");
            std::process::exit(1);
        }
        println!(
            "packages ({}) — {} files:",
            plan.cascade.len(),
            plan.owned_files
        );
        for p in &plan.cascade {
            println!("  {p}");
        }
        println!("\norphaned deps of these ({}):", plan.orphans.len());
        for p in &plan.orphans {
            println!("  {p}");
        }
        println!("\nhome leftovers ({}):", plan.home_paths.len());
        for p in &plan.home_paths {
            println!("  {}", p.display());
        }
        println!(
            "\nconfig leftover candidates ({}):",
            plan.system_leftovers.len()
        );
        for p in plan.system_leftovers.iter().take(10) {
            println!("  {}", p.display());
        }
        println!("\ncached archives ({}):", plan.cache_files.len());
        for p in &plan.cache_files {
            println!("  {}", p.display());
        }
        return Ok(());
    }

    let mut terminal = init_terminal();
    let mut app = app::App::new();
    let result = app.run(&mut terminal);
    restore_terminal();
    result
}
