//! Everything that CHANGES the system lives here.
//!
//! Two rules:
//!   1. Nothing runs without passing through a confirmation screen.
//!   2. Every operation first lists exactly what it will do (the plan), then
//!      carries it out only if the user confirms.
//!
//! Commands that need privileges run with the TUI suspended, in the plain
//! terminal, so sudo can prompt for a password and build output is visible.

use std::io::{Write, stdin, stdout};
use std::path::PathBuf;
use std::process::Command;

use crate::pacman;

/// What a mega delete would do. Worked out and shown first, applied only
/// after confirmation.
#[derive(Debug, Default)]
pub struct MegaPlan {
    /// Target package(s) — several can be marked at once
    pub packages: Vec<String>,
    /// The pacman -Rs cascade: the package plus deps it leaves unneeded
    pub cascade: Vec<String>,
    /// How many files the packages own (pacman -Ql)
    pub owned_files: usize,
    /// Orphans left behind that these packages depended on
    pub orphans: Vec<String>,
    /// Leftovers under ~/.config, ~/.cache, ~/.local/share, ~/.local/state
    pub home_paths: Vec<PathBuf>,
    /// .pacsave/.pacnew candidates that removal may leave under /etc
    pub system_leftovers: Vec<PathBuf>,
    /// Downloaded archives in /var/cache/pacman/pkg
    pub cache_files: Vec<PathBuf>,
    /// Why the dry run failed, if it did. Set means confirmation is refused.
    pub error: Option<String>,
}

impl MegaPlan {
    pub fn total_extra(&self) -> usize {
        self.orphans.len()
            + self.home_paths.len()
            + self.system_leftovers.len()
            + self.cache_files.len()
    }
}

/// The names a package might use for its directories in $HOME. AUR packages
/// carry suffixes like "-bin" or "-git" while the config folder is usually
/// created under the bare name.
fn name_variants(name: &str) -> Vec<String> {
    let mut v = vec![name.to_string()];
    for suffix in ["-bin", "-git", "-appimage", "-debug", "-electron", "-desktop", "-app"] {
        if let Some(base) = name.strip_suffix(suffix) {
            if !base.is_empty() && !v.contains(&base.to_string()) {
                v.push(base.to_string());
            }
        }
    }
    v
}

/// Normalise for comparison: lowercase, drop anything not alphanumeric.
/// "GitHub Desktop" and "github-desktop" collapse to the same thing.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Does a directory name belong to this package?
///
/// NOTE: exact matching found nothing — the package is "bitwarden" but the
/// folder is "Bitwarden", the package is "github-desktop" but the folder is
/// "GitHub Desktop". Both sides are normalised before comparing. Reverse-DNS
/// names like "com.bitwarden.desktop" count too when the package name appears
/// as a whole segment.
fn entry_matches(entry: &str, variants: &[String]) -> bool {
    let e = normalize(entry);
    if variants.iter().any(|v| normalize(v) == e) {
        return true;
    }
    let segments: Vec<&str> = entry.split('.').collect();
    if segments.len() >= 3 {
        return segments
            .iter()
            .any(|seg| variants.iter().any(|v| normalize(v) == normalize(seg)));
    }
    false
}

/// Work out the mega delete plan. Changes nothing.
pub fn plan_mega(names: &[String]) -> MegaPlan {
    let mut plan = MegaPlan {
        packages: names.to_vec(),
        ..Default::default()
    };

    // NOTE: --nosave and --print cannot be combined ("may not be used
    // together"). -n only decides whether config files are kept, not which
    // packages go, so the preview uses -Rs and the real run uses -Rns.
    let mut args: Vec<&str> = vec!["pacman", "-Rs", "--print", "--print-format", "%n"];
    args.extend(names.iter().map(|s| s.as_str()));
    let (ok, stdout, stderr) = pacman::run_status(&args);

    if !ok {
        // pacman puts the detail on stdout and the "error:" line on stderr;
        // show both.
        let mut msg: Vec<String> = stderr
            .lines()
            .chain(stdout.lines())
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        msg.dedup();
        plan.error = Some(msg.join("\n"));
        return plan;
    }

    plan.cascade = stdout
        .lines()
        .map(|s| s.trim().to_string())
        // Package names have no spaces — an error line slipping through dies here
        .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
        .collect();

    let owned: Vec<String> = names.iter().flat_map(|n| pacman::files(n)).collect();
    plan.owned_files = owned.len();

    // After removal, config files under /etc are left behind as .pacsave.
    // They do not exist yet (the package is still installed), so these are
    // listed as candidates.
    for path in &owned {
        if path.starts_with("/etc/") && !path.ends_with('/') {
            plan.system_leftovers.push(PathBuf::from(format!("{path}.pacsave")));
        }
    }

    // Orphans: ONLY the ones related to these packages.
    //
    // NOTE: the whole of `pacman -Qtdq` used to go in here, so deleting "ada"
    // listed 20 completely unrelated packages — rust, qemu-system-aarch64,
    // nvm, pnpm — as about to be removed. -Rs already cascades the deps this
    // removal leaves unneeded; what belongs here is only packages that are
    // already orphaned AND are a dependency of what is going.
    let system_orphans = pacman::orphans();
    if !system_orphans.is_empty() {
        let mut related: Vec<String> = Vec::new();
        for target in &plan.cascade {
            for dep in pacman::deps_of(target) {
                let d = pacman::dep_name(&dep).to_string();
                if system_orphans.contains(&d)
                    && !plan.cascade.contains(&d)
                    && !related.contains(&d)
                {
                    related.push(d);
                }
            }
        }
        plan.orphans = related;
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let variants: Vec<String> = names.iter().flat_map(|n| name_variants(n)).collect();
    for base in [".config", ".cache", ".local/share", ".local/state"] {
        let dir = PathBuf::from(&home).join(base);
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            if entry_matches(&fname, &variants) {
                let p = e.path();
                if !plan.home_paths.contains(&p) {
                    plan.home_paths.push(p);
                }
            }
        }
    }
    plan.home_paths.sort();

    // Cached archives for the WHOLE cascade, not just the named targets
    if let Ok(entries) = std::fs::read_dir("/var/cache/pacman/pkg") {
        let files: Vec<(String, PathBuf)> = entries
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect();
        for name in &plan.cascade {
            let prefix = format!("{name}-");
            for (fname, path) in &files {
                // "ripgrep-15.2.0-1-x86_64.pkg.tar.zst" — name + '-' + version.
                // Requiring a digit after the dash rules out neighbours like
                // "ripgrep-all".
                if fname.starts_with(&prefix)
                    && fname[prefix.len()..].starts_with(|c: char| c.is_ascii_digit())
                    && !plan.cache_files.contains(path)
                {
                    plan.cache_files.push(path.clone());
                }
            }
        }
    }

    plan
}

