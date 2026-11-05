use super::*;

use helix_term::application::Application;
use helix_view::editor::ChordCommands;
use termina::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, Modifiers};

/// Send key events: `('j', Press)`, ...
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

fn line(app: &Application) -> usize {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    text.char_to_line(doc.selection(view.id).primary().cursor(text))
}

#[tokio::test(flavor = "multi_thread")]
async fn keys_held_together_run_chords() -> anyhow::Result<()> {
    use KeyEventKind::{Press, Release, Repeat};
    let mut config = helpers::test_config();
    // Hold j, press k: to the last line.
    config.editor.chords.insert(
        helix_view::document::Mode::Normal,
        [(
            "j+k".to_string(),
            ChordCommands::One("goto_last_line".into()),
        )]
        .into(),
    );
    let mut app = helpers::AppBuilder::new()
        .with_config(config)
        .with_input_text("#[a|]#\nb\nc\nd\ne\n")
        .build()?;

    send(
        &mut app,
        &[('j', Press), ('k', Press), ('k', Release), ('j', Release)],
    )
    .await;
    assert!(line(&app) >= 4, "line {}", line(&app));

    // j alone: down one when let go, once however long it is held.
    send(
        &mut app,
        &[('g', Press), ('g', Release), ('g', Press), ('g', Release)],
    )
    .await;
    assert_eq!(line(&app), 0);
    send(&mut app, &[('j', Press), ('j', Release)]).await;
    assert_eq!(line(&app), 1);
    send(
        &mut app,
        &[('j', Press), ('j', Repeat), ('j', Repeat), ('j', Release)],
    )
    .await;
    assert_eq!(line(&app), 2);

    // j then a key that makes no chord: both as usual.
    send(
        &mut app,
        &[('j', Press), ('x', Press), ('x', Release), ('j', Release)],
    )
    .await;
    assert_eq!(line(&app), 3);
    let (view, doc) = helix_view::current_ref!(app.editor);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .fragment(doc.text().slice(..)),
        "d\n"
    );
    // Other keys repeat as before.
    send(&mut app, &[('k', Press), ('k', Repeat), ('k', Release)]).await;
    assert_eq!(line(&app), 1);
    Ok(())
}
