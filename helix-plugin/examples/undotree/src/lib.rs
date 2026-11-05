//! Example plugin (API 0.5 `history`): the undo tree in a picker, like
//! undotree. Every branch is there, not only the ones `u` and `U` reach;
//! pick a revision to make the text that revision's.

use helix_plugin_api::{
    editor, history, register_plugin, CommandInfo, Item, Picker, Plugin, Registration,
};

/// The picker's lines: the tree drawn depth first, newest branch first,
/// with `(revision, line)`.
fn lines(tree: &history::Tree) -> Vec<(u32, String)> {
    let revisions = &tree.revisions;
    let mut children = vec![Vec::new(); revisions.len()];
    for (n, revision) in revisions.iter().enumerate().skip(1) {
        children[revision.parent as usize].push(n);
    }
    let newest = revisions.iter().map(|revision| revision.time).max().unwrap_or(0);
    let mut out = Vec::new();
    // (revision, indent) still to draw.
    let mut stack = vec![(0usize, 0usize)];
    while let Some((n, depth)) = stack.pop() {
        let revision = &revisions[n];
        let marker = if n as u32 == tree.current { "●" } else { "○" };
        let ago = (newest.saturating_sub(revision.time)) / 1000;
        let what = if n == 0 {
            "original".to_string()
        } else {
            format!("+{} -{}", revision.inserted, revision.deleted)
        };
        out.push((
            n as u32,
            format!("{}{marker} {n:>4}  {what:<12} {}", "│ ".repeat(depth), age(ago)),
        ));
        // The first child continues this line; later ones branch off.
        let kids = &children[n];
        for (i, child) in kids.iter().enumerate() {
            stack.push((*child, depth + usize::from(i + 1 < kids.len())));
        }
    }
    out
}

fn age(seconds: u64) -> String {
    match seconds {
        0 => "newest".into(),
        s if s < 60 => format!("{s}s before"),
        s if s < 3600 => format!("{}m before", s / 60),
        s => format!("{}h before", s / 3600),
    }
}

struct UndoTree;

impl Plugin for UndoTree {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: vec![CommandInfo {
                name: "show".into(),
                doc: "Browse the undo tree and jump to a revision".into(),
            }],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        if name != "show" {
            return Err(format!("unknown command {name}"));
        }
        let doc = editor::current_document().ok_or("no document")?;
        let tree = history::revisions(doc)?;
        let items = lines(&tree)
            .into_iter()
            .map(|(revision, line)| Item::new(vec![line], revision.to_string()))
            .collect();
        editor::pick(Picker::new("Undo tree", items), move |choice| {
            let Some(revision) = choice.items.first().and_then(|item| item.data.parse().ok())
            else {
                return Ok(());
            };
            history::jump(doc, revision)?;
            editor::set_status(&format!("Revision {revision}"));
            Ok(())
        })?;
        Ok(())
    }
}

register_plugin!(UndoTree);

#[cfg(test)]
mod test {
    use super::*;

    fn revision(parent: u32, time: u64) -> history::Revision {
        history::Revision {
            parent,
            time,
            inserted: 1,
            deleted: 0,
        }
    }

    #[test]
    fn draws_branches() {
        // 0 -> 1 -> 2, and 1 -> 3 (a branch after an undo).
        let tree = history::Tree {
            revisions: vec![revision(0, 0), revision(0, 1000), revision(1, 2000), revision(1, 3000)],
            current: 3,
        };
        let lines = lines(&tree);
        let order: Vec<u32> = lines.iter().map(|(n, _)| *n).collect();
        assert_eq!(order, [0, 1, 3, 2]);
        assert!(lines[2].1.starts_with("● "), "{:?}", lines[2]);
        assert!(lines[3].1.starts_with("│ ○"), "{:?}", lines[3]);
    }
}