/// Suspend the TUI and run the commands in the plain terminal. stdio is
/// inherited so sudo can prompt and build output is visible.
fn run_interactive(title: &str, cmds: Vec<Vec<String>>) {
    ratatui::restore();

    println!("\n\x1b[1;35m▌ {title}\x1b[0m\n");

    let mut failed = false;
    for cmd in cmds {
        if cmd.is_empty() {
            continue;
        }
        println!("\x1b[2m$ {}\x1b[0m", cmd.join(" "));
        let status = Command::new(&cmd[0]).args(&cmd[1..]).status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                println!("\x1b[1;31m  exited with {}\x1b[0m", s);
                failed = true;
            }
            Err(e) => {
                println!("\x1b[1;31m  could not run: {e}\x1b[0m");
                failed = true;
            }
        }
    }

    if failed {
        println!("\n\x1b[1;33mSome steps did not finish — see above.\x1b[0m");
    }
    print!("\n\x1b[1mPress Enter to continue\x1b[0m");
    let _ = stdout().flush();
    let mut buf = String::new();
    let _ = stdin().read_line(&mut buf);
}

/// Install the chosen packages. yay covers both the repos and the AUR.
pub fn install(names: &[String]) {
    if names.is_empty() {
        return;
    }
    let mut cmd = vec!["yay".to_string(), "-S".to_string(), "--needed".to_string()];
    cmd.extend(names.iter().cloned());
    run_interactive(&format!("Installing: {}", names.join(", ")), vec![cmd]);
}

/// Normal removal: the package itself and nothing else.
///
/// No -s, so dependencies are left alone, and config files are kept as
/// .pacsave. That is precisely what separates it from a mega delete.
pub fn remove_normal(names: &[String]) {
    if names.is_empty() {
        return;
    }
    let mut cmd = vec!["yay".to_string(), "-R".to_string()];
    cmd.extend(names.iter().cloned());
    run_interactive(&format!("Removing: {}", names.join(", ")), vec![cmd]);
}

/// Mega removal: the packages, the deps they leave unneeded, home leftovers,
/// .pacsave remnants and the cached archives.
pub fn remove_mega(plan: &MegaPlan) {
    let mut cmds: Vec<Vec<String>> = Vec::new();

    // 1) Packages and cascade, without keeping config files
    let mut first: Vec<String> = vec!["yay".into(), "-Rns".into(), "--noconfirm".into()];
    first.extend(plan.packages.iter().cloned());
    cmds.push(first);

    // 2) Orphans left behind
    if !plan.orphans.is_empty() {
        let mut c: Vec<String> = vec!["sudo".into(), "pacman".into(), "-Rns".into(), "--noconfirm".into()];
        c.extend(plan.orphans.iter().cloned());
        cmds.push(c);
    }

    // 3) Leftovers in $HOME (no root needed)
    for p in &plan.home_paths {
        cmds.push(vec!["rm".into(), "-rf".into(), "--".into(), p.display().to_string()]);
    }

    // 4) .pacsave/.pacnew under /etc. They only appear AFTER the removal, so
    //    -f keeps this from failing with "no such file".
    if !plan.system_leftovers.is_empty() {
        let mut c: Vec<String> = vec!["sudo".into(), "rm".into(), "-f".into(), "--".into()];
        for p in &plan.system_leftovers {
            c.push(p.display().to_string());
            c.push(format!("{}", p.with_extension("pacnew").display()));
        }
        cmds.push(c);
    }

    // 5) Downloaded archives
    if !plan.cache_files.is_empty() {
        let mut c: Vec<String> = vec!["sudo".into(), "rm".into(), "-f".into(), "--".into()];
        c.extend(plan.cache_files.iter().map(|p| p.display().to_string()));
        cmds.push(c);
    }

    run_interactive(&format!("MEGA DELETE: {}", plan.packages.join(", ")), cmds);
}
