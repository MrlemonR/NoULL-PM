//! Rendering. Layout: list on the left, details on the right, tabs and the
//! search box at the bottom.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Overlay, Tab};
use crate::ops::MegaPlan;
use crate::pacman::dep_name;
use crate::theme::theme;

pub fn draw(f: &mut Frame, app: &mut App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // list + details
            Constraint::Length(1), // tabs
            Constraint::Length(3), // search
            Constraint::Length(1), // status / help
        ])
        .split(f.area());

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(root[0]);

    draw_list(f, app, panes[0]);
    draw_details(f, app, panes[1]);
    draw_tabs(f, app, root[1]);
    draw_search(f, app, root[2]);
    draw_status(f, app, root[3]);

    draw_overlay(f, app);
}

fn draw_overlay(f: &mut Frame, app: &mut App) {
    // The mega dialog reports back how tall its content was and how many rows
    // it had, so key handling can clamp scrolling. Collected while the overlay
    // is borrowed immutably, written back afterwards.
    let mut measured: Option<(u16, u16)> = None;

    match &app.overlay {
        Overlay::None => {}
        Overlay::DeleteChoice { cursor } => draw_delete_choice(f, app, *cursor),
        Overlay::MegaConfirm {
            plan,
            scroll,
            button,
            ..
        } => {
            measured = Some(draw_mega_confirm(f, plan, *scroll, *button));
        }
        Overlay::Installed {
            names,
            files,
            scroll,
        } => draw_installed(f, names, files, *scroll),
        Overlay::Busy(msg) => draw_busy(f, msg),
    }

    if let (
        Some((content, view)),
        Overlay::MegaConfirm {
            content_lines,
            view_lines,
            ..
        },
    ) = (measured, &mut app.overlay)
    {
        *content_lines = content;
        *view_lines = view;
    }
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let c = theme();
    let title = match app.tab {
        Tab::Downloaded => format!(" Installed ({}) ", app.list().len()),
        Tab::Remote => format!(" Results ({}) ", app.list().len()),
    };

    let items: Vec<ListItem> = app
        .list()
        .iter()
        .map(|p| {
            let marked = app.marked.contains(&p.name);
            let mut spans = vec![
                Span::styled(if marked { "▐ " } else { "  " }, Style::default().fg(c.green)),
                Span::styled(
                    p.name.clone(),
                    Style::default().fg(if p.installed { c.text } else { c.subtext0 }),
                ),
                Span::raw(" "),
                Span::styled(p.version.clone(), Style::default().fg(c.overlay0)),
            ];
            if p.is_aur() {
                spans.push(Span::styled("  aur", Style::default().fg(c.peach)));
            }
            if p.installed && app.tab == Tab::Remote {
                spans.push(Span::styled("  installed", Style::default().fg(c.green)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(c.surface0))
                .title(Span::styled(
                    title,
                    Style::default().fg(c.mauve).add_modifier(Modifier::BOLD),
                )),
        )
        .highlight_style(Style::default().bg(c.selected).add_modifier(Modifier::BOLD));

    f.render_stateful_widget(list, area, &mut app.state);
}

fn draw_details(f: &mut Frame, app: &App, area: Rect) {
    let c = theme();
    let mut lines: Vec<Line> = Vec::new();

    let Some(cur) = app.current() else {
        f.render_widget(Paragraph::new("").block(detail_block(" Details ")), area);
        return;
    };

    // Fall back to what the list knows until the full record arrives
    let d = app.details.as_ref().unwrap_or(cur);

    lines.push(Line::from(vec![
        Span::styled(
            d.name.clone(),
            Style::default().fg(c.mauve).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(d.version.clone(), Style::default().fg(c.overlay0)),
    ]));

    let mut badges: Vec<Span> = vec![Span::styled(
        if cur.is_aur() { "AUR" } else { "repo" },
        Style::default().fg(if cur.is_aur() { c.peach } else { c.blue }),
    )];
    if cur.installed {
        badges.push(Span::raw("  ·  "));
        badges.push(Span::styled("installed", Style::default().fg(c.green)));
    }
    if let Some(v) = &d.votes {
        badges.push(Span::raw("  ·  "));
        badges.push(Span::styled(
            format!("{v} votes"),
            Style::default().fg(c.yellow),
        ));
    }
    if !d.size.is_empty() {
        badges.push(Span::raw("  ·  "));
        badges.push(Span::styled(d.size.clone(), Style::default().fg(c.overlay0)));
    }
    lines.push(Line::from(badges));
    lines.push(Line::raw(""));

    if !d.description.is_empty() {
        lines.push(Line::styled(
            d.description.clone(),
            Style::default().fg(c.text),
        ));
        lines.push(Line::raw(""));
    }
    if !d.url.is_empty() {
        lines.push(Line::styled(d.url.clone(), Style::default().fg(c.blue)));
        lines.push(Line::raw(""));
    }

    if app.details.is_none() && !cur.installed {
        lines.push(Line::styled("  loading…", Style::default().fg(c.overlay0)));
    }

    // --- Dependencies, marking which are already installed ---
    section(&mut lines, "Requires");
    if d.depends.is_empty() {
        lines.push(Line::styled("  none", Style::default().fg(c.overlay0)));
    } else {
        for raw in &d.depends {
            let have = app.installed_names.contains(dep_name(raw));
            lines.push(Line::from(vec![
                Span::styled(
                    if have { "  ✓ " } else { "  ✗ " },
                    Style::default().fg(if have { c.green } else { c.red }),
                ),
                Span::styled(
                    raw.clone(),
                    Style::default().fg(if have { c.subtext0 } else { c.text }),
                ),
                Span::styled(
                    if have { "" } else { "   will be installed" },
                    Style::default().fg(c.overlay0),
                ),
            ]));
        }
    }

    if !d.optdepends.is_empty() {
        section(&mut lines, "Optional");
        for raw in &d.optdepends {
            let have = app.installed_names.contains(dep_name(raw));
            lines.push(Line::from(vec![
                Span::styled(
                    if have { "  ✓ " } else { "  · " },
                    Style::default().fg(if have { c.green } else { c.overlay0 }),
                ),
                Span::styled(raw.clone(), Style::default().fg(c.subtext0)),
            ]));
        }
    }

    // --- What depends on this package ---
    if cur.installed {
        section(&mut lines, "Required by");
        if d.required_by.is_empty() {
            lines.push(Line::styled("  nothing", Style::default().fg(c.overlay0)));
        } else {
            for r in &d.required_by {
                lines.push(Line::styled(
                    format!("  {r}"),
                    Style::default().fg(c.subtext0),
                ));
            }
        }

        section(&mut lines, &format!("Files ({})", app.files.len()));
        if app.files.is_empty() {
            lines.push(Line::styled("  reading…", Style::default().fg(c.overlay0)));
        } else {
            for path in &app.files {
                lines.push(Line::styled(
                    format!("  {path}"),
                    Style::default().fg(c.overlay0),
                ));
            }
        }
    }

    let p = Paragraph::new(lines)
        .block(detail_block(" Details "))
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    f.render_widget(p, area);
}

fn detail_block(title: &str) -> Block<'_> {
    let c = theme();
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(c.surface0))
        .title(Span::styled(
            title.to_string(),
            Style::default().fg(c.mauve).add_modifier(Modifier::BOLD),
        ))
}

fn section(lines: &mut Vec<Line>, title: &str) {
    let c = theme();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        title.to_string(),
        Style::default().fg(c.blue).add_modifier(Modifier::BOLD),
    ));
}

