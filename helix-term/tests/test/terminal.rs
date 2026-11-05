use super::*;

use std::time::Duration;

use helix_term::application::Application;
use helix_view::document::Mode;

fn screen(app: &Application) -> String {
    let screen = app.terminal_buffer();
    (screen.area.y..screen.area.bottom())
        .map(|y| {
            (screen.area.x..screen.area.right())
                .map(|x| screen[(x, y)].symbol.to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run the editor until the screen shows `text`.
async fn wait_for_screen(app: &mut Application, text: &str) {
    for _ in 0..200 {
        app.editor.reset_idle_timer();
        helpers::run_event_loop_until_idle(app).await;
        if screen(app).contains(text) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{text:?} never showed:\n{}", screen(app));
}

#[tokio::test(flavor = "multi_thread")]
async fn terminals_run_programs_and_take_keys() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[o|]#ne\n")
        .build()?;
    helpers::send_keys(
        &mut app,
        ":terminal sh -c \"printf 'hello from sh\\n'; exec cat\"<ret>",
    )
    .await;
    wait_for_screen(&mut app, "hello from sh").await;
    // A split below the document, in insert mode: keys go to the program.
    assert_eq!(app.editor.tree.views().count(), 2);
    assert_eq!(app.editor.mode(), Mode::Insert);
    helpers::send_keys(&mut app, "typed<ret>").await;
    // The terminal echoes it, then cat prints it again.
    wait_for_screen(&mut app, "typed\ntyped").await;
    // Esc goes to the program too; C-\ C-n leaves insert mode.
    helpers::send_keys(&mut app, "<esc>").await;
    assert_eq!(app.editor.mode(), Mode::Insert);
    helpers::send_keys(&mut app, "<C-\\><C-n>").await;
    assert_eq!(app.editor.mode(), Mode::Normal);
    // Normal-mode keys are the editor's: move to the document above.
    helpers::send_keys(&mut app, "<C-w>k").await;
    assert!(helix_view::doc!(app.editor).terminal.is_none());
    // Closing the terminal's buffer ends the program.
    helpers::send_keys(&mut app, "<C-w>j:buffer-close<ret>").await;
    assert_eq!(app.editor.documents.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminals_tell_when_the_program_ends() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[o|]#ne\n")
        .build()?;
    helpers::send_keys(&mut app, ":terminal sh -c \"exit 3\"<ret>").await;
    wait_for_screen(&mut app, "[process exited 3]").await;
    // Nothing in it to save: closing it asks nothing.
    helpers::send_keys(&mut app, "<C-\\><C-n>:buffer-close<ret>").await;
    assert_eq!(app.editor.documents.len(), 1);
    Ok(())
}
