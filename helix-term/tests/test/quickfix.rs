//! The quickfix list: `]q` `[q` `]Q` `[Q`, `:cnext` & co, `:cbuffer`, `:grep`.

use std::path::Path;

use helix_term::application::Application;
use helix_view::editor::Severity;

use super::*;

fn status(app: &Application) -> (String, Severity) {
    let (message, severity) = app.editor.get_status().expect("a status message");
    (message.to_string(), *severity)
}

fn location(app: &Application) -> (String, usize, usize) {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    let pos = doc.selection(view.id).primary().cursor(text);
    let line = text.char_to_line(pos);
    let name = doc
        .path()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (name, line + 1, pos - text.line_to_char(line) + 1)
}

/// A directory with two files and an editor showing compiler-like output
/// pointing into them.
fn setup() -> (Application, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "one\ntwo\nthree\nfour\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "alpha\nbeta\n").unwrap();
    let a = dir.path().join("a.rs");
    let b = dir.path().join("b.rs");
    let output = format!(
        "#[c|]#ompiling\n{a}:2:3: error: first\nnoise 12:30:45\n{a}:4:1: warning: second\nerror: third\n --> {b}:2:2\n",
        a = a.display(),
        b = b.display()
    );
    let app = AppBuilder::new().with_input_text(output).build().unwrap();
    (app, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn navigating_the_list() -> anyhow::Result<()> {
    let (mut app, _dir) = setup();
    send_keys(&mut app, ":cbuffer<ret>").await;
    // Filling the list goes to the first entry.
    assert_eq!(location(&app), ("a.rs".into(), 2, 3));
    assert_eq!(status(&app).0, "(1 of 3) error: first");

    send_keys(&mut app, "]q").await;
    assert_eq!(location(&app), ("a.rs".into(), 4, 1));
    assert_eq!(status(&app).0, "(2 of 3) warning: second");

    // rustc's `-->` lines take the message from the line before.
    send_keys(&mut app, "]q").await;
    assert_eq!(location(&app), ("b.rs".into(), 2, 2));
    assert_eq!(status(&app).0, "(3 of 3) error: third");

    send_keys(&mut app, "]q").await;
    assert_eq!(
        status(&app),
        (
            "No more entries in the quickfix list".into(),
            Severity::Error
        )
    );
    assert_eq!(location(&app), ("b.rs".into(), 2, 2));

    send_keys(&mut app, "[q").await;
    assert_eq!(location(&app), ("a.rs".into(), 4, 1));
    send_keys(&mut app, "[Q").await;
    assert_eq!(location(&app), ("a.rs".into(), 2, 3));
    send_keys(&mut app, "[q").await;
    assert_eq!(status(&app).0, "No previous entry in the quickfix list");
    send_keys(&mut app, "]Q").await;
    assert_eq!(location(&app), ("b.rs".into(), 2, 2));
    // Counts.
    send_keys(&mut app, "2[q").await;
    assert_eq!(location(&app), ("a.rs".into(), 2, 3));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn typable_commands() -> anyhow::Result<()> {
    let (mut app, _dir) = setup();
    send_keys(&mut app, ":cbuffer<ret>:cnext<ret>").await;
    assert_eq!(status(&app).0, "(2 of 3) warning: second");
    send_keys(&mut app, ":cn<ret>").await;
    assert_eq!(status(&app).0, "(3 of 3) error: third");
    send_keys(&mut app, ":cprev 2<ret>").await;
    assert_eq!(status(&app).0, "(1 of 3) error: first");
    send_keys(&mut app, ":clast<ret>").await;
    assert_eq!(status(&app).0, "(3 of 3) error: third");
    send_keys(&mut app, ":cfirst<ret>").await;
    assert_eq!(status(&app).0, "(1 of 3) error: first");
    send_keys(&mut app, ":cc 2<ret>").await;
    assert_eq!(status(&app).0, "(2 of 3) warning: second");
    send_keys(&mut app, ":cc 9<ret>").await;
    assert_eq!(status(&app).0, "No entry 9 (the list has 3)");
    // `:cc` alone: the current entry again.
    send_keys(&mut app, "gg:cc<ret>").await;
    assert_eq!(location(&app), ("a.rs".into(), 4, 1));
    send_keys(&mut app, ":cclear<ret>]q").await;
    assert_eq!(status(&app).0, "The quickfix list is empty");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn jumps_can_be_undone_with_the_jumplist() -> anyhow::Result<()> {
    let (mut app, _dir) = setup();
    send_keys(&mut app, ":cbuffer<ret>]q").await;
    assert_eq!(location(&app), ("a.rs".into(), 4, 1));
    send_keys(&mut app, "<C-o>").await;
    assert_eq!(location(&app), ("a.rs".into(), 2, 3));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_lists() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[n|]#othing here\n")
        .build()?;
    send_keys(&mut app, "]q").await;
    assert_eq!(
        status(&app),
        ("The quickfix list is empty".into(), Severity::Error)
    );
    send_keys(&mut app, ":copen<ret>").await;
    assert_eq!(status(&app).0, "'copen': The quickfix list is empty");
    send_keys(&mut app, ":cbuffer<ret>").await;
    assert_eq!(status(&app).0, "buffer: no locations found");
    send_keys(&mut app, ":cdiagnostics<ret>").await;
    assert_eq!(status(&app).0, "diagnostics: no locations found");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn grep_fills_the_list() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("x.txt"), "nothing\nneedle one\n")?;
    std::fs::write(dir.path().join("y.txt"), "needle two\n")?;
    let mut app = AppBuilder::new().with_input_text("#[a|]#\n").build()?;
    send_keys(
        &mut app,
        &format!(":grep needle {}<ret>", dir.path().display()),
    )
    .await;
    // The search runs in the background.
    for _ in 0..50 {
        if status(&app).0.starts_with('(') {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        app.editor.reset_idle_timer();
        run_event_loop_until_idle(&mut app).await;
    }
    let (message, _) = status(&app);
    assert!(message.starts_with("(1 of 2) "), "{message}");
    assert!(message.contains("needle"), "{message}");
    let (file, line, _) = location(&app);
    assert!(
        (file == "x.txt" && line == 2) || (file == "y.txt" && line == 1),
        "{file}:{line}"
    );
    assert!(Path::new(&dir.path().join(&file)).exists());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn grep_columns_count_bytes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    // `é` and `✓` are 2 and 3 bytes: ripgrep reports column 7 for `needle`.
    std::fs::write(dir.path().join("u.txt"), "é ✓ needle\n")?;
    let mut app = AppBuilder::new().with_input_text("#[a|]#\n").build()?;
    send_keys(
        &mut app,
        &format!(":grep needle {}<ret>", dir.path().display()),
    )
    .await;
    for _ in 0..50 {
        if status(&app).0.starts_with('(') {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        app.editor.reset_idle_timer();
        run_event_loop_until_idle(&mut app).await;
    }
    // The cursor is on the `n` (char 4), not 2 chars further.
    assert_eq!(location(&app), ("u.txt".to_string(), 1, 5));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quickfix_picker_opens() -> anyhow::Result<()> {
    let (mut app, _dir) = setup();
    send_keys(&mut app, ":cbuffer<ret><space>q").await;
    let (message, severity) = status(&app);
    assert_eq!(severity, Severity::Info);
    assert_eq!(message, "Quickfix: buffer (3 entries)");
    // Choosing the second entry jumps there.
    send_keys(&mut app, "<down><ret>").await;
    assert_eq!(location(&app), ("a.rs".into(), 4, 1));
    Ok(())
}
