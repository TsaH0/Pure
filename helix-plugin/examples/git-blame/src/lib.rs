//! Example plugin: git blame for the cursor's line, at the end of the line,
//! like git-blame.nvim (or gitsigns' current-line blame):
//!
//! ```text
//! let x = compute();      Ana Lima, 3 days ago • Cache the parser
//! ```
//!
//! The file is blamed once when it opens and again when it is saved; moving
//! the cursor only redraws. Lines changed since the last save show nothing.
//! `toggle` turns it off and on, `show` opens the line's commit in a popup.
//!
//! It runs `git` (plugins.toml: `[git-blame]` `allow-process = ["git"]`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use helix_plugin_api::{
    decorations::{self, annotation},
    editor, register_plugin, text, CommandInfo, DocumentId, EventKind, Plugin, Registration,
    SpawnOptions,
};

#[derive(Debug, Clone, PartialEq)]
struct Commit {
    sha: String,
    author: String,
    /// Unix seconds.
    time: u64,
    summary: String,
}

struct Blame {
    /// Commit index of each 0-based line.
    lines: Vec<Option<usize>>,
    commits: Vec<Commit>,
    /// The document changed since: lines may have moved.
    stale: bool,
}

thread_local! {
    static BLAMES: RefCell<HashMap<DocumentId, Blame>> = RefCell::new(HashMap::new());
    static ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    /// Documents git blame is running for.
    static RUNNING: RefCell<Vec<DocumentId>> = const { RefCell::new(Vec::new()) };
}

/// Parse `git blame --porcelain` output.
fn parse_porcelain(output: &str) -> (Vec<Option<usize>>, Vec<Commit>) {
    let mut commits: Vec<Commit> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut lines: Vec<Option<usize>> = Vec::new();
    let mut current: Option<(usize, usize)> = None; // (commit, final line)
    for line in output.lines() {
        if let Some(content) = line.strip_prefix('\t') {
            let _ = content;
            if let Some((commit, final_line)) = current.take() {
                if lines.len() <= final_line {
                    lines.resize(final_line + 1, None);
                }
                lines[final_line] = Some(commit);
            }
            continue;
        }
        let mut words = line.split(' ');
        let first = words.next().unwrap_or_default();
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            let final_line: usize = words.nth(1).and_then(|n| n.parse().ok()).unwrap_or(1);
            let commit = *index.entry(first.to_string()).or_insert_with(|| {
                commits.push(Commit {
                    sha: first.to_string(),
                    author: String::new(),
                    time: 0,
                    summary: String::new(),
                });
                commits.len() - 1
            });
            current = Some((commit, final_line - 1));
            continue;
        }
        let Some((commit, _)) = current else {
            continue;
        };
        let value = line.split_once(' ').map_or("", |(_, v)| v);
        match first {
            "author" => commits[commit].author = value.to_string(),
            "author-time" => commits[commit].time = value.parse().unwrap_or(0),
            "summary" => commits[commit].summary = value.to_string(),
            _ => {}
        }
    }
    (lines, commits)
}

