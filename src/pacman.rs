//! pacman / yay queries and output parsing.
//!
//! Rule: this module CHANGES NOTHING, it only reads. Everything that changes
//! the system lives in ops.rs, and every path there goes through a
//! confirmation screen.

use std::collections::HashSet;
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct Pkg {
    pub name: String,
    pub version: String,
    pub description: String,
    pub url: String,
    pub repo: String,
    pub depends: Vec<String>,
    pub optdepends: Vec<String>,
    pub required_by: Vec<String>,
    /// Virtual names this package provides ("sh", "cron" and the like)
    pub provides: Vec<String>,
    pub size: String,
    pub install_reason: String,
    pub installed: bool,
    /// Vote count on AUR results, None for repo packages
    pub votes: Option<String>,
}

impl Pkg {
    pub fn is_aur(&self) -> bool {
        self.repo == "aur"
    }
}

/// "26.2.5-1" -> "26.2.5", "1:1.6.8-1" -> "1:1.6.8"
///
/// Arch's pkgver can't contain a '-', so the last one always introduces
/// pkgrel. A split package's translation/locale sub-packages are commonly
/// rebuilt with their own pkgrel independent of the parent — comparing on
/// full "version-rel" strings misses those; the pkgver alone still matches.
fn base_version(version: &str) -> &str {
    version.rsplit_once('-').map(|(v, _)| v).unwrap_or(version)
}

/// "pcre2", "pacman>6.1", "sh=1.0" -> "pcre2"
///
/// Dependencies arrive with version constraints; checking whether one is
/// installed needs the bare name.
pub fn dep_name(raw: &str) -> &str {
    let end = raw
        .find(|c| c == '>' || c == '<' || c == '=' || c == ':')
        .unwrap_or(raw.len());
    raw[..end].trim()
}

/// Parse `pacman -Qi` output — every installed package in one call, ~100ms.
///
/// Format: "Field           : value", with continuation lines indented. Field
/// names start at column 0, so checking the indent is enough to tell them
/// apart.
fn parse_qi(text: &str, aur_names: &HashSet<String>) -> Vec<Pkg> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }

        let mut pkg = Pkg::default();
        let mut field = String::new();
        let mut value = String::new();

        // Store a field once it ends
        let flush = |field: &str, value: &str, pkg: &mut Pkg| {
            let v = value.trim();
            match field {
                "Name" => pkg.name = v.to_string(),
                "Version" => pkg.version = v.to_string(),
                "Description" => pkg.description = v.to_string(),
                "URL" => pkg.url = v.to_string(),
                "Installed Size" => pkg.size = v.to_string(),
                "Install Reason" => pkg.install_reason = v.to_string(),
                "Depends On" => pkg.depends = split_list(v),
                "Optional Deps" => pkg.optdepends = split_list(v),
                "Required By" => pkg.required_by = split_list(v),
                "Provides" => pkg.provides = split_list(v),
                _ => {}
            }
        };

        for line in block.lines() {
            if line.starts_with(char::is_whitespace) && !field.is_empty() {
                // Continuation line — append with a space between
                value.push(' ');
                value.push_str(line.trim());
                continue;
            }
            if let Some((f, v)) = line.split_once(':') {
                flush(&field, &value, &mut pkg);
                field = f.trim().to_string();
                value = v.to_string();
            }
        }
        flush(&field, &value, &mut pkg);

        if pkg.name.is_empty() {
            continue;
        }
        pkg.installed = true;
        pkg.repo = if aur_names.contains(&pkg.name) {
            "aur".to_string()
        } else {
            "repo".to_string()
        };
        out.push(pkg);
    }
    out
}

