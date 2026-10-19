//! `:s/pattern/replacement/flags`

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
