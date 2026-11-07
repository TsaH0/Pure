use super::*;

use std::collections::BTreeMap;

use helix_term::application::Application;
use helix_term::keymap::modes::{read_mode_files, ModeConfig};
use helix_view::document::Mode;

/// An editor with the shipped modes (vim, emacs, ki...) and `extra`
/// (`[modes.<name>]` tables).
fn app(text: &str, extra: &str, file: Option<&std::path::Path>) -> anyhow::Result<Application> {
    #[derive(serde::Deserialize)]
    struct Extra {
        modes: BTreeMap<String, ModeConfig>,
    }
    let runtime = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/modes");
    let (mut modes, errors) = read_mode_files(&runtime);
    assert!(errors.is_empty(), "{errors:?}");
    if !extra.is_empty() {
        modes.extend(toml::from_str::<Extra>(extra)?.modes);
    }
    let mut config = helpers::test_config();
    config.modes = modes;
    let mut builder = helpers::AppBuilder::new().with_config(config);
    builder = match file {
        Some(file) => builder.with_file(file, None),
        None => builder.with_input_text(text),
    };
    builder.build()
}

async fn keys(app: &mut Application, keys: &str) {
    helpers::send_keys(app, keys).await
}

fn text(app: &Application) -> String {
    helix_view::doc!(app.editor).text().to_string()
}

fn selection(app: &Application) -> String {
    let (view, doc) = helix_view::current_ref!(app.editor);
    doc.selection(view.id)
        .primary()
        .fragment(doc.text().slice(..))
        .to_string()
}

fn statusline(app: &Application) -> String {
    let screen = app.terminal_buffer();
    let y = screen.area.bottom() - 2;
    (screen.area.x..screen.area.right())
        .map(|x| screen[(x, y)].symbol.to_string())
        .collect()
}

