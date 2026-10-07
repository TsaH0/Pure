use std::cell::RefCell;

use helix_core::doc_formatter::set_skip_plain_text;
use helix_term::application::Application;
use helix_view::editor::{WhitespaceConfig, WhitespaceRender, WhitespaceRenderValue};
use tui::buffer::Buffer;

use super::*;

/// Lines long enough to be scrolled horizontally, with some graphemes that are not plain ASCII.
fn long_lines_text() -> String {
    let mut first = String::from("    ");
    let mut second = String::new();
    for i in 0..400 {
        first.push_str(&format!("word{i} "));
        match i % 50 {
            7 => first.push('\t'),
            19 => first.push_str("日本 "),
            31 => first.push_str("e\u{301} "),
            43 => first.push_str("😶\u{200d}🌫\u{fe0f} "),
            _ => {}
        }
        second.push_str(&format!("value_{i}, "));
    }
    format!("short line\n{first}\n\n{second}\nlast line\n")
}

/// Long lines of Lua code with strings, comments, nested functions and local variables.
fn long_lua_lines() -> String {
    let mut line = String::from("local t = {");
    let mut nested = String::from("local function f(a, b) ");
    for i in 0..300 {
        line.push_str(&format!(
            "k{i} = {{ a = {i}, s = \"str\\t{i}\", f = function(x) return x + a end }}, "
        ));
        if i % 40 == 7 {
            line.push_str("--[[ block TODO: comment ]] ");
        }
        nested.push_str(&format!(
            "local v{i} = a + b * {i} if v{i} > 3 then b = v{i} end "
        ));
    }
    line.push_str("} -- FIXME: trailing comment");
    nested.push_str("return a end");
    format!("-- header\n{line}\n--[[ multi\nline ]]\n{nested}\nlocal x = 1\n")
}

/// Long lines of Rust code with doc comments (injected markdown), macros and strings.
fn long_rust_lines() -> String {
    let mut doc = String::from("/// Docs with `code`, **bold** and [links](https://example.com)");
    let mut code = String::from("fn f<'a>(x: &'a str) -> Vec<u32> { vec![");
    for i in 0..400 {
        doc.push_str(&format!(" more *docs* `x{i}`"));
        code.push_str(&format!("{i}u32, x.len() as u32 + {i}, "));
        if i % 50 == 13 {
            code.push_str("/* inline */ ");
            code.push_str(&format!("println!(\"{{}} {i}\", \"s\").len() as u32, "));
        }
    }
    code.push_str("] }");
    format!("{doc}\n{code}\nconst X: &str = \"done\";\n")
}

/// Renders the result of `keys` with and without skipping plain text when traversing the
/// document and asserts that the rendered screens are identical.
async fn assert_same_rendering(soft_wrap: bool, keys: &str) -> anyhow::Result<()> {
    assert_same_rendering_of(&long_lines_text(), "txt", soft_wrap, keys).await
}

async fn assert_same_rendering_of(
    text: &str,
    extension: &str,
    soft_wrap: bool,
    keys: &str,
) -> anyhow::Result<()> {
    let mut file = tempfile::Builder::new()
        .suffix(&format!(".{extension}"))
        .tempfile()?;
    std::io::Write::write_all(&mut file, text.as_bytes())?;
    let mut config = helpers::test_config();
    config.editor.cursorline = true;
    config.editor.rulers = vec![40, 100];
    config.editor.indent_guides.render = true;
    config.editor.soft_wrap.enable = Some(soft_wrap);
    config.editor.whitespace = WhitespaceConfig {
        render: WhitespaceRender::Basic(WhitespaceRenderValue::All),
        ..Default::default()
    };

    let mut screens = Vec::new();
    for skip in [false, true] {
        set_skip_plain_text(skip);
        let screen = RefCell::new(None::<Buffer>);
        let mut app = helpers::AppBuilder::new()
            .with_file(file.path(), None)
            .with_config(config.clone())
            .build()?;
        test_key_sequence(
            &mut app,
            Some(keys),
            Some(&|app: &Application| {
                *screen.borrow_mut() = Some(app.terminal_buffer().clone());
            }),
            false,
        )
        .await?;
        screens.push(screen.into_inner().expect("screen was rendered"));
    }
    set_skip_plain_text(true);

    assert_eq!(
        screens[0], screens[1],
        "rendering differs when skipping plain text (file type: {extension}, soft wrap: \
         {soft_wrap}, keys: {keys})"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn skipping_plain_text_renders_the_same() -> anyhow::Result<()> {
    let keys = [
        // the end of a long indented line with tabs and wide/combining graphemes
        "jgl",
        "jgl200h",
        // scrolled just far enough that the indentation of the line is off-screen
        "j100l",
        "j120l",
        // many cursors along the line, some of them left of the viewport
        "%sword\\d*5 <ret>",
        "j500l",
        "j1000lvbb",
        "jglgs",
        // a long line of plain text
        "jjjgl",
        "jjj300l",
        "jjjgl40h",
        // a match highlight and jump labels (overlays) on a scrolled line
        "jgl/word3<ret>",
        "jjj700lgw",
        // edit at the end of a long line
        "jjjglaabc<esc>",
        "jA end<esc>",
    ];
    for soft_wrap in [false, true] {
        for keys in keys {
            assert_same_rendering(soft_wrap, keys).await?;
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn skipping_plain_text_renders_highlighted_code_the_same() -> anyhow::Result<()> {
    let lua_keys = [
        "jgl",
        "jgl300h",
        "j3000l",
        "j5000lvbb",
        "jjjjgl",
        "jjjj2000l",
        "jjjj6000l",
        "jglaabc<esc>",
        "jjjjgli -- x <esc>",
        "j4000lgw",
    ];
    let rust_keys = [
        "gl",
        "3000l",
        "gl500h",
        "jgl",
        "j2500l",
        "j6000l",
        "jglA, 1<esc>",
    ];
    for soft_wrap in [false, true] {
        for keys in lua_keys {
            assert_same_rendering_of(&long_lua_lines(), "lua", soft_wrap, keys).await?;
        }
        for keys in rust_keys {
            assert_same_rendering_of(&long_rust_lines(), "rs", soft_wrap, keys).await?;
        }
    }
    Ok(())
}
