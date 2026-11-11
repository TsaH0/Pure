use super::*;

use helix_term::application::Application;

fn row(app: &Application, y: u16) -> String {
    let screen = app.terminal_buffer();
    (screen.area.x..screen.area.right())
        .map(|x| screen[(x, y)].symbol.to_string())
        .collect()
}

/// Which screen rows hold a statusline (with the buffer's name).
fn statusline_rows(app: &Application) -> Vec<u16> {
    let screen = app.terminal_buffer();
    (screen.area.top()..screen.area.bottom())
        .filter(|&y| row(app, y).contains("[scratch]"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn statusline_moves_between_bottom_above_cmdline_and_top() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[o|]#ne\ntwo\n")
        .build()?;
    let last = app.terminal_buffer().area.bottom() - 1;
    helpers::send_keys(&mut app, "<esc>").await;
    // By default, the last row, the command line above it.
    assert_eq!(statusline_rows(&app), [last]);
    helpers::send_keys(&mut app, ":echo hello<ret>").await;
    assert!(
        row(&app, last - 1).starts_with("hello"),
        "{:?}",
        row(&app, last - 1)
    );
    let (view, doc) = helix_view::current_ref!(app.editor);
    let lines = view.inner_area(doc);
    assert_eq!(
        lines.bottom(),
        last - 1,
        "the text stops above the command line"
    );

    // Helix's: the statusline above the command line.
    helpers::send_keys(&mut app, ":set statusline.position above-cmdline<ret>").await;
    assert_eq!(statusline_rows(&app), [last - 1]);
    helpers::send_keys(&mut app, ":echo hi<ret>").await;
    assert!(row(&app, last).starts_with("hi"), "{:?}", row(&app, last));

    // At the top of the view: the text starts below it.
    helpers::send_keys(&mut app, ":set statusline.position top<ret>").await;
    assert_eq!(statusline_rows(&app), [0]);
    assert!(row(&app, 1).contains("one"), "{:?}", row(&app, 1));
    let (view, doc) = helix_view::current_ref!(app.editor);
    assert_eq!(view.inner_area(doc).y, 1);

    // Back to the bottom, with a horizontal split: the lower view's
    // statusline goes to the last row, the upper one's stays above it.
    helpers::send_keys(&mut app, ":set statusline.position bottom<ret>:hsplit<ret>").await;
    let rows = statusline_rows(&app);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1], last);
    assert!(rows[0] < last - 1);
    Ok(())
}
