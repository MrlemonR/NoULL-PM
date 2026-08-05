# NoULL' PM

A terminal package manager for pacman and the AUR, for Arch.

Two tabs sit above the search bar, switched with `Tab`:

- **Downloaded** — searches the installed packages. Local, instant, no network.
- **AUR + pacman** — searches the repos and the AUR.

The query survives a tab switch, so a term typed once can be looked up in the
other source with a single keypress. The right pane shows the selected
package's description, what it requires (marking which of those are **already
installed**), what requires it, and the full paths of the files it owns.

```
noull-pm
```

## Keys

| Key | What it does |
|---|---|
| `Tab` | switch tabs (Downloaded ↔ AUR + pacman) |
| type | search — instant on Downloaded, 280ms after the last keystroke on the remote tab |
| `↑` `↓` | move through the list |
| `PgUp` `PgDn` | jump ten rows |
| `←` `→` | scroll the right pane (file lists run long) |
| `space` | mark / unmark a package |
| `shift+space` / `ctrl+space` | mark everything between the last mark and the cursor |
| `D` | installs on **AUR + pacman** · opens the delete choices on **Downloaded** |
| `Esc` | clear the query · quit when the query is empty |
| `Ctrl+C` | quit from anywhere |

`D` acts on the marked packages, or on the row under the cursor when nothing is
marked. After an install, the full paths of everything that landed are listed.

`shift+space` needs the kitty keyboard protocol, which the app asks for at
startup with `DISAMBIGUATE_ESCAPE_CODES | REPORT_ALL_KEYS_AS_ESCAPE_CODES` —
the first flag alone is not enough, because space is a plain-text key and the
shift modifier never reaches the app. `ctrl+space` does the same thing and
works on any terminal.

## Deleting

`D` opens two choices:

- **Normal delete** — the package and nothing else. Dependencies are left
  alone and config files are kept as `.pacsave`. (`yay -R`)
- **MEGA delete** — the package, the dependencies this removal leaves
  unneeded, leftovers in `$HOME`, `.pacsave`/`.pacnew` remnants and the
  downloaded package archives.

MEGA delete **always** lists everything it would touch first, scrolls if that
list is longer than the screen, and puts *Cancel* and *Continue* at the bottom
right — `←` `→` to choose, `Enter` to act, `Esc` to back out. Nothing is
truncated: a confirmation that hides part of what it is about to delete is
worse than a long one. When pacman refuses the removal because something else
depends on the package, the reason is shown and *Continue* does nothing.

Home leftovers are looked for under `~/.config`, `~/.cache`,
`~/.local/share` and `~/.local/state`. Matching is not exact: names are
lowercased with everything non-alphanumeric dropped, because the package is
`bitwarden` while the folder is `Bitwarden`, and the package is
`github-desktop` while the folder is `GitHub Desktop`. Reverse-DNS names like
`com.bitwarden.desktop` are caught when the package name appears as a whole
segment.

### Preview without the TUI

```bash
noull-pm --plan <package>
```

Prints what a mega delete would do and exits, touching nothing.

## Theme

The config is written on first run:

```
~/.config/noull-pm/theme.conf
```

It defaults to `theme = auto`, which **follows the desktop theme**: the active
name is read from `~/.config/quickshell/theme.txt`, which is what `qs-theme`
writes. Switching the desktop theme recolours this too, even while the window
is open — both files are re-read once a second. Put a palette name there
instead to pin one.

Shipped palettes: `catppuccin-mocha`, `monochrome`, `gruvbox`, `nord`,
`everforest` — the same colours the rice uses. To add your own, open a new
`[name]` section and fill in the same keys; anything left out falls back to
Catppuccin Mocha. Key names match `palettes.json` so colours can be copied
straight across. The active theme is shown in brackets at the end of the
status line.

> In the config `#` only starts a comment at the **start of a line** — colours
> are `#rrggbb`, so cutting at `#` anywhere blanked every value.

## How it works

- **The installed list** is read at startup with a single `pacman -Qi` (over a
  thousand packages in ~100ms) and served from memory afterwards. Dependency
  and reverse-dependency data comes from there too, with no extra processes.
- **The remote search** is `yay -Ss`. It takes ~1.2s and hits the network, so
  it is debounced and runs on a background thread; the UI never blocks. Late
  replies are dropped by comparing the query they were issued for.
- **Results are sorted by relevance**: exact match, then prefix, then
  substring; on a tie repo before AUR. yay's own output puts the exact match
  last.
- **Installs and removals** run with the TUI suspended, in the plain terminal,
  so sudo can prompt and build output is visible.
- **The installed check counts virtual packages**: 7zip's `sh` dependency is
  provided by bash, not a package of its own, so each package's `Provides` goes
  into the set of installed names.

## Requirements

`pacman`, `yay`, `curl`, a Rust toolchain (`cargo`). `install.sh` checks for
all of these and offers to install `rust` via pacman if it's missing.

## Installing

```bash
git clone https://github.com/MrlemonR/noull-pm.git
cd noull-pm
./install.sh
```

Builds the release binary and installs it to `~/.local/bin/noull-pm`. Safe to
re-run — it just rebuilds and reinstalls.