/// The tabs above the search bar, switched with Tab.
fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let c = theme();
    let mut spans = vec![Span::raw(" ")];

    for tab in [Tab::Downloaded, Tab::Remote] {
        let on = app.tab == tab;
        spans.push(Span::styled(
            format!(" {} ", tab.label()),
            if on {
                Style::default()
                    .bg(c.mauve)
                    .fg(c.base)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(c.overlay0)
            },
        ));
        spans.push(Span::raw(" "));
    }

    spans.push(Span::styled("  ⇥ Tab", Style::default().fg(c.surface1)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let c = theme();
    let spinner = if app.search_pending { " …" } else { "" };
    let content = if app.query.is_empty() {
        Line::from(vec![
            Span::styled("  ", Style::default().fg(c.mauve)),
            Span::styled(
                match app.tab {
                    Tab::Downloaded => "type to filter installed packages",
                    Tab::Remote => "type to search pacman + AUR",
                },
                Style::default().fg(c.surface1),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("  ", Style::default().fg(c.mauve)),
            Span::styled(
                app.query.clone(),
                Style::default().fg(c.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(c.mauve)),
            Span::styled(spinner.to_string(), Style::default().fg(c.overlay0)),
        ])
    };

    f.render_widget(
        Paragraph::new(content).block(
            Block::default().borders(Borders::ALL).border_style(
                Style::default().fg(if app.query.is_empty() {
                    c.surface1
                } else {
                    c.mauve
                }),
            ),
        ),
        area,
    );
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let c = theme();
    let marked = app.marked.len();
    let help = match app.tab {
        Tab::Remote => "Tab switch · space mark · shift+space range · D install · Esc clear",
        Tab::Downloaded => "Tab switch · space mark · shift+space range · D delete · Esc quit",
    };

    let mut spans = vec![Span::styled(
        format!(" {} ", app.status),
        Style::default().fg(c.subtext0),
    )];
    if marked > 0 {
        spans.push(Span::styled(
            format!("· {marked} marked "),
            Style::default().fg(c.green).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(
        format!("· {help}"),
        Style::default().fg(c.overlay0),
    ));
    spans.push(Span::styled(
        format!("  [{}]", c.name),
        Style::default().fg(c.surface1),
    ));

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

fn popup(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1])[1]
}

/// A right-aligned Cancel / Continue pair.
fn buttons(selected: usize, accent: Color, enabled: bool) -> Line<'static> {
    let c = theme();
    let chip = |label: &str, index: usize, colour: Color| {
        let on = index == selected;
        Span::styled(
            format!("  {label}  "),
            if on {
                Style::default()
                    .bg(colour)
                    .fg(c.base)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(c.subtext0)
            },
        )
    };

    Line::from(vec![
        chip("Cancel", 0, c.surface1),
        Span::raw("  "),
        chip("Continue", 1, if enabled { accent } else { c.surface1 }),
        Span::raw(" "),
    ])
    .alignment(Alignment::Right)
}

fn draw_delete_choice(f: &mut Frame, app: &App, cursor: usize) {
    let c = theme();
    let name = app.delete_targets_label();
    let area = popup(f.area(), 62, 34);
    f.render_widget(Clear, area);

    let pick = |i: usize, label: &str, desc: &str, colour: Color| -> Vec<Line<'static>> {
        let on = i == cursor;
        vec![
            Line::from(vec![
                Span::styled(if on { "  ▸ " } else { "    " }, Style::default().fg(colour)),
                Span::styled(
                    label.to_string(),
                    Style::default()
                        .fg(if on { colour } else { c.subtext0 })
                        .add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }),
                ),
            ]),
            Line::styled(format!("      {desc}"), Style::default().fg(c.overlay0)),
            Line::raw(""),
        ]
    };

    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                name,
                Style::default().fg(c.text).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
    ];
    lines.extend(pick(
        0,
        "Normal delete",
        "only this package, dependencies and configs stay",
        c.blue,
    ));
    lines.extend(pick(
        1,
        "MEGA delete",
        "package + unused deps + home leftovers + .pacsave + cache",
        c.red,
    ));
    lines.push(Line::styled(
        "  ↑↓ choose · Enter confirm · Esc cancel",
        Style::default().fg(c.overlay0),
    ));

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(c.mauve))
                .style(Style::default().bg(c.base))
                .title(Span::styled(
                    " Delete ",
                    Style::default().fg(c.mauve).add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

/// Returns (content lines, visible rows) so key handling can clamp scrolling.
fn draw_mega_confirm(f: &mut Frame, plan: &MegaPlan, scroll: u16, button: usize) -> (u16, u16) {
    let c = theme();
    let area = popup(f.area(), 82, 82);
    f.render_widget(Clear, area);

    let refused = plan.error.is_some();
    let title = if refused {
        format!(" Cannot remove: {} ", plan.packages.join(", "))
    } else {
        format!(
            " MEGA delete: {} · {} pkg + {} extra ",
            plan.packages.join(", "),
            plan.cascade.len(),
            plan.total_extra()
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(c.red))
        .style(Style::default().bg(c.base))
        .title(Span::styled(
            title,
            Style::default().fg(c.red).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Body above, buttons pinned to the bottom
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(err) = &plan.error {
        lines.push(Line::styled(
            "pacman refused this removal:",
            Style::default().fg(c.red).add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));
        for l in err.lines() {
            lines.push(Line::styled(format!("  {l}"), Style::default().fg(c.text)));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "  Nothing was changed.",
            Style::default().fg(c.subtext0),
        ));
    } else {
        lines.push(Line::styled(
            "This will permanently remove:",
            Style::default().fg(c.red).add_modifier(Modifier::BOLD),
        ));

        // NOTE: nothing is truncated any more — the dialog scrolls instead. A
        // confirmation that hides part of what it is about to delete is worse
        // than a long one.
        let mut group = |title: String, items: Vec<String>, colour: Color| {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                title,
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ));
            if items.is_empty() {
                lines.push(Line::styled("  none", Style::default().fg(c.overlay0)));
            }
            for i in &items {
                lines.push(Line::styled(
                    format!("  {i}"),
                    Style::default().fg(c.subtext0),
                ));
            }
        };

        group(
            format!(
                "Packages ({}) — {} files",
                plan.cascade.len(),
                plan.owned_files
            ),
            plan.cascade.clone(),
            c.text,
        );
        group(
            format!("Orphaned deps of these ({})", plan.orphans.len()),
            plan.orphans.clone(),
            c.yellow,
        );
        group(
            format!("Home leftovers ({})", plan.home_paths.len()),
            plan.home_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            c.peach,
        );
        group(
            format!(
                "Config leftovers ({} candidates)",
                plan.system_leftovers.len()
            ),
            plan.system_leftovers
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            c.overlay0,
        );
        group(
            format!("Cached archives ({})", plan.cache_files.len()),
            plan.cache_files
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            c.overlay0,
        );
    }

    let content_lines = lines.len() as u16;
    let view_lines = rows[0].height;
    let max_scroll = content_lines.saturating_sub(view_lines);
    let scroll = scroll.min(max_scroll);

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        rows[0],
    );

    let footer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(rows[1]);

    let hint = if max_scroll > 0 {
        format!(
            " ↑↓ scroll · {}/{} lines",
            (scroll + view_lines).min(content_lines),
            content_lines
        )
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(Line::styled(hint, Style::default().fg(c.overlay0))),
        footer[0],
    );
    f.render_widget(
        Paragraph::new(vec![buttons(button, c.red, !refused)]),
        footer[1],
    );

    (content_lines, view_lines)
}

fn draw_installed(f: &mut Frame, names: &[String], files: &[String], scroll: u16) {
    let c = theme();
    let area = popup(f.area(), 82, 78);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(c.green))
        .style(Style::default().bg(c.base))
        .title(Span::styled(
            " Installed files ",
            Style::default().fg(c.green).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let mut lines = vec![
        Line::styled(
            format!("Installed: {}", names.join(", ")),
            Style::default().fg(c.green).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
    ];
    for l in files {
        let style = if l.starts_with("── ") {
            Style::default().fg(c.blue).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(c.overlay0)
        };
        lines.push(Line::styled(l.clone(), style));
    }

    let max_scroll = (lines.len() as u16).saturating_sub(rows[0].height);
    f.render_widget(
        Paragraph::new(lines).scroll((scroll.min(max_scroll), 0)),
        rows[0],
    );
    f.render_widget(
        Paragraph::new(Line::styled(
            " ↑↓ scroll · Esc close",
            Style::default().fg(c.overlay0),
        )),
        rows[1],
    );
}

fn draw_busy(f: &mut Frame, msg: &str) {
    let c = theme();
    let area = popup(f.area(), 46, 14);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Line::styled(msg.to_string(), Style::default().fg(c.text)))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(c.mauve))
                    .style(Style::default().bg(c.base)),
            ),
        area,
    );
}
