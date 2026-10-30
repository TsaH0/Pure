//! `editor.registers = "vim"` and `:normal`.

use helix_term::application::Application;
use helix_view::editor::RegisterStyle;

use super::*;

fn app(text: &str, style: RegisterStyle) -> Application {
    let mut config = helpers::test_config();
    config.editor.registers = style;
    helpers::AppBuilder::new()
        .with_config(config)
        .with_input_text(text)
        .build()
        .unwrap()
}

fn register(app: &Application, name: char) -> Option<Vec<String>> {
    app.editor
        .registers
        .read(name, &app.editor)
        .map(|values| values.map(|value| value.into_owned()).collect())
}

fn text(app: &Application) -> String {
    let (_, doc) = helix_view::current_ref!(app.editor);
    doc.text().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn vim_registers_keep_the_last_yank_and_deletes() -> anyhow::Result<()> {
    let mut app = app("#[o|]#ne\ntwo\nthree\nfour\n", RegisterStyle::Vim);
    // A yank goes to "0 as well.
    helpers::send_keys(&mut app, "xy;").await;
    assert_eq!(register(&app, '0').unwrap(), ["one\n"]);
    // Whole-line deletes go to "1, moving the older ones along.
    helpers::send_keys(&mut app, "xd").await;
    helpers::send_keys(&mut app, "xd").await;
    assert_eq!(register(&app, '1').unwrap(), ["two\n"]);
    assert_eq!(register(&app, '2').unwrap(), ["one\n"]);
    // "" has the last delete; "0 still the yank.
    assert_eq!(register(&app, '"').unwrap(), ["two\n"]);
    assert_eq!(register(&app, '0').unwrap(), ["one\n"]);
    // A small delete goes to "-, not "1.
    helpers::send_keys(&mut app, "ed").await;
    assert_eq!(register(&app, '-').unwrap(), ["three"]);
    assert_eq!(register(&app, '1').unwrap(), ["two\n"]);
    // Naming a register leaves the numbered ones alone; "_ discards.
    // (The selection is now the empty first line: `d` deletes it.)
    helpers::send_keys(&mut app, "dx\"_d").await;
    assert_eq!(register(&app, '1').unwrap(), ["\n"]);
    assert_eq!(register(&app, '2').unwrap(), ["two\n"]);
    helpers::send_keys(&mut app, "ux\"ad").await;
    assert_eq!(register(&app, 'a').unwrap(), ["four\n"]);
    assert_eq!(register(&app, '1').unwrap(), ["\n"]);
    // "0 pastes the yank even after deletes.
    helpers::send_keys(&mut app, "\"0P").await;
    assert!(text(&app).starts_with("one\n"), "{:?}", text(&app));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn helix_registers_are_unchanged_by_default() -> anyhow::Result<()> {
    let mut app = app("#[o|]#ne\ntwo\n", RegisterStyle::Helix);
    helpers::send_keys(&mut app, "xy;xd").await;
    assert_eq!(register(&app, '0'), None);
    assert_eq!(register(&app, '1'), None);
    assert_eq!(register(&app, '"').unwrap(), ["one\n"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn normal_plays_keys_and_abandons_unfinished_commands() -> anyhow::Result<()> {
    let mut app = app("#[o|]#ne two\n", RegisterStyle::Helix);
    helpers::send_keys(&mut app, ":normal ed<ret>").await;
    assert_eq!(text(&app), " two\n");
    // Insert mode is left at the end.
    helpers::send_keys(&mut app, ":norm ggiX<ret>").await;
    assert_eq!(text(&app), "X two\n");
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    Ok(())
}
