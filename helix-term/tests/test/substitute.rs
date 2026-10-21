//! `:s/pattern/replacement/flags`

use helix_term::application::Application;
use helix_view::editor::Severity;

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn current_line_and_global_flag() -> anyhow::Result<()> {
    // Without a range and with just a cursor: the cursor's line, first match.
    test(("#[a|]#a a\na a a\n", ":s/a/b/<ret>", "#[b|]#a a\na a a\n")).await?;
    test(("#[a|]#a a\na a a\n", ":s/a/b/g<ret>", "#[b|]#b b\na a a\n")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn whole_document_and_line_ranges() -> anyhow::Result<()> {
    test((
        "#[o|]#ne\ntwo\none\n",
        ":%s/one/1/<ret>",
        // The cursor goes to the last changed line, like in Vim.
        "1\ntwo\n#[1|]#\n",
    ))
    .await?;
    test(("#[x|]#\nx\nx\nx\n", ":2,3s/x/y/<ret>", "x\ny\n#[y|]#\nx\n")).await?;
    test(("#[x|]#\nx\nx\n", ":2,$s/x/y/<ret>", "x\ny\n#[y|]#\n")).await?;
    test(("x\n#[x|]#\nx\n", ":.s/x/z/<ret>", "x\n#[z|]#\nx\n")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn within_selections() -> anyhow::Result<()> {
    // A real selection limits the substitution to the selected text.
    test(("aaa #[aaa|]# aaa", ":s/a/b/g<ret>", "aaa #[bbb|]# aaa")).await?;
    // Several cursors: their lines.
    test((
        "#[a|]#a\naa\n#(a|)#a\n",
        ":s/a/b/<ret>",
        "#[b|]#a\naa\n#(b|)#a\n",
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_groups_and_case() -> anyhow::Result<()> {
    test((
        "#[J|]#ohn Smith\n",
        ":s/(\\w+) (\\w+)/\\U\\2\\E, \\1/<ret>",
        "#[S|]#MITH, John\n",
    ))
    .await?;
    test((
        "#[f|]#oo_bar_baz\n",
        ":s/_(\\w)/\\u$1/g<ret>",
        "#[f|]#ooBarBaz\n",
    ))
    .await?;
    test((
        "#[2|]#026-10-19\n",
        // `<lt>` and `<gt>` are how the key notation of tests writes `<` and `>`.
        ":s#(?<lt>y<gt>\\d+)-(?<lt>m<gt>\\d+)-(?<lt>d<gt>\\d+)#${d}/${m}/${y}#<ret>",
        "#[1|]#9/10/2026\n",
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn one_eyed_kirby() -> anyhow::Result<()> {
    // Vim's capture-everything group `\(.*\)`, and the same as `(.*)`.
    test((
        "#[a|]#lpha\nbeta\n",
        ":%s/^\\(.*\\)$/\"\\1\",/<ret>",
        "\"alpha\",\n#[\"|]#beta\",\n",
    ))
    .await?;
    test((
        "#[a|]#lpha\nbeta\n",
        ":%s/(.*)/[$1]/<ret>",
        "[alpha]\n#[[|]#beta]\n",
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn vim_regex_recipes() -> anyhow::Result<()> {
    // `\zs`: only the number is replaced.
    test((
        "#[p|]#rice: 100\n",
        ":s/price: \\zs\\d\\+/200/g<ret>",
        "#[p|]#rice: 200\n",
    ))
    .await?;
    // Swap around `=` in very magic mode.
    test((
        "#[a|]# = b\n",
        ":s/\\v(\\w+)\\s*\\=\\s*(\\w+)/\\2 = \\1/<ret>",
        "#[b|]# = a\n",
    ))
    .await?;
    // Trailing whitespace; `e` makes "no match" silent.
    test(("#[a|]#  \nb\t\n", ":%s/\\s\\+$//e<ret>", "a\n#[b|]#\n")).await?;
    // Sleeping Kirby and `\r`.
    test((
        "#[<|]#a><b>\n",
        // `<lt>` and `<gt>` are `<` and `>` in the key notation of tests.
        ":s/<lt>\\(.\\{-}\\)<gt>/\\1\\r/g<ret>",
        "#[a|]#\nb\n\n",
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn quiet_flag() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[a|]#bc\n")
        .build()?;
    test_key_sequence(
        &mut app,
        Some(":s/x/y/e<ret>"),
        Some(&|app| assert!(app.editor.get_status().is_none())),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn substitute_long_form() -> anyhow::Result<()> {
    test(("#[a|]#bc\n", ":substitute /b/X/<ret>", "#[a|]#Xc\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn one_undo_step_and_search_register() -> anyhow::Result<()> {
    // Undo reverts the whole substitution.
    test(("#[a|]#a\naa\n", ":%s/a/b/g<ret>u", "#[a|]#a\naa\n")).await?;
    // The pattern becomes the last search: `n` finds the next match.
    test((
        "#[x|]# foo\nfoo\nfoo\n",
        ":s/zzz//<ret>:s/foo/bar/<ret>n",
        "x bar\n#[foo|]#\nfoo\n",
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_and_counting() -> anyhow::Result<()> {
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[a|]#bc\n")
        .build()?;
    test_key_sequence(
        &mut app,
        Some(":s/x/y/<ret>"),
        Some(&|app| {
            let (message, severity) = app.editor.get_status().unwrap();
            assert_eq!(*severity, Severity::Error);
            assert_eq!(message, "Pattern not found: x");
        }),
        false,
    )
    .await?;

    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[a|]#bc\n")
        .build()?;
    test_key_sequence(
        &mut app,
        Some(":s/(b/y/<ret>"),
        Some(&|app| {
            let (message, severity) = app.editor.get_status().unwrap();
            assert_eq!(*severity, Severity::Error);
            assert!(message.contains("invalid pattern"), "{message}");
        }),
        false,
    )
    .await?;

    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[a|]#a a\na\n")
        .build()?;
    test_key_sequence(
        &mut app,
        Some(":%s/a//gn<ret>"),
        Some(&|app| {
            assert_eq!(app.editor.get_status().unwrap().0, "4 matches on 2 lines");
            // Nothing changed.
            assert_eq!(helix_view::doc!(app.editor).text().to_string(), "aa a\na\n");
        }),
        false,
    )
    .await?;

    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[a|]#a a\na\n")
        .build()?;
    test_key_sequence(
        &mut app,
        Some(":%s/a/b/g<ret>"),
        Some(&|app| {
            assert_eq!(
                app.editor.get_status().unwrap().0,
                "4 substitutions on 2 lines"
            );
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn other_s_commands_still_work() -> anyhow::Result<()> {
    // `:sort` is not mistaken for a substitution.
    test((
        "#[c|]#\n#(b|)#\n#(a|)#\n",
        ":sort<ret>",
        "#[a|]#\n#(b|)#\n#(c|)#\n",
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn space_v_very_magic_on_selection() -> anyhow::Result<()> {
    // `space v` opens `:s/\v`: groups and `+` need no backslashes, and only the
    // selection changes.
    test((
        "foo_bar #[foo_bar baz_qux|]#",
        r"<space>v(\w+)_(\w+)/\2_\1/g<ret>",
        "foo_bar #[bar_foo qux_baz|]#",
    ))
    .await?;
    // With just a cursor: its line.
    test((
        "one two\n#[t|]#hree four\n",
        r"<space>v(\w+) (\w+)/\2 \1/<ret>",
        "one two\n#[f|]#our three\n",
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_preview() -> anyhow::Result<()> {
    use helix_term::commands::substitute_preview::snapshot;

    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[p|]#rint( hello )\nprint(x)\nother\n")
        .build()?;
    let text = |app: &Application| helix_view::doc!(app.editor).text().to_string();

    // While the pattern is typed: the matching lines, unchanged.
    helpers::send_keys(&mut app, r":%s/\vprint\(\s*(.*)\)").await;
    let (title, lines) = snapshot().expect("a preview");
    assert_eq!(title, "2 matches on 2 lines");
    assert_eq!(lines, ["print( hello )", "print(x)"]);

    // With a replacement: the lines as they will be. The buffer is untouched.
    helpers::send_keys(&mut app, r"/say(\1)").await;
    assert_eq!(snapshot().unwrap().1, ["say(hello )", "say(x)"]);
    assert_eq!(text(&app), "print( hello )\nprint(x)\nother\n");

    // Esc drops the preview and changes nothing.
    helpers::send_keys(&mut app, "<esc>").await;
    assert!(snapshot().is_none());
    assert_eq!(text(&app), "print( hello )\nprint(x)\nother\n");

    // A bad pattern shows the error; Enter applies and drops the preview.
    helpers::send_keys(&mut app, r":s/\(").await;
    assert!(snapshot().is_some_and(|(title, lines)| lines.is_empty() && !title.is_empty()));
    helpers::send_keys(&mut app, r"<backspace><backspace>print/P/<ret>").await;
    assert!(snapshot().is_none());
    assert_eq!(text(&app), "P( hello )\nprint(x)\nother\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_preview_numbers_source_lines_and_splits_cr_endings() -> anyhow::Result<()> {
    use helix_term::commands::substitute_preview::{snapshot, snapshot_line_numbers};
    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[a|]#\na\n")
        .build()?;
    helpers::send_keys(&mut app, r":%s/a/x\ny/g").await;
    assert_eq!(snapshot().unwrap().1, ["x", "y", "x", "y"]);
    assert_eq!(
        snapshot_line_numbers().unwrap(),
        [Some(0), None, Some(1), None]
    );
    assert_eq!(helix_view::doc!(app.editor).text().to_string(), "a\na\n");
    helpers::send_keys(&mut app, "<esc>").await;

    // Replace the same document through ordinary editing to keep one app/job queue.
    helpers::send_keys(&mut app, "%c<esc>").await;
    let (view, doc) = helix_view::current!(app.editor);
    let transaction = helix_core::Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some("a\ra\r".into()))].into_iter(),
    );
    doc.apply(&transaction, view.id);
    helpers::send_keys(&mut app, ":%s/a/x/g").await;
    assert_eq!(snapshot().unwrap().1, ["x", "x"]);
    #[cfg(feature = "unicode-lines")]
    assert_eq!(snapshot_line_numbers().unwrap(), [Some(0), Some(1)]);
    // Without unicode-lines the editor treats CR-only text as one source
    // line; the preview still splits it, leaving continuation rows blank.
    #[cfg(not(feature = "unicode-lines"))]
    assert_eq!(snapshot_line_numbers().unwrap(), [Some(0), None]);
    assert_eq!(helix_view::doc!(app.editor).text().to_string(), "a\ra\r");
    helpers::send_keys(&mut app, "<esc>").await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_counts_generated_rows() -> anyhow::Result<()> {
    use helix_term::commands::substitute_preview::{snapshot, snapshot_rows};

    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[a|]#\nb\nc\n")
        .build()?;
    // Each of 3 lines becomes 4 rows: 12 rows from 3 source lines.
    helpers::send_keys(&mut app, r":%s/\w/x\ry\rz\rw/").await;
    let (title, lines) = snapshot().expect("a preview");
    assert_eq!(title, "3 matches on 3 lines");
    assert_eq!(lines.len(), 12);
    assert_eq!(snapshot_rows(), Some((12, false)));
    helpers::send_keys(&mut app, "<esc>").await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_preview_shows_changes_in_place() -> anyhow::Result<()> {
    use helix_view::graphics::Modifier;
    use std::cell::RefCell;

    let mut app = helpers::AppBuilder::new()
        .with_input_text("#[[|]#workspace]\nresolver = \"2\"\n")
        .build()?;
    let screen = RefCell::new(None);
    test_key_sequence(
        &mut app,
        Some(":%s/workspace/what"),
        Some(&|app: &Application| {
            *screen.borrow_mut() = Some(app.terminal_buffer().clone());
        }),
        false,
    )
    .await?;
    let screen = screen.into_inner().expect("a rendered screen");
    let area = screen.area;
    let row = |y: u16| -> String {
        (area.x..area.right())
            .map(|x| screen[(x, y)].symbol.to_string())
            .collect()
    };
    // The buffer's line: the match struck out, the replacement after it.
    let (y, x) = (area.y..area.bottom())
        .find_map(|y| {
            let row = row(y);
            row.find("[workspacewhat]").map(|x| (y, x as u16))
        })
        .expect("the line with its change shown in place");
    let struck = &screen[(x + 1, y)];
    assert_eq!(struck.symbol.to_string(), "w");
    assert!(struck.modifier.contains(Modifier::CROSSED_OUT));
    let added = &screen[(x + 10, y)];
    assert_eq!(added.symbol.to_string(), "w");
    assert!(!added.modifier.contains(Modifier::CROSSED_OUT));
    assert_ne!(added.fg, struck.fg);
    // Unchanged text keeps its style.
    assert!(!screen[(x, y)].modifier.contains(Modifier::CROSSED_OUT));
    assert!(!screen[(x + 14, y)].modifier.contains(Modifier::CROSSED_OUT));
    Ok(())
}