/// "glibc  libgcc  pcre2" -> ["glibc","libgcc","pcre2"], "None" -> []
///
/// Optional Deps lines read "name: description", so the trailing colon goes.
fn split_list(value: &str) -> Vec<String> {
    let v = value.trim();
    if v.is_empty() || v == "None" {
        return Vec::new();
    }
    v.split_whitespace()
        .map(|s| s.trim_end_matches(':').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Every installed package. Called once at startup.
pub fn installed() -> Vec<Pkg> {
    let aur: HashSet<String> = run(&["pacman", "-Qmq"])
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut pkgs = parse_qi(&run(&["pacman", "-Qi"]), &aur);
    pkgs.sort_by(|a, b| a.name.cmp(&b.name));
    pkgs
}

/// `yay -Ss` — repo and AUR results together.
///
/// Format:
///   extra/ripgrep 15.2.0-1 (1.4 MiB 4.3 MiB) (Installed)
///       A search tool that ...
///   aur/fuzzy 0.0.2-2 (+0 0.00) [308d5h]
///       Dynamic live fuzzy finder ...
pub fn search(query: &str, installed_names: &HashSet<String>) -> Vec<Pkg> {
    let text = run(&["yay", "-Ss", query]);
    let mut out: Vec<Pkg> = Vec::new();

    for line in text.lines() {
        if line.starts_with(char::is_whitespace) {
            // Description line — belongs to the entry above
            if let Some(last) = out.last_mut() {
                if last.description.is_empty() {
                    last.description = line.trim().to_string();
                }
            }
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(qualified) = parts.next() else { continue };
        let Some((repo, name)) = qualified.split_once('/') else {
            continue;
        };

        let version = parts.next().unwrap_or("").to_string();
        let rest: Vec<&str> = parts.collect();

        // Votes on AUR results: "(+12 0.53)"
        let votes = rest
            .iter()
            .find(|t| t.starts_with("(+"))
            .map(|t| t.trim_start_matches("(+").to_string());

        out.push(Pkg {
            name: name.to_string(),
            version,
            repo: repo.to_string(),
            installed: installed_names.contains(name),
            votes,
            ..Default::default()
        });
    }

    // Split packages (Arch's term for several packages built from one
    // PKGBUILD — locale packs, sub-plugins) are named "<parent>-<suffix>"
    // and share the parent's pkgver. Searching "libreoffice" otherwise
    // returns libreoffice-fresh plus ~400 per-locale packages
    // (libreoffice-fresh-af, -am, -ar, ...), each one just noise once
    // libreoffice-fresh itself is installed.
    //
    // Matching on pkgver, not the full "version-rel" string, is what catches
    // libreoffice-still's own locale packs too — Arch rebuilds those with
    // their own pkgrel (still-af 25.8.7-1 vs still itself 25.8.7-5).
    // pkgver alone is what keeps this from also swallowing packages that
    // only *look* namespaced, like python-numpy under python: numpy's
    // upstream version practically never collides with python's, so it
    // stays visible.
    let hidden: HashSet<usize> = (0..out.len())
        .filter(|&i| {
            out.iter().enumerate().any(|(j, parent)| {
                j != i
                    && base_version(&out[i].version) == base_version(&parent.version)
                    && out[i].name.len() > parent.name.len()
                    && out[i].name.starts_with(parent.name.as_str())
                    && out[i].name.as_bytes()[parent.name.len()] == b'-'
            })
        })
        .collect();
    if !hidden.is_empty() {
        out = out
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !hidden.contains(i))
            .map(|(_, p)| p)
            .collect();
    }

    // yay prints every AUR result first, so the exact match sinks to the
    // bottom: searching "ripgrep" left ripgrep in 17th place. Sort by
    // relevance instead.
    let q = query.to_lowercase();
    out.sort_by_key(|p| {
        let n = p.name.to_lowercase();
        let rank = if n == q {
            0
        } else if n.starts_with(&q) {
            1
        } else if n.contains(&q) {
            2
        } else {
            3
        };
        // On a tie, repo before AUR, then shortest name, then alphabetical.
        //
        // NOTE: alphabetical alone was the bug — searching "libreoffice"
        // put "libreoffice-extension-texmaths" above "libreoffice-fresh"
        // for no better reason than 'e' < 'f', and buried the actual app
        // under ~400 per-locale packages (libreoffice-fresh-af, -am, -ar,
        // ...) that all tie in the same prefix-match rank. The shortest
        // name in a tier is almost always the base package — a locale or
        // extension variant is, definitionally, the base name plus more.
        (rank, p.is_aur(), p.name.len(), p.name.clone())
    });
    out
}

/// Details for a package that is not installed: `pacman -Si` for repos, the
/// RPC for the AUR. The search list only carries name and version; this is
/// what fills the right pane.
pub fn remote_details(pkg: &Pkg) -> Option<Pkg> {
    if pkg.is_aur() {
        return aur_info(&pkg.name);
    }
    let text = run(&["pacman", "-Si", &pkg.name]);
    let parsed = parse_qi(&text, &HashSet::new());
    let mut p = parsed.into_iter().next()?;
    p.installed = pkg.installed;
    p.repo = pkg.repo.clone();
    Some(p)
}

/// AUR RPC v5. yay will not hand over the details of a single package, so we
/// ask directly; dependencies come from here.
fn aur_info(name: &str) -> Option<Pkg> {
    let url = format!("https://aur.archlinux.org/rpc/v5/info?arg[]={name}");
    let body = run(&["curl", "-sf", "--max-time", "10", &url]);
    let obj = json_first_result(&body)?;

    Some(Pkg {
        name: name.to_string(),
        version: json_str(&obj, "Version").unwrap_or_default(),
        description: json_str(&obj, "Description").unwrap_or_default(),
        url: json_str(&obj, "URL").unwrap_or_default(),
        repo: "aur".to_string(),
        depends: json_array(&obj, "Depends"),
        optdepends: json_array(&obj, "OptDepends"),
        ..Default::default()
    })
}

/// Full paths of the files a package installed.
pub fn files(name: &str) -> Vec<String> {
    run(&["pacman", "-Ql", name])
        .lines()
        .filter_map(|l| l.split_once(' ').map(|(_, path)| path.to_string()))
        .collect()
}

/// Dependencies of one installed package.
pub fn deps_of(name: &str) -> Vec<String> {
    let text = run(&["pacman", "-Qi", name]);
    parse_qi(&text, &HashSet::new())
        .into_iter()
        .next()
        .map(|p| p.depends)
        .unwrap_or_default()
}

/// Dependencies nothing uses any more.
pub fn orphans() -> Vec<String> {
    run(&["pacman", "-Qtdq"])
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn run(args: &[&str]) -> String {
    run_status(args).1
}

/// (succeeded, stdout, stderr)
///
/// NOTE: on a dry run pacman writes the detail of a failure to **stdout**
/// (":: removing ada breaks dependency ..."); only the "error:" line goes to
/// stderr. Parsing stdout without checking the exit code means mistaking that
/// message for a package name — exactly what the mega delete preview did.
pub fn run_status(args: &[&str]) -> (bool, String, String) {
    match Command::new(args[0]).args(&args[1..]).output() {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(e) => (false, String::new(), e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tiny JSON reader.
//
// Only a handful of fields are needed from the AUR RPC, so we pull out that
// much here rather than taking on serde_json. Not a full parser, and does not
// need to be one.
// ---------------------------------------------------------------------------

fn json_first_result(body: &str) -> Option<String> {
    let start = body.find("\"results\":[")? + "\"results\":[".len();
    let rest = &body[start..];
    if rest.trim_start().starts_with(']') {
        return None;
    }
    // Take the first object by counting braces (a single record in the array)
    let mut depth = 0usize;
    let mut begin = None;
    for (i, c) in rest.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    begin = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(rest[begin?..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn json_str(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = obj.find(&pat)? + pat.len();
    let rest = &obj[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next() {
                Some('u') => {
                    // \u escapes — AUR dependencies encode > and < this way
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => break,
            },
            other => out.push(other),
        }
    }
    None
}

fn json_array(obj: &str, key: &str) -> Vec<String> {
    let pat = format!("\"{key}\":[");
    let Some(start) = obj.find(&pat) else {
        return Vec::new();
    };
    let rest = &obj[start + pat.len()..];
    let Some(end) = rest.find(']') else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut chars = rest[..end].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_str {
                    out.push(std::mem::take(&mut cur));
                }
                in_str = !in_str;
            }
            '\\' if in_str => match chars.next() {
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        cur.push(ch);
                    }
                }
                Some(other) => cur.push(other),
                None => break,
            },
            other if in_str => cur.push(other),
            _ => {}
        }
    }
    out
}
