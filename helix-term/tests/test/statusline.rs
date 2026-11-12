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

#[tokio::test(flavor = "multi_thread")]
async fn statusline_position_as_a_number_of_rows_above_the_bottom() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[o|]#ne\ntwo\n")
        .build()?;
    let last = app.terminal_buffer().area.bottom() - 1;
    // 0 and 1 are the named positions.
    helpers::send_keys(&mut app, ":set statusline.position 1<ret>").await;
    assert_eq!(statusline_rows(&app), [last - 1]);
    helpers::send_keys(&mut app, ":set statusline.position 0<ret>").await;
    assert_eq!(statusline_rows(&app), [last]);
    // 3: the statusline three rows up, the command line under it, two
    // empty rows at the bottom.
    helpers::send_keys(&mut app, ":set statusline.position 3<ret>:echo hey<ret>").await;
    assert_eq!(statusline_rows(&app), [last - 3]);
    assert!(
        row(&app, last - 2).starts_with("hey"),
        "{:?}",
        row(&app, last - 2)
    );
    assert_eq!(row(&app, last).trim(), "");
    assert_eq!(row(&app, last - 1).trim(), "");
    // The command line is typed there too.
    helpers::send_keys(&mut app, ":ech").await;
    assert!(
        row(&app, last - 2).starts_with(":ech"),
        "{:?}",
        row(&app, last - 2)
    );
    Ok(())
}

fn cmdline_app(cmdline: helix_view::editor::CmdlineConfig) -> anyhow::Result<Application> {
    let mut config = helpers::test_config();
    config.editor.cmdline = cmdline;
    helpers::AppBuilder::new()
        .with_config(config)
        .with_input_text("#[o|]#ne\ntwo\n")
        .build()
}

#[tokio::test(flavor = "multi_thread")]
async fn pickers_keep_their_search_line_with_a_popup_command_line() -> anyhow::Result<()> {
    use helix_view::editor::{CmdlineConfig, CmdlineStyle};
    for style in [CmdlineStyle::Popup, CmdlineStyle::Panel] {
        let mut app = cmdline_app(CmdlineConfig {
            style,
            ..Default::default()
        })?;
        helpers::send_keys(&mut app, "<space>bscr").await;
        let screen = app.terminal_buffer();
        let rows: Vec<_> = (screen.area.top()..screen.area.bottom())
            .map(|y| row(&app, y))
            .collect();
        // The typed text is in the picker's own line, not in a command
        // line box.
        assert!(
            rows.iter().any(|r| r.contains("│ scr")),
            "{style:?}: {rows:#?}"
        );
        assert!(!rows.iter().any(|r| r.contains("> scr")), "{rows:#?}");
        assert!(
            !rows.iter().any(|r| r.contains("Tab complete")),
            "{rows:#?}"
        );
        // And it filters.
        assert!(rows.iter().any(|r| r.contains("[scratch]")), "{rows:#?}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn command_line_icons_borders_titles_and_placement() -> anyhow::Result<()> {
    use helix_view::editor::{CmdlineBorder, CmdlineConfig, CmdlineStyle};
    // The bottom command line with an icon for `:`.
    let mut app = cmdline_app(CmdlineConfig {
        icons: [(":".to_string(), "λ ".to_string())].into(),
        bottom_rows: 2,
        ..Default::default()
    })?;
    let last = app.terminal_buffer().area.bottom() - 1;
    helpers::send_keys(&mut app, ":wri").await;
    assert!(
        row(&app, last - 1).starts_with("λ wri"),
        "{:?}",
        row(&app, last - 1)
    );
    helpers::send_keys(&mut app, "<esc>:o<tab>").await;
    let screen = app.terminal_buffer();
    let menu_rows = (screen.area.top()..last - 1)
        .filter(|&y| row(&app, y).contains("open"))
        .count();
    assert!(menu_rows <= 2, "at most bottom-rows rows");

    // A rounded popup at the top left, without title or hints.
    let mut app = cmdline_app(CmdlineConfig {
        style: CmdlineStyle::Popup,
        border: CmdlineBorder::Rounded,
        popup_top: 0,
        popup_left: 0,
        title: false,
        hints: false,
        selected_marker: "» ".to_string(),
        ..Default::default()
    })?;
    helpers::send_keys(&mut app, ":wri<tab>").await;
    let top = row(&app, 0);
    assert!(top.starts_with("╭──"), "{top:?}");
    assert!(!top.contains("Command") && !top.contains("Tab"), "{top:?}");
    assert!(row(&app, 1).starts_with("│ :wri"), "{:?}", row(&app, 1));
    assert!(row(&app, 2).starts_with("├"), "{:?}", row(&app, 2));
    let screen = app.terminal_buffer();
    assert!(
        (screen.area.top()..screen.area.bottom()).any(|y| row(&app, y).contains("│» ")),
        "the selected marker"
    );
    Ok(())
}