fn status(app: &Application) -> String {
    app.editor
        .get_status()
        .map(|(message, _)| message.to_string())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn key_modes_switch_keys_per_buffer_and_show_in_the_statusline() -> anyhow::Result<()> {
    let extra = r#"
[modes.mine]
description = "My keys"
lighter = "Mine"
[modes.mine.keys.normal]
x = "goto_line_end"
g = { q = "goto_file_start" }

[modes.rusty]
languages = ["rust"]
[modes.rusty.keys.normal]
X = "goto_line_end"
"#;
    let mut app = app("#[o|]#ne two\nthree\n", extra, None)?;
    assert!(!statusline(&app).contains("[Mine]"), "{}", statusline(&app));
    keys(&mut app, ":mode mine<ret>").await;
    assert_eq!(status(&app), "Mode mine on");
    assert!(statusline(&app).contains("[Mine]"), "{}", statusline(&app));
    // x is the mode's; g q is added to Pure's g menu, and g g stays.
    keys(&mut app, "x").await;
    assert_eq!(selection(&app), "o");
    let (view, doc) = helix_view::current_ref!(app.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(cursor, 6);
    keys(&mut app, "gq").await;
    assert_eq!(selection(&app), "o");

    // Per buffer: a new one doesn't have it.
    keys(&mut app, ":new<ret>").await;
    assert!(!statusline(&app).contains("[Mine]"));
    keys(&mut app, ":buffer-close!<ret>").await;
    assert!(statusline(&app).contains("[Mine]"));

    // A language turns its mode on; it can be turned off in one buffer.
    keys(&mut app, ":set-language rust<ret>").await;
    assert!(
        statusline(&app).contains("[rusty Mine]"),
        "{}",
        statusline(&app)
    );
    keys(&mut app, ":mode rusty off<ret>").await;
    assert!(statusline(&app).contains("[Mine]") && !statusline(&app).contains("rusty"));

    keys(&mut app, ":mode mine off<ret>").await;
    assert!(!statusline(&app).contains("Mine"));
    keys(&mut app, "x").await;
    assert_eq!(selection(&app), "one two\n", "Pure's x again");

    keys(&mut app, ":mode nope<ret>").await;
    assert!(status(&app).contains("no mode 'nope'"), "{}", status(&app));
    keys(&mut app, ":describe-mode vim<ret>").await;
    keys(&mut app, "<esc>").await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn vim_mode_brings_vim_keys() -> anyhow::Result<()> {
    let mut app = app("#[o|]#ne\ntwo\nthree\n", "", None)?;
    keys(&mut app, ":mode vim<ret>").await;
    assert!(statusline(&app).contains("[Vim]"));
    keys(&mut app, "dd").await;
    assert_eq!(text(&app), "two\nthree\n");
    keys(&mut app, "x").await;
    assert_eq!(text(&app), "wo\nthree\n");
    keys(&mut app, "u").await;
    assert_eq!(text(&app), "two\nthree\n");
    keys(&mut app, "<C-r>").await;
    assert_eq!(text(&app), "wo\nthree\n");
    keys(&mut app, "jyyp").await;
    assert_eq!(text(&app), "wo\nthree\nthree\n");
    keys(&mut app, "G$").await;
    assert_eq!(app.editor.mode(), Mode::Normal);
    // Off: d is Pure's again.
    keys(&mut app, ":mode vim off<ret>ggd").await;
    assert_eq!(text(&app), "o\nthree\nthree\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn emacs_mode_edits_in_insert_mode_and_vim_turns_it_off() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\n")?;
    let mut app = app("", "", Some(file.path()))?;
    keys(&mut app, ":mode emacs<ret>").await;
    // It goes to insert mode and stays there.
    assert_eq!(app.editor.mode(), Mode::Insert);
    keys(&mut app, "<C-e>!<C-a>[<C-n><C-a><C-k>").await;
    assert_eq!(text(&app), "[one!\n\n");
    keys(&mut app, "<C-p><C-e><C-b><C-d>").await;
    assert_eq!(
        text(&app),
        "[one\n\n",
        "C-b back over the !, C-d deletes it"
    );
    keys(&mut app, "<C-x><C-s>").await;
    helpers::run_event_loop_until_idle(&mut app).await;
    assert_eq!(std::fs::read_to_string(file.path())?, "[one\n\n");
    // Vim conflicts with it: Emacs goes off (back to normal mode).
    keys(&mut app, "<esc>:mode vim<ret>").await;
    assert!(statusline(&app).contains("[Vim]") && !statusline(&app).contains("Emacs"));
    assert_eq!(app.editor.mode(), Mode::Normal);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ki_mode_moves_in_its_selection_modes() -> anyhow::Result<()> {
    let mut app = app("#[f|]#oo barBaz qux\nline two\n", "", None)?;
    keys(&mut app, ":mode ki<ret>").await;
    // Ki starts in WORD.
    assert!(
        statusline(&app).contains("[Ki WORD]"),
        "{}",
        statusline(&app)
    );
    // From a cursor the first step takes the word it is in, then the next.
    keys(&mut app, "ll").await;
    assert_eq!(selection(&app), "barBaz ");
    keys(&mut app, "w").await;
    assert!(
        statusline(&app).contains("[Ki SUBWORD]"),
        "{}",
        statusline(&app)
    );
    keys(&mut app, "j").await;
    assert_eq!(selection(&app), "Baz ", "back by subword (camelCase part)");
    keys(&mut app, "a").await;
    assert!(statusline(&app).contains("[Ki LINE]"));
    keys(&mut app, "l").await;
    assert_eq!(selection(&app), "line two\n");
    // Actions: r deletes the selection.
    keys(&mut app, "r").await;
    assert_eq!(text(&app), "foo barBaz qux\n");
    // Off: its sub-modes go with it.
    keys(&mut app, ":mode ki off<ret>").await;
    assert!(!statusline(&app).contains("[Ki"), "{}", statusline(&app));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn vim_operators_take_motions_text_objects_and_typed_characters() -> anyhow::Result<()> {
    let mut app = app("#[c|]#all(one, two) end\n", "", None)?;
    keys(&mut app, ":mode vim<ret>").await;
    // Motions move without selecting.
    keys(&mut app, "w").await;
    assert_eq!(selection(&app), "(");
    // Operator + text object: the object's character is typed after.
    keys(&mut app, "ci(X<esc>").await;
    assert_eq!(text(&app), "call(X) end\n");
    // Operator + find: dt<char>.
    keys(&mut app, "0dt(").await;
    assert_eq!(text(&app), "(X) end\n");
    // A fixed character in a sequence: diw is ["select_textobject_inner", ...].
    keys(&mut app, "$diw").await;
    assert_eq!(text(&app), "(X) \n");
    keys(&mut app, "u0dw").await;
    assert_eq!(text(&app), "X) end\n");
    // e, b and ge land where Vim's do; cw changes to the word's end.
    keys(&mut app, "e").await;
    assert_eq!(selection(&app), ")");
    keys(&mut app, "e").await;
    assert_eq!(selection(&app), "d");
    keys(&mut app, "ge").await;
    assert_eq!(selection(&app), ")");
    keys(&mut app, "bcwY<esc>").await;
    assert_eq!(text(&app), "Y) end\n");
    // * searches the word under the cursor.
    let mut app2 = app_with("#[f|]#oo bar foo\n")?;
    keys(&mut app2, ":mode vim on<ret>*").await;
    let (view, doc) = helix_view::current_ref!(app2.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(cursor, 8, "on the next foo");
    Ok(())
}

fn app_with(text: &str) -> anyhow::Result<Application> {
    app(text, "", None)
}

#[tokio::test(flavor = "multi_thread")]
async fn emacs_mark_extends_the_region_until_killed() -> anyhow::Result<()> {
    let mut app = app("#[o|]#ne two\n", "", None)?;
    keys(&mut app, ":mode emacs<ret><C-space>").await;
    assert!(
        statusline(&app).contains("[Emacs Mark]"),
        "{}",
        statusline(&app)
    );
    // The block cursor is in the region: three steps take "one ".
    keys(&mut app, "<C-f><C-f><C-f><C-w>").await;
    assert_eq!(text(&app), "two\n");
    assert!(!statusline(&app).contains("Mark"), "{}", statusline(&app));
    keys(&mut app, "<C-y>").await;
    assert_eq!(text(&app), "one two\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ki_held_layers_act_along_a_movement() -> anyhow::Result<()> {
    use termina::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, Modifiers};
    async fn send(app: &mut Application, events: &[(char, KeyEventKind)]) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        for &(c, kind) in events {
            let key = KeyEvent {
                code: KeyCode::Char(c),
                kind,
                modifiers: Modifiers::NONE,
                state: KeyEventState::NONE,
            };
            tx.send(Ok(termina::Event::Key(key))).unwrap();
        }
        assert!(app.event_loop_until_idle(&mut stream).await);
    }
    use KeyEventKind::{Press, Release};
    let mut app = app("#[f|]#oo bar baz\n", "", None)?;
    keys(&mut app, ":mode ki<ret>").await;
    // The terminal reports releases.
    send(&mut app, &[('s', Release)]).await;
    // Hold r, press l: delete along "next word" (WORD mode).
    send(
        &mut app,
        &[('r', Press), ('l', Press), ('l', Release), ('r', Release)],
    )
    .await;
    assert_eq!(text(&app), "bar baz\n");
    // Tapped alone, r deletes the selection.
    send(
        &mut app,
        &[('l', Press), ('l', Release), ('r', Press), ('r', Release)],
    )
    .await;
    assert_eq!(text(&app), "baz\n");
    Ok(())
}