/// "3 days ago".
fn ago(then: u64, now: u64) -> String {
    let seconds = now.saturating_sub(then);
    let (amount, unit) = match seconds {
        0..60 => return "just now".into(),
        60..3600 => (seconds / 60, "minute"),
        3600..86_400 => (seconds / 3600, "hour"),
        86_400..2_592_000 => (seconds / 86_400, "day"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    format!("{amount} {unit}{} ago", if amount == 1 { "" } else { "s" })
}

fn describe(commit: &Commit, now: u64) -> String {
    if commit.sha.bytes().all(|b| b == b'0') {
        return "You • Not committed yet".into();
    }
    format!("{}, {} • {}", commit.author, ago(commit.time, now), commit.summary)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Run git blame on `doc`'s file, then draw.
fn blame(doc: DocumentId) -> Result<(), String> {
    if !ENABLED.get() {
        return Ok(());
    }
    let Some(path) = editor::document_path(doc)? else {
        return Ok(());
    };
    if RUNNING.with_borrow(|running| running.contains(&doc)) {
        return Ok(());
    }
    let (dir, name) = path.rsplit_once('/').unwrap_or((".", &path));
    let options = SpawnOptions {
        cwd: Some(if dir.is_empty() { "/".into() } else { dir.into() }),
        ..Default::default()
    };
    RUNNING.with_borrow_mut(|running| running.push(doc));
    let started = editor::spawn(&["git", "blame", "--porcelain", "--", name], options, move |result| {
        RUNNING.with_borrow_mut(|running| running.retain(|d| *d != doc));
        // Not in a repository, not tracked...: nothing to show.
        let Ok(output) = result else {
            BLAMES.with_borrow_mut(|blames| {
                blames.insert(doc, Blame { lines: Vec::new(), commits: Vec::new(), stale: false });
            });
            return Ok(());
        };
        if output.exit_code != Some(0) {
            // Remembered as "nothing to blame", so it isn't asked again.
            BLAMES.with_borrow_mut(|blames| {
                blames.insert(doc, Blame { lines: Vec::new(), commits: Vec::new(), stale: false });
            });
            return decorations::set_annotations(doc, "blame", &[]);
        }
        let (lines, commits) = parse_porcelain(&output.stdout);
        BLAMES.with_borrow_mut(|blames| {
            blames.insert(doc, Blame { lines, commits, stale: false });
        });
        draw(doc)
    });
    if let Err(err) = started {
        // git not granted: don't try on every cursor move.
        RUNNING.with_borrow_mut(|running| running.retain(|d| *d != doc));
        BLAMES.with_borrow_mut(|blames| {
            blames.insert(doc, Blame { lines: Vec::new(), commits: Vec::new(), stale: false });
        });
        return Err(err);
    }
    Ok(())
}

/// The blame of the cursor's line at its end.
fn draw(doc: DocumentId) -> Result<(), String> {
    if !ENABLED.get() || editor::current_document() != Some(doc) {
        return Ok(());
    }
    let source = editor::text(doc)?;
    let head = editor::selections(doc)?.first().map_or(0, |r| r.head);
    let line = source[..text::char_to_byte(&source, head)].matches('\n').count();
    // Opened before the plugin loaded (or never blamed): blame it now.
    if !BLAMES.with_borrow(|blames| blames.contains_key(&doc)) {
        return blame(doc);
    }
    let label = BLAMES.with_borrow(|blames| {
        let blame = blames.get(&doc).filter(|b| !b.stale)?;
        let commit = &blame.commits[(*blame.lines.get(line)?)?];
        Some(describe(commit, now()))
    });
    let Some(label) = label else {
        return decorations::set_annotations(doc, "blame", &[]);
    };
    // At the line's end (before its line break).
    let starts = text::line_starts(&source);
    let end = starts
        .get(line + 1)
        .map_or(text::char_len(&source), |next| next.saturating_sub(1));
    decorations::set_annotations(doc, "blame", &[annotation(end, format!("      {label}"), "comment")])
}

struct GitBlame;

impl Plugin for GitBlame {
    fn init(_config: String) -> Result<Registration, String> {
        let command = |name: &str, doc: &str| CommandInfo { name: name.into(), doc: doc.into() };
        Ok(Registration {
            commands: vec![
                command("toggle", "Turn the line blame off or on"),
                command("show", "Show the cursor line's commit"),
            ],
            events: vec![
                EventKind::DocumentOpen,
                EventKind::DocumentSave,
                EventKind::DocumentChange,
                EventKind::DocumentClose,
                EventKind::SelectionChange,
            ],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        let doc = editor::current_document().ok_or("no document")?;
        match name {
            "toggle" => {
                ENABLED.set(!ENABLED.get());
                if ENABLED.get() {
                    blame(doc)
                } else {
                    for doc in editor::documents() {
                        decorations::set_annotations(doc, "blame", &[])?;
                    }
                    Ok(())
                }
            }
            "show" => {
                let source = editor::text(doc)?;
                let head = editor::selections(doc)?.first().map_or(0, |r| r.head);
                let line = source[..text::char_to_byte(&source, head)].matches('\n').count();
                let commit = BLAMES
                    .with_borrow(|blames| {
                        let blame = blames.get(&doc)?;
                        Some(blame.commits[(*blame.lines.get(line)?)?].clone())
                    })
                    .ok_or("no blame for this line (save the file first)")?;
                editor::popup(&format!(
                    "**{}**\n\n{} · {}\n\n`{}`",
                    commit.summary,
                    commit.author,
                    ago(commit.time, now()),
                    commit.sha
                ))?;
                Ok(())
            }
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_document_open(doc: DocumentId) {
        let _ = blame(doc);
    }
    fn on_document_save(doc: DocumentId) {
        let _ = blame(doc);
    }
    fn on_document_change(doc: DocumentId) {
        // Lines may have moved: nothing until the next save.
        BLAMES.with_borrow_mut(|blames| {
            if let Some(blame) = blames.get_mut(&doc) {
                blame.stale = true;
            }
        });
        let _ = decorations::set_annotations(doc, "blame", &[]);
    }
    fn on_document_close(doc: DocumentId) {
        BLAMES.with_borrow_mut(|blames| blames.remove(&doc));
    }
    fn on_selection_change(doc: DocumentId) {
        let _ = draw(doc);
    }
}

register_plugin!(GitBlame);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn porcelain_output_is_parsed() {
        let sha = "a".repeat(40);
        let other = "b".repeat(40);
        let output = format!(
            "{sha} 1 1 2\nauthor Ana\nauthor-time 1000\nsummary First\nfilename x\n\tline one\n\
             {sha} 2 2\n\tline two\n\
             {other} 1 3 1\nauthor Bo\nauthor-time 2000\nsummary Second\nfilename x\n\tline three\n"
        );
        let (lines, commits) = parse_porcelain(&output);
        assert_eq!(lines, [Some(0), Some(0), Some(1)]);
        assert_eq!(commits[0].author, "Ana");
        assert_eq!(commits[1].summary, "Second");
        assert_eq!(describe(&commits[1], 2000 + 3 * 86_400), "Bo, 3 days ago • Second");
    }

    #[test]
    fn ages() {
        assert_eq!(ago(0, 30), "just now");
        assert_eq!(ago(0, 3600), "1 hour ago");
        assert_eq!(ago(0, 2 * 31_536_000), "2 years ago");
    }
}
