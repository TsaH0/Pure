//! End-to-end tests of WebAssembly plugins in a running editor, using the
//! test plugin and the example plugin from `helix-plugin`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use helix_term::application::Application;
use helix_view::editor::Severity;

use super::*;

fn helix_plugin_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../helix-plugin")
}

/// A plugins directory with `testplugin` (with `permissions`) and the
/// `text-tools` example.
fn plugins_dir(permissions: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let test = root.path().join("testplugin");
    std::fs::create_dir_all(&test).unwrap();
    std::fs::copy(
        helix_plugin_dir().join("tests/fixtures/testplugin.wasm"),
        test.join("plugin.wasm"),
    )
    .unwrap();
    std::fs::write(
        test.join("plugin.toml"),
        format!("name = \"testplugin\"\nversion = \"1.0.0\"\napi = \"0.1\"\npermissions = [{permissions}]\n"),
    )
    .unwrap();

    let tools = root.path().join("text-tools");
    std::fs::create_dir_all(&tools).unwrap();
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/text-tools").join(file),
            tools.join(file),
        )
        .unwrap();
    }
    root
}

const ALL: &str = r#""read-buffers", "edit-buffers", "open-files", "run-commands""#;

/// An editor with `text` (in the test selection syntax), committed to the
/// undo history, and the plugins loaded with `settings`.
fn app_with_plugins(
    text: &str,
    permissions: &str,
    settings: &str,
) -> (Application, tempfile::TempDir) {
    let mut app = AppBuilder::new().with_input_text(text).build().unwrap();
    let (view, doc) = helix_view::current!(app.editor);
    doc.append_changes_to_history(view);
    let dir = plugins_dir(permissions);
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings).unwrap();
    (app, dir)
}

fn text(app: &Application) -> String {
    helix_view::doc!(app.editor).text().to_string()
}

fn status(app: &Application) -> (String, Severity) {
    let (message, severity) = app.editor.get_status().expect("a status message");
    (message.to_string(), *severity)
}

async fn keys(app: &mut Application, keys: &str) {
    send_keys(app, keys).await
}

/// Let debounced plugin events be delivered.
async fn deliver_events(app: &mut Application) {
    tokio::time::sleep(Duration::from_millis(150)).await;
    // The idle timer already fired; without new input nothing would re-arm it.
    app.editor.reset_idle_timer();
    run_event_loop_until_idle(app).await;
}

fn cursor(app: &Application) -> usize {
    let (view, doc) = helix_view::current_ref!(app.editor);
    doc.selection(view.id)
        .primary()
        .cursor(doc.text().slice(..))
}

fn primary(app: &Application) -> (usize, usize) {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let range = doc.selection(view.id).primary();
    (range.from(), range.to())
}

fn assert_error(app: &Application, contains: &str) {
    let (message, severity) = status(app);
    assert_eq!(severity, Severity::Error, "{message}");
    assert!(
        message.contains(contains),
        "expected '{contains}' in '{message}'"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_load_and_register_commands() -> anyhow::Result<()> {
    let (app, _dir) = app_with_plugins("#[a|]#", ALL, "");
    assert!(
        app.editor.get_status().is_none(),
        "{:?}",
        app.editor.get_status()
    );
    let names = helix_term::plugins::command_names();
    assert!(names.contains(&"text-tools/upper".to_string()), "{names:?}");
    assert!(
        names.contains(&"testplugin/status".to_string()),
        "{names:?}"
    );
    let list = helix_term::plugins::describe();
    assert!(list.contains("**text-tools** 0.1.0"), "{list}");
    assert!(list.contains("`testplugin/loop`"), "{list}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_edits_are_one_undo_step() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", ALL, "");
    keys(&mut app, ":plugin-run testplugin/two-edits<ret>").await;
    assert_eq!(text(&app), "bahello");
    assert_eq!(status(&app), ("start=ba".into(), Severity::Info));
    keys(&mut app, "u").await;
    assert_eq!(text(&app), "hello");
    keys(&mut app, "U").await;
    assert_eq!(text(&app), "bahello");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn example_plugin_transforms_selections() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[hello|]# world", ALL, "");
    keys(&mut app, ":plugin-run text-tools/upper<ret>").await;
    assert_eq!(text(&app), "HELLO world");
    // The selection is kept.
    assert_eq!(primary(&app), (0, 5));
    keys(&mut app, "%:plugin-run text-tools/lower<ret>").await;
    assert_eq!(text(&app), "hello world");

    // With just the cursor, the whole document.
    let (mut app, _dir) = app_with_plugins("#[p|]#ear\napple\nfig\n", ALL, "");
    keys(&mut app, ":plugin-run text-tools/sort-lines<ret>").await;
    assert_eq!(text(&app), "apple\nfig\npear\n");
    assert_eq!(status(&app).0, "sorted 3 lines");
    keys(&mut app, ":plugin-run text-tools/count<ret>").await;
    assert_eq!(status(&app).0, "document: 3 lines, 3 words, 15 chars");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_sets_selections() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", ALL, "");
    keys(&mut app, ":plugin-run testplugin/select-all<ret>").await;
    assert_eq!(primary(&app), (0, 5));
    // Editor commands work on the new selection.
    keys(&mut app, "d").await;
    assert_eq!(text(&app), "");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_errors_are_shown() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", ALL, "");
    keys(&mut app, ":plugin-run testplugin/fail oops<ret>").await;
    assert_error(&app, "testplugin/fail: failed: oops");
    keys(&mut app, ":plugin-run nope/cmd<ret>").await;
    assert_error(&app, "no plugin named 'nope'");
    keys(&mut app, ":plugin-run testplugin/nope<ret>").await;
    assert_error(&app, "has no command 'nope'");
    keys(&mut app, ":plugin-run no-slash<ret>").await;
    assert_error(&app, "expected <plugin>/<command>");
    // Invalid edits are rejected and change nothing.
    keys(&mut app, ":plugin-run testplugin/bad-edit<ret>").await;
    assert_error(&app, "for a document of 5 chars");
    keys(&mut app, ":plugin-run testplugin/overlap<ret>").await;
    assert_error(&app, "overlap");
    assert_eq!(text(&app), "hello");
    // Errors don't disable the plugin.
    keys(&mut app, ":plugin-run testplugin/status still here<ret>").await;
    assert_eq!(status(&app), ("still here".into(), Severity::Info));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn crashing_plugins_are_disabled_and_the_editor_keeps_working() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", ALL, "");
    keys(&mut app, ":plugin-run testplugin/panic<ret>").await;
    assert_error(&app, "was disabled because it crashed");
    keys(&mut app, ":plugin-run testplugin/status hi<ret>").await;
    assert_error(&app, "is disabled");
    assert!(helix_term::plugins::describe().contains("disabled: it crashed"));
    // Other plugins and the editor still work.
    keys(&mut app, "ixy<esc>:plugin-run text-tools/count<ret>").await;
    assert_eq!(text(&app), "xyhello");
    assert_eq!(status(&app).0, "document: 1 lines, 1 words, 7 chars");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn runaway_plugins_are_stopped() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", ALL, "");
    let start = std::time::Instant::now();
    keys(&mut app, ":plugin-run testplugin/loop<ret>").await;
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    assert_error(&app, "took longer than 2000 ms");
    keys(&mut app, ":plugin-run testplugin/alloc<ret>").await;
    assert_error(&app, "is disabled");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_run_editor_commands() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello\nworld", ALL, "");
    // A static command.
    keys(&mut app, ":plugin-run testplugin/run goto_line_end<ret>").await;
    assert_eq!(cursor(&app), 4);
    // A typable command with arguments.
    keys(
        &mut app,
        ":plugin-run testplugin/run :echo from the plugin<ret>",
    )
    .await;
    assert_eq!(status(&app).0, "from the plugin");
    // An unknown command.
    keys(&mut app, ":plugin-run testplugin/run no_such_command<ret>").await;
    assert_error(&app, "Plugin command failed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_open_files() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("from disk")?;
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", ALL, "");
    keys(
        &mut app,
        &format!(":plugin-run testplugin/open {}<ret>", file.path().display()),
    )
    .await;
    assert_eq!(text(&app), "from disk");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn permissions_are_enforced_in_the_editor() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", r#""read-buffers""#, "");
    keys(&mut app, ":plugin-run testplugin/insert x<ret>").await;
    assert_error(&app, "permission denied: add \"edit-buffers\"");
    assert_eq!(text(&app), "hello");
    keys(&mut app, ":plugin-run testplugin/run goto_line_end<ret>").await;
    assert_error(&app, "permission denied: add \"run-commands\"");
    keys(&mut app, ":plugin-run testplugin/read<ret>").await;
    assert_eq!(status(&app).0, "read 5 chars");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_plugins_are_not_loaded() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", ALL, "[testplugin]\nenabled = false\n");
    assert!(!helix_term::plugins::command_names()
        .iter()
        .any(|c| c.starts_with("testplugin/")));
    keys(&mut app, ":plugin-run testplugin/status hi<ret>").await;
    assert_error(&app, "no plugin named 'testplugin'");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_are_passed_to_plugins() -> anyhow::Result<()> {
    let (app, _dir) = app_with_plugins("#[h|]#ello", ALL, "[testplugin]\ninit-status = true\n");
    assert_eq!(status(&app).0, "testplugin loaded");
    let (app, _dir) = app_with_plugins("#[h|]#ello", ALL, "[testplugin]\nfail-init = true\n");
    assert_error(&app, "init failed on purpose");
    // The other plugin still loaded.
    assert!(helix_term::plugins::command_names().contains(&"text-tools/upper".to_string()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_receive_events() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", ALL, "");
    keys(&mut app, "ixyz<esc>").await;
    deliver_events(&mut app).await;
    keys(&mut app, ":plugin-run testplugin/events<ret>").await;
    let message = status(&app).0;
    assert!(message.contains("mode:normal>insert"), "{message}");
    assert!(message.contains("mode:insert>normal"), "{message}");
    // Typing three chars is delivered as one change event, not three.
    assert_eq!(message.matches("change:").count(), 1, "{message}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_see_opened_documents() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("abc")?;
    let (mut app, _dir) =
        app_with_plugins("#[h|]#ello", ALL, "[testplugin]\nstatus-on-open = true\n");
    keys(&mut app, &format!(":open {}<ret>", file.path().display())).await;
    deliver_events(&mut app).await;
    let expected = format!(
        "opened {}",
        helix_stdx::path::canonicalize(file.path()).display()
    );
    assert_eq!(status(&app).0, expected);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_edits_do_not_retrigger_change_events() -> anyhow::Result<()> {
    let (mut app, _dir) =
        app_with_plugins("#[h|]#ello", ALL, "[testplugin]\nedit-on-change = true\n");
    keys(&mut app, "ix<esc>").await;
    // Give a feedback loop plenty of time to show up.
    for _ in 0..4 {
        deliver_events(&mut app).await;
    }
    assert_eq!(text(&app), "xhello!");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_event_handlers_are_disabled() -> anyhow::Result<()> {
    let (mut app, _dir) =
        app_with_plugins("#[h|]#ello", ALL, "[testplugin]\nloop-on-change = true\n");
    keys(&mut app, "ix<esc>").await;
    deliver_events(&mut app).await;
    assert_error(&app, "took longer than");
    assert!(!helix_term::plugins::wants(
        helix_plugin::EventKind::DocumentChange
    ));
    // The editor keeps working and the plugin isn't called anymore.
    keys(&mut app, "iy<esc>").await;
    deliver_events(&mut app).await;
    assert_eq!(text(&app), "xyhello");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_list_and_completion() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello", ALL, "");
    keys(&mut app, ":plugin-list<ret>").await;
    assert!(app
        .editor
        .get_status()
        .is_none_or(|(_, severity)| *severity != Severity::Error));
    let completions = helix_term::ui::completers::plugin_command(&app.editor, "upper");
    assert!(
        completions
            .iter()
            .any(|(_, c)| c.content == "text-tools/upper"),
        "{completions:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn no_plugins_directory() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[h|]#ello").build()?;
    helix_term::plugins::load_with_settings(&mut app.editor, Path::new("/nonexistent"), "")?;
    assert!(helix_term::plugins::command_names().is_empty());
    keys(&mut app, ":plugin-run a/b<ret>").await;
    assert_error(&app, "no plugins are loaded");
    keys(&mut app, "ix<esc>").await;
    assert_eq!(text(&app), "xhello");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn api_02_sdk_plugin_edits_alongside_legacy_plugins() -> anyhow::Result<()> {
    let (mut app, dir) = app_with_plugins("#[hé|]#llo\n", ALL, "");
    let plugin = dir.path().join("testplugin2");
    std::fs::create_dir_all(&plugin)?;
    std::fs::copy(
        helix_plugin_dir().join("tests/fixtures/testplugin2-v0.2.wasm"),
        plugin.join("plugin.wasm"),
    )?;
    std::fs::write(
        plugin.join("plugin.toml"),
        format!("name = \"testplugin2\"\nversion = \"1\"\napi = \"0.2\"\npermissions = [{ALL}]\n"),
    )?;
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    let names = helix_term::plugins::command_names();
    assert!(names.contains(&"testplugin/status".to_owned()));
    assert!(names.contains(&"testplugin2/inspect".to_owned()));
    assert!(names.contains(&"text-tools/upper".to_owned()));
    keys(&mut app, ":plugin-run testplugin2/inspect<ret>").await;
    assert_eq!(text(&app), "v2:héllo\n");
    assert_eq!(status(&app).0, "héllo\n:Mode::Normal");
    keys(&mut app, "u").await;
    assert_eq!(text(&app), "héllo\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn picker_close_hook_runs_only_on_cancel_and_only_once() -> anyhow::Result<()> {
    use helix_term::ui::{overlay::overlaid, Picker, PickerColumn};
    use helix_view::editor::Action;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    let mut app = AppBuilder::new().with_input_text("#[a|]#").build()?;
    for (key, expected_close, expected_action) in [
        ("<esc>", 1, 0),
        ("<C-c>", 1, 0),
        ("<ret>", 0, 1),
        ("<C-s>", 0, 2),
        ("<C-v>", 0, 3),
    ] {
        let closed = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::new(AtomicUsize::new(0));
        let close_count = closed.clone();
        let accept_action = accepted.clone();
        let owned_message = String::from("picker closed");
        helix_term::job::dispatch(move |_, compositor| {
            let picker = Picker::new(
                [PickerColumn::new("Name", |item: &String, _: &()| {
                    item.clone().into()
                })],
                0,
                ["item".to_owned()],
                (),
                move |_, _, action| {
                    accept_action.store(
                        match action {
                            Action::Replace => 1,
                            Action::HorizontalSplit => 2,
                            Action::VerticalSplit => 3,
                            Action::Load => 4,
                        },
                        Ordering::Relaxed,
                    );
                },
            )
            .with_on_close(move |ctx| {
                close_count.fetch_add(1, Ordering::Relaxed);
                ctx.editor.set_status(owned_message);
            });
            compositor.push(Box::new(overlaid(picker)));
        })
        .await;
        keys(&mut app, "").await;
        keys(&mut app, key).await;
        assert_eq!(closed.load(Ordering::Relaxed), expected_close, "{key}");
        assert_eq!(accepted.load(Ordering::Relaxed), expected_action, "{key}");
        keys(&mut app, "<space>'").await;
        keys(&mut app, "<esc>").await;
        assert_eq!(
            closed.load(Ordering::Relaxed),
            expected_close,
            "reopened {key}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn picker_close_hook_cancels_empty_picker_on_enter() -> anyhow::Result<()> {
    use helix_term::ui::{overlay::overlaid, Picker, PickerColumn};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let mut app = AppBuilder::new().with_input_text("#[a|]#").build()?;
    let closed = Arc::new(AtomicUsize::new(0));
    let count = closed.clone();
    helix_term::job::dispatch(move |_, compositor| {
        let picker = Picker::new(
            [PickerColumn::new("Name", |item: &String, _: &()| {
                item.clone().into()
            })],
            0,
            Vec::new(),
            (),
            |_, _, _| panic!("empty picker cannot accept"),
        )
        .with_on_close(move |_| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        compositor.push(Box::new(overlaid(picker)));
    })
    .await;
    keys(&mut app, "").await;
    keys(&mut app, "<ret>").await;
    assert_eq!(closed.load(Ordering::Relaxed), 1);
    Ok(())
}

/// An editor with only the 0.2 test plugin, with `permissions` and the
/// plugin's `plugins.toml` table `settings`.
fn app_with_v2(permissions: &str, settings: &str) -> (Application, tempfile::TempDir) {
    let mut app = AppBuilder::new()
        .with_input_text("#[a|]#\n")
        .build()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let plugin = root.path().join("testplugin2");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::copy(
        helix_plugin_dir().join("tests/fixtures/testplugin2-v0.2.wasm"),
        plugin.join("plugin.wasm"),
    )
    .unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        format!("name = \"testplugin2\"\nversion = \"1\"\napi = \"0.2\"\npermissions = [{permissions}]\n"),
    )
    .unwrap();
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        root.path(),
        &format!("[testplugin2]\n{settings}\n"),
    )
    .unwrap();
    (app, root)
}

const WITH_PROCESS: &str =
    r#""read-buffers", "edit-buffers", "open-files", "run-commands", "process""#;

/// Wait until the status message satisfies `done` (results of programs and
/// pickers arrive asynchronously).
async fn wait_for_status(app: &mut Application, done: impl Fn(&str) -> bool) -> String {
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(30)).await;
        app.editor.reset_idle_timer();
        run_event_loop_until_idle(app).await;
        if let Some((message, _)) = app.editor.get_status() {
            if done(message) {
                return message.to_string();
            }
        }
    }
    panic!("timed out; status: {:?}", app.editor.get_status());
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_run_programs_when_granted() -> anyhow::Result<()> {
    // Without the user's grant, the permission alone is not enough.
    let (mut app, dir) = app_with_v2(WITH_PROCESS, "");
    keys(&mut app, ":plugin-run testplugin2/spawn echo hi<ret>").await;
    assert_error(&app, "allow-process");

    // Grant `sh` only. (Reload rather than build a second application: only
    // the first one in a runtime gets the job queue.)
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[testplugin2]\nallow-process = [\"sh\"]\n",
    )?;
    keys(&mut app, ":plugin-run testplugin2/spawn echo hi<ret>").await;
    assert_error(&app, "'echo' is not in allow-process");
    keys(
        &mut app,
        r#":plugin-run testplugin2/spawn sh -c "echo out; echo err <gt>&2; exit 4"<ret>"#,
    )
    .await;
    let message = wait_for_status(&mut app, |s| s.starts_with("exit:")).await;
    assert_eq!(message, "exit:Some(4);out:out\n;err:err\n");

    // `:plugin-list` shows the grant.
    assert!(helix_term::plugins::describe().contains("process: only sh"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_pickers_report_the_choice_and_the_split() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    for (keys_, expected) in [
        ("<down><ret>", "choice:[1]:Some(OpenAction::Replace)"),
        ("<C-v>", "choice:[0]:Some(OpenAction::Vsplit)"),
        ("<C-s>", "choice:[0]:Some(OpenAction::Hsplit)"),
        ("<A-ret>", "choice:[0]:Some(OpenAction::Replace)"),
        ("<esc>", "choice:[]:None"),
        // Filtering keeps the original index.
        ("gam<ret>", "choice:[2]:Some(OpenAction::Replace)"),
    ] {
        keys(
            &mut app,
            ":plugin-run testplugin2/picker alpha beta gamma<ret>",
        )
        .await;
        // The picker opens once the command's event is handled.
        deliver_events(&mut app).await;
        app.editor.set_status("");
        keys(&mut app, keys_).await;
        let message = wait_for_status(&mut app, |s| s.starts_with("choice:")).await;
        assert_eq!(message, expected, "{keys_}");
    }
    // Answered pickers are not kept for `space '`: it reopens nothing.
    app.editor.set_status("");
    keys(&mut app, "<space>'").await;
    keys(&mut app, "<ret>").await;
    deliver_events(&mut app).await;
    assert!(!app
        .editor
        .get_status()
        .is_some_and(|(s, _)| s.starts_with("choice:")));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn fresh_sdk_02_picker_data_matches_the_ui_after_a_failed_push() -> anyhow::Result<()> {
    let (mut app, dir) = app_with_v2(ALL, "");
    std::fs::copy(
        helix_plugin_dir().join("tests/fixtures/testplugin2.wasm"),
        dir.path().join("testplugin2/plugin.wasm"),
    )?;
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    keys(
        &mut app,
        ":plugin-run testplugin2/data-picker first second<ret>",
    )
    .await;
    let id = status(&app)
        .0
        .strip_prefix("data-picker:")
        .unwrap()
        .parse::<u32>()?;
    helix_term::job::dispatch_callback(helix_term::job::Callback::EditorCompositorJobs(Box::new(
        move |editor, _, jobs| {
            assert!(helix_term::plugins::run_command(
                editor,
                jobs,
                "testplugin2/push-items-fail",
                vec![id.to_string()]
            )
            .is_err());
            helix_term::plugins::run_command(
                editor,
                jobs,
                "testplugin2/push-items",
                vec![id.to_string()],
            )
            .unwrap();
        },
    )))
    .await;
    deliver_events(&mut app).await;
    keys(&mut app, "<down><down><ret>").await;
    assert_eq!(status(&app).0, "data:[\"new item\"]");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_prompts_return_the_answer() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    keys(&mut app, ":plugin-run testplugin2/prompt one two<ret>").await;
    deliver_events(&mut app).await;
    // The prompt starts with "init".
    keys(&mut app, "x<ret>").await;
    let message = wait_for_status(&mut app, |s| s.starts_with("answer:")).await;
    assert_eq!(message, "answer:Some(\"initx\")");

    keys(&mut app, ":plugin-run testplugin2/prompt<ret>").await;
    deliver_events(&mut app).await;
    app.editor.set_status("");
    keys(&mut app, "<esc>").await;
    let message = wait_for_status(&mut app, |s| s.starts_with("answer:")).await;
    assert_eq!(message, "answer:None");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_pickers_show_the_plugins_items_for_the_query() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    keys(&mut app, ":plugin-run testplugin2/live<ret>").await;
    deliver_events(&mut app).await;
    keys(&mut app, "ab").await;
    // Debounced query, then the plugin's answer.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        deliver_events(&mut app).await;
    }
    keys(&mut app, "<down><ret>").await;
    let message = wait_for_status(&mut app, |s| s.starts_with("live:")).await;
    assert_eq!(message, "live:[\"ab-2\"]");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_hear_saves_closes_and_selection_changes() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    let doc = helix_view::doc!(app.editor).id().to_string();

    keys(&mut app, "l").await;
    let message = wait_for_status(&mut app, |s| s.starts_with("selection:")).await;
    assert_eq!(message, format!("selection:{doc}"));

    let file = tempfile::NamedTempFile::new()?;
    app.editor.set_status("");
    keys(&mut app, &format!(":w! {}<ret>", file.path().display())).await;
    let message = wait_for_status(&mut app, |s| s.starts_with("save:")).await;
    assert_eq!(message, format!("save:{doc}"));

    keys(&mut app, ":new<ret>").await;
    app.editor.set_status("");
    keys(&mut app, ":buffer-close! <ret>").await;
    let closed = wait_for_status(&mut app, |s| s.starts_with("close:")).await;
    assert!(closed.starts_with("close:"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_log_shows_what_a_plugin_logged() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    keys(&mut app, ":plugin-log testplugin2<ret>").await;
    assert_eq!(
        status(&app).0,
        "Plugin 'testplugin2' hasn't logged anything"
    );
    keys(&mut app, ":plugin-run testplugin2/log first second<ret>").await;
    keys(&mut app, ":plugin-log testplugin2<ret>").await;
    assert_eq!(text(&app), "first\nsecond\n");
    keys(&mut app, ":plugin-log nope<ret>").await;
    assert_error(&app, "no plugin named 'nope'");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_pickers_preview_text() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    keys(
        &mut app,
        ":plugin-run testplugin2/text-preview alpha beta<ret>",
    )
    .await;
    deliver_events(&mut app).await;
    // Moving renders the preview of each item (the test backend draws).
    keys(&mut app, "<down><up><down><esc>").await;
    deliver_events(&mut app).await;
    assert_eq!(text(&app), "a\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_can_close_a_document_they_edited() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    let closed = helix_view::doc!(app.editor).id();
    keys(&mut app, ":plugin-run testplugin2/edit-close<ret>").await;
    assert!(app.editor.documents().all(|doc| doc.id() != closed));
    keys(&mut app, "iworks<esc>").await;
    assert_eq!(text(&app), "works\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_command_recursion_stops_and_the_depth_recovers() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    for _ in 0..2 {
        keys(&mut app, ":plugin-run testplugin2/recursive<ret>").await;
        assert_error(&app, "nested too deeply");
    }
    keys(&mut app, ":plugin-run testplugin2/log recovered<ret>").await;
    keys(&mut app, ":plugin-log testplugin2<ret>").await;
    assert_eq!(text(&app), "recovered\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_picker_actions_run_in_call_order() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    keys(&mut app, ":plugin-run testplugin2/ordered-picker<ret>").await;
    deliver_events(&mut app).await;
    keys(&mut app, "last<ret>").await;
    assert_eq!(
        wait_for_status(&mut app, |s| s.starts_with("ordered:")).await,
        "ordered:[\"last\"]"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_picker_replacement_cancels_only_the_previous_request() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    keys(&mut app, ":plugin-run testplugin2/two-pickers<ret>").await;
    deliver_events(&mut app).await;
    wait_for_status(&mut app, |s| s == "first:None").await;
    keys(&mut app, "<ret>").await;
    wait_for_status(&mut app, |s| s == "second:Some(OpenAction::Replace)").await;
    keys(&mut app, ":plugin-log testplugin2<ret>").await;
    assert_eq!(text(&app), "first:None\nsecond:Some(OpenAction::Replace)\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_accept_literal_double_dash_and_boolean_disable() -> anyhow::Result<()> {
    let (mut app, dir) = app_with_plugins("#[a|]#", ALL, "");
    keys(&mut app, ":plugin-run testplugin/status -- -x<ret>").await;
    assert_eq!(status(&app).0, "-- -x");
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "testplugin = false")?;
    assert!(!helix_term::plugins::command_names()
        .iter()
        .any(|name| name.starts_with("testplugin/")));
    assert!(helix_term::plugins::command_names().contains(&"text-tools/upper".to_owned()));
    keys(&mut app, ":plugin-run testplugin/status hi<ret>").await;
    assert_error(&app, "no plugin named 'testplugin'");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_event_subscribers_see_the_previous_plugins_edits() -> anyhow::Result<()> {
    let (mut app, dir) = app_with_v2(ALL, "append-on-open = true");
    let second = dir.path().join("second");
    std::fs::create_dir(&second)?;
    std::fs::copy(
        dir.path().join("testplugin2/plugin.wasm"),
        second.join("plugin.wasm"),
    )?;
    let manifest = std::fs::read_to_string(dir.path().join("testplugin2/plugin.toml"))?;
    std::fs::write(
        second.join("plugin.toml"),
        manifest.replace("testplugin2", "second"),
    )?;
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[testplugin2]\nappend-on-open = true\n[second]\nappend-on-open = true",
    )?;
    let file = helpers::temp_file_with_contents("abc")?;
    keys(&mut app, &format!(":open {}<ret>", file.path().display())).await;
    deliver_events(&mut app).await;
    assert_eq!(text(&app), "abc[3][6]");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_blocked_in_wasi_return_control_to_the_editor() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v2(ALL, "");
    let start = std::time::Instant::now();
    keys(&mut app, ":plugin-run testplugin2/sleep<ret>").await;
    assert_error(&app, "took longer than 2000 ms");
    assert!(start.elapsed() < Duration::from_secs(10));
    keys(&mut app, "iworks<esc>").await;
    assert_eq!(text(&app), "worksa\n");
    keys(&mut app, ":plugin-run testplugin2/sleep<ret>").await;
    assert_error(&app, "is disabled");
    Ok(())
}

fn app_with_v3() -> (Application, tempfile::TempDir) {
    app_with_v3_settings(ALL, "")
}

fn app_with_v3_settings(permissions: &str, settings: &str) -> (Application, tempfile::TempDir) {
    let mut app = AppBuilder::new()
        .with_input_text("#[a|]#\n")
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let plugin = dir.path().join("testplugin3");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::copy(
        helix_plugin_dir().join("tests/fixtures/testplugin3.wasm"),
        plugin.join("plugin.wasm"),
    )
    .unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        format!("name = \"testplugin3\"\nversion = \"1\"\napi = \"0.3\"\npermissions = [{permissions}]\n"),
    )
    .unwrap();
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings).unwrap();
    (app, dir)
}

/// Process an input burst in one compositor callback, without an intervening
/// render or debounce job that could hide a stale matcher snapshot.
async fn picker_input_burst(app: &mut Application, keys: &str) {
    let keys = helix_view::input::parse_macro(keys).unwrap();
    helix_term::job::dispatch_callback(helix_term::job::Callback::EditorCompositorJobs(Box::new(
        move |editor, compositor, jobs| {
            for key in keys {
                compositor.handle_event(
                    &helix_view::input::Event::Key(key),
                    &mut helix_term::compositor::Context {
                        editor,
                        jobs,
                        scroll: None,
                    },
                );
            }
        },
    )))
    .await;
    deliver_events(app).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_live_query_input_invalidates_selection_before_debounce() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3();
    keys(&mut app, ":plugin-run testplugin3/live<ret>").await;
    picker_input_burst(&mut app, "a<ret>").await;
    assert_eq!(
        status(&app).0,
        "cancelled",
        "the initial row cannot be accepted for a new query"
    );
    assert!(
        helix_term::plugins::log_lines("testplugin3")?.is_empty(),
        "a closed picker's queued query is dropped"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_live_pickers_ignore_stale_set_and_push_results() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3();
    keys(&mut app, ":plugin-run testplugin3/live<ret>").await;
    keys(&mut app, "a").await;
    assert!(helix_term::plugins::log_lines("testplugin3")?
        .iter()
        .any(|line| line.contains("query:1:a")));
    helix_term::job::dispatch_callback(helix_term::job::Callback::EditorCompositorJobs(Box::new(
        |editor, _, jobs| {
            for args in [vec!["0", "set"], vec!["0", "push", "old", "older"]] {
                helix_term::plugins::run_command(
                    editor,
                    jobs,
                    "testplugin3/items",
                    args.into_iter().map(str::to_owned).collect(),
                )
                .unwrap();
            }
        },
    )))
    .await;
    deliver_events(&mut app).await;
    keys(&mut app, "<down><ret>").await;
    assert_eq!(
        status(&app).0,
        "picked:[0]",
        "only the current generation's row remains"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_live_queries_that_revert_still_get_a_new_generation() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3();
    keys(&mut app, ":plugin-run testplugin3/live<ret>").await;
    keys(&mut app, "a").await;
    picker_input_burst(&mut app, "b<backspace>").await;
    let lines = helix_term::plugins::log_lines("testplugin3")?;
    assert!(
        lines.iter().any(|line| line.contains("query:1:a")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("query:3:a")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("query:2:ab")),
        "the intermediate query is debounced"
    );
    keys(&mut app, "<ret>").await;
    assert_eq!(status(&app).0, "picked:[0]");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_streams_before_exit_and_callbacks_see_edits_in_order() -> anyhow::Result<()> {
    let (mut app, dir) =
        app_with_v3_settings(WITH_PROCESS, "[testplugin3]\nallow-process = [\"sh\"]\n");
    let script = dir.path().join("stream.sh");
    let gate = dir.path().join("continue");
    std::fs::write(&script, "printf 'first\\n'\nwhile [ ! -f \"$1\" ]; do sleep .01; done\nprintf 'second\\nend'\nprintf err >&2\n")?;
    keys(
        &mut app,
        &format!(
            ":plugin-run testplugin3/stream sh '{}' '{}'<ret>",
            script.display(),
            gate.display()
        ),
    )
    .await;
    let first = wait_for_status(&mut app, |message| message.starts_with("chunk:first")).await;
    assert_eq!(first, "chunk:first\n:snapshot:a\n");
    assert!(!gate.exists(), "the child is still waiting");
    keys(&mut app, "ix<esc>").await;
    std::fs::write(gate, "continue")?;
    let done = wait_for_status(&mut app, |message| message.starts_with("done:")).await;
    assert_eq!(done, "done:Some(0):end:err");
    keys(&mut app, ":plugin-run testplugin3/output-events<ret>").await;
    let output = status(&app).0;
    assert!(output.contains("chunk:first\\n:snapshot:a\\n"), "{output}");
    assert!(
        output.contains("chunk:second\\n:snapshot:xa\\n"),
        "{output}"
    );
    assert!(output.find("chunk:first").unwrap() < output.find("chunk:second").unwrap());
    assert!(output.find("chunk:second").unwrap() < output.find("done:").unwrap());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_streams_finish_after_kill_and_spawn_errors() -> anyhow::Result<()> {
    let (mut app, _dir) =
        app_with_v3_settings(WITH_PROCESS, "[testplugin3]\nallow-process = true\n");
    keys(
        &mut app,
        r#":plugin-run testplugin3/stream sh -c "sleep 30"<ret>"#,
    )
    .await;
    let id = status(&app)
        .0
        .strip_prefix("stream-id:")
        .unwrap()
        .parse::<u32>()?;
    keys(&mut app, &format!(":plugin-run testplugin3/kill {id}<ret>")).await;
    assert_eq!(
        wait_for_status(&mut app, |message| message.starts_with("done:")).await,
        "done:None::[killed]"
    );
    keys(
        &mut app,
        ":plugin-run testplugin3/stream /nonexistent/program<ret>",
    )
    .await;
    assert!(
        wait_for_status(&mut app, |message| message.starts_with("error:"))
            .await
            .contains("failed to run '/nonexistent/program'")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_process_requests_are_dropped_on_reload_and_disable() -> anyhow::Result<()> {
    let settings = "[testplugin3]\nallow-process = [\"sh\"]\n";
    let (mut app, dir) = app_with_v3_settings(WITH_PROCESS, settings);
    let script = dir.path().join("must-not-run.sh");
    let marker = dir.path().join("ran");
    std::fs::write(&script, "printf ran > \"$1\"\n")?;
    for (index, command) in [
        "spawn-reload",
        "interactive-reload",
        "spawn-disable",
        "interactive-disable",
    ]
    .into_iter()
    .enumerate()
    {
        helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings)?;
        keys(
            &mut app,
            &format!(
                ":plugin-run testplugin3/{command} sh '{}' '{}'<ret>",
                script.display(),
                marker.display()
            ),
        )
        .await;
        deliver_events(&mut app).await;
        assert!(!marker.exists(), "{command} started a cancelled request");
        assert!(
            !helix_term::plugins::command_names()
                .iter()
                .any(|name| name.starts_with("testplugin3/")),
            "{command} must have removed or disabled the owner"
        );
        keys(&mut app, "ix<esc>").await;
        assert_eq!(
            text(&app),
            format!("{}a\n", "x".repeat(index + 1)),
            "{command} must not leave a queued terminal handover"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_then_kill_in_one_call_keeps_action_order() -> anyhow::Result<()> {
    let (mut app, _dir) =
        app_with_v3_settings(WITH_PROCESS, "[testplugin3]\nallow-process = [\"sh\"]\n");
    keys(
        &mut app,
        r#":plugin-run testplugin3/spawn-kill sh -c "sleep 30"<ret>"#,
    )
    .await;
    assert_eq!(
        wait_for_status(&mut app, |message| message.starts_with("done:")).await,
        "done:None::[killed]"
    );
    Ok(())
}

async fn v3_focus(app: &mut Application, focused: bool) {
    #[cfg(not(windows))]
    let event = if focused {
        termina::Event::FocusIn
    } else {
        termina::Event::FocusOut
    };
    #[cfg(windows)]
    let event = if focused {
        crossterm::event::Event::FocusGained
    } else {
        crossterm::event::Event::FocusLost
    };
    app.handle_terminal_events(Ok(event)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_command_focus_and_idle_events_do_not_rearm_idle() -> anyhow::Result<()> {
    use helix_plugin::EventKind;
    use helix_term::plugins;
    let (mut app, dir) = app_with_v3();
    plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[testplugin3]\nextended-events = true\n",
    )?;
    keys(&mut app, "hl").await;
    v3_focus(&mut app, false).await;
    v3_focus(&mut app, true).await;
    deliver_events(&mut app).await;
    let lines = plugins::log_lines("testplugin3")?;
    for expected in [
        "event:command:move_char_left",
        "event:command:move_char_right",
        "event:focus:lost",
        "event:focus:gained",
        "event:idle",
    ] {
        assert!(
            lines.iter().any(|line| line.contains(expected)),
            "{lines:?}"
        );
    }
    let focus: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("event:focus:"))
        .collect();
    assert!(focus[0].contains("lost"));
    assert!(focus[1].contains("gained"));
    // No input: the consumed idle timer must stay consumed. A new timer
    // would make this return early and produce another idle notification.
    assert!(tokio::time::timeout(
        Duration::from_millis(700),
        run_event_loop_until_idle(&mut app)
    )
    .await
    .is_err());
    assert_eq!(plugins::log_lines("testplugin3")?, lines);

    plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    for kind in [
        EventKind::PostCommand,
        EventKind::FocusGained,
        EventKind::FocusLost,
        EventKind::EditorIdle,
    ] {
        assert!(!plugins::wants(kind));
    }
    keys(&mut app, "hl").await;
    v3_focus(&mut app, false).await;
    deliver_events(&mut app).await;
    assert!(plugins::log_lines("testplugin3")?.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_opens_files_at_positions_in_the_requested_split() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3();
    let file = helpers::temp_file_with_contents("first\nhéllo\nlast\n")?;
    for (action, views) in [("replace", 1), ("hsplit", 2), ("vsplit", 3)] {
        keys(
            &mut app,
            &format!(
                ":plugin-run testplugin3/open-at '{}' {action} 2 3<ret>",
                file.path().display()
            ),
        )
        .await;
        assert_eq!(text(&app), "first\nhéllo\nlast\n");
        assert_eq!(cursor(&app), 8, "{action}: columns count characters");
        assert_eq!(app.editor.tree.views().count(), views);
    }
    keys(
        &mut app,
        &format!(
            ":plugin-run testplugin3/open-at '{}' replace 4294967295 4294967295<ret>",
            file.path().display()
        ),
    )
    .await;
    assert!(cursor(&app) <= helix_view::doc!(app.editor).text().len_chars());
    Ok(())
}

async fn v3_close_popup_without_typing(app: &mut Application, id: u32) {
    helix_term::job::dispatch_callback(helix_term::job::Callback::EditorCompositorJobs(Box::new(
        move |editor, _, jobs| {
            helix_term::plugins::run_command(
                editor,
                jobs,
                "testplugin3/close-popup",
                vec![id.to_string()],
            )
            .unwrap();
        },
    )))
    .await;
    deliver_events(app).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_popups_close_by_id_and_on_escape_once() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3();
    keys(&mut app, ":plugin-run testplugin3/two-popups<ret>").await;
    deliver_events(&mut app).await;
    let ids: Vec<u32> = status(&app)
        .0
        .strip_prefix("popups:")
        .unwrap()
        .split(',')
        .map(|id| id.parse().unwrap())
        .collect();
    // Closing the lower popup must leave the upper one alive.
    v3_close_popup_without_typing(&mut app, ids[0]).await;
    wait_for_status(&mut app, |message| {
        message == format!("popup-closed:{}", ids[0])
    })
    .await;
    keys(&mut app, "<esc>").await;
    wait_for_status(&mut app, |message| {
        message == format!("popup-closed:{}", ids[1])
    })
    .await;
    v3_close_popup_without_typing(&mut app, ids[0]).await;
    keys(&mut app, ":plugin-run testplugin3/popup-events<ret>").await;
    assert_eq!(status(&app).0, format!("closed:{ids:?}"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_popup_show_then_close_preserves_action_order() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3();
    keys(
        &mut app,
        ":plugin-run testplugin3/popup-close-now centered hello<ret>",
    )
    .await;
    let closed = wait_for_status(&mut app, |message| message.starts_with("popup-closed:")).await;
    let id: u32 = closed.strip_prefix("popup-closed:").unwrap().parse()?;
    keys(&mut app, ":plugin-run testplugin3/popup-events<ret>").await;
    assert_eq!(status(&app).0, format!("closed:[{id}]"));
    keys(&mut app, "iworks<esc>").await;
    assert_eq!(text(&app), "worksa\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn api_03_popup_next_key_closes_it_and_reaches_the_editor() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3();
    keys(&mut app, ":plugin-run testplugin3/popup cursor hello<ret>").await;
    deliver_events(&mut app).await;
    let id: u32 = status(&app).0.strip_prefix("popup:").unwrap().parse()?;
    keys(&mut app, "ix<esc>").await;
    wait_for_status(&mut app, |message| message == format!("popup-closed:{id}")).await;
    assert_eq!(text(&app), "xa\n");
    keys(&mut app, ":plugin-run testplugin3/popup-events<ret>").await;
    assert_eq!(status(&app).0, format!("closed:[{id}]"));
    Ok(())
}

fn app_with_example(name: &str, settings: &str) -> (Application, tempfile::TempDir) {
    let mut app = AppBuilder::new()
        .with_input_text("#[a|]#\n")
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let plugin = dir.path().join(name);
    std::fs::create_dir(&plugin).unwrap();
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples").join(name).join(file),
            plugin.join(file),
        )
        .unwrap();
    }
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings).unwrap();
    (app, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn sdk_03_example_callbacks_and_positioned_open_work_in_editor() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_example("api-demo", "");
    keys(&mut app, ":plugin-run api-demo/popup centered<ret>").await;
    let id = status(&app).0.strip_prefix("popup:").unwrap().to_owned();
    keys(&mut app, "<esc>").await;
    assert_eq!(status(&app).0, format!("closed:{id}"));
    keys(&mut app, ":plugin-run api-demo/live<ret>").await;
    keys(&mut app, "ab").await;
    keys(&mut app, "<ret>").await;
    assert_eq!(status(&app).0, "choice:[\"ab:2\"]");
    let file = helpers::temp_file_with_contents("first\nhéllo\nlast\n")?;
    keys(
        &mut app,
        &format!(
            ":plugin-run api-demo/open-at '{}' vsplit 2 3<ret>",
            file.path().display()
        ),
    )
    .await;
    assert_eq!(text(&app), "first\nhéllo\nlast\n");
    assert_eq!(cursor(&app), 8);
    assert_eq!(app.editor.tree.views().count(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sdk_03_example_streams_before_completion() -> anyhow::Result<()> {
    let (mut app, dir) = app_with_example("api-demo", "[api-demo]\nallow-process = [\"sh\"]\n");
    let script = dir.path().join("stream.sh");
    let gate = dir.path().join("continue");
    std::fs::write(&script, "printf 'first\\n'\nwhile [ ! -f \"$1\" ]; do sleep .01; done\nprintf tail\nprintf err >&2\n")?;
    keys(
        &mut app,
        &format!(
            ":plugin-run api-demo/stream sh '{}' '{}'<ret>",
            script.display(),
            gate.display()
        ),
    )
    .await;
    assert_eq!(
        wait_for_status(&mut app, |message| message.starts_with("chunk:")).await,
        "chunk:first\n"
    );
    assert!(!gate.exists());
    keys(&mut app, "ix<esc>").await;
    std::fs::write(gate, "continue")?;
    assert_eq!(
        wait_for_status(&mut app, |message| message.starts_with("done:")).await,
        "done:Some(0):tail:err"
    );
    assert_eq!(text(&app), "xa\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn displaced_terminal_requests_are_answered() -> anyhow::Result<()> {
    let settings = "[testplugin3]\nallow-process = [\"true\"]\n";
    let (mut app, _dir) = app_with_v3_settings(WITH_PROCESS, settings);
    keys(
        &mut app,
        ":plugin-run testplugin3/interactive-twice true<ret>",
    )
    .await;
    let log = helix_term::plugins::log_lines("testplugin3")?;
    let ids: Vec<u32> = log
        .last()
        .and_then(|line| line.strip_prefix("twice:"))
        .expect("ids logged")
        .split(':')
        .map(|id| id.parse().unwrap())
        .collect();
    let open = |id| {
        helix_term::plugins::with_host(|host| {
            host.has_request("testplugin3", id, helix_plugin::PendingKind::Process)
        })
        .unwrap()
    };
    // Only one program can be queued: the second replaced the first. The
    // queued one is still open; drop it (as quitting would).
    assert!(open(ids[1]), "the queued request stays open until it runs");
    assert!(app.drop_queued_program());
    let message = wait_for_status(&mut app, |message| message.starts_with("error:")).await;
    assert!(
        message.contains("cancelled before it could run"),
        "{message}"
    );
    // Both were answered, so neither request stays open forever.
    assert!(!open(ids[0]), "the displaced request must be completed");
    assert!(!open(ids[1]), "the dropped request must be completed");
    assert!(!app.drop_queued_program());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_decorate_documents() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[a|]#\n")
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("todos");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/todos").join(file),
            plugin.join(file),
        )?;
    }
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    let signs = |app: &Application| {
        let doc = helix_view::doc!(app.editor);
        doc.plugin_decorations
            .signs_by_line(doc.text().slice(..))
            .into_iter()
            .map(|(line, (text, _))| (line, text.to_string()))
            .collect::<Vec<_>>()
    };
    assert!(helix_view::doc!(app.editor).plugin_decorations.is_empty());

    // Typing a TODO: the change event decorates the line.
    keys(&mut app, "i// TODO x<esc>").await;
    deliver_events(&mut app).await;
    assert_eq!(signs(&app), [(0, "●".to_string())]);
    assert_eq!(
        app.editor
            .plugin_statusline
            .get("todos/count")
            .map(String::as_str),
        Some("1 todo")
    );

    // A new line above moves the sign along with the text right away.
    keys(&mut app, "ggOnew<esc>").await;
    assert_eq!(signs(&app), [(1, "●".to_string())]);
    deliver_events(&mut app).await;
    assert_eq!(signs(&app), [(1, "●".to_string())]);

    // Removing it clears the decorations and the statusline segment.
    keys(&mut app, "%d").await;
    deliver_events(&mut app).await;
    assert!(signs(&app).is_empty());
    assert!(app.editor.plugin_statusline.is_empty());

    // Reloading drops what a plugin left behind.
    keys(&mut app, "i// FIXME y<esc>").await;
    deliver_events(&mut app).await;
    assert!(!helix_view::doc!(app.editor).plugin_decorations.is_empty());
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[todos]\nenabled = false\n",
    )?;
    assert!(helix_view::doc!(app.editor).plugin_decorations.is_empty());
    assert!(app.editor.plugin_statusline.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_keys_only_fill_free_keys() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[a|]#\n")
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("todos");
    std::fs::create_dir(&plugin)?;
    std::fs::copy(
        helix_plugin_dir().join("examples/todos/plugin.wasm"),
        plugin.join("plugin.wasm"),
    )?;
    let manifest = std::fs::read_to_string(helix_plugin_dir().join("examples/todos/plugin.toml"))?;
    // `space h` is already bound (select references): the plugin can't take it.
    std::fs::write(
        plugin.join("plugin.toml"),
        format!("{manifest}h = \"list\"\n"),
    )?;
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;

    // A free key: the plugin's command runs.
    keys(&mut app, "<space>T").await;
    assert_eq!(status(&app).0, "No TODO, FIXME or NOTE here");
    app.editor.set_status("");
    keys(&mut app, "<space>h").await;
    assert_ne!(
        app.editor
            .get_status()
            .map(|(s, _)| s.to_string())
            .as_deref(),
        Some("No TODO, FIXME or NOTE here"),
        "space h must keep its own command"
    );
    assert!(helix_term::plugins::describe().contains("`space T` list"));

    // `keys = false`: none of its keys.
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[todos]\nkeys = false\n",
    )?;
    app.editor.set_status("");
    keys(&mut app, "<space>T").await;
    assert_ne!(
        app.editor
            .get_status()
            .map(|(s, _)| s.to_string())
            .as_deref(),
        Some("No TODO, FIXME or NOTE here")
    );

    // Keys must name the plugin's own commands.
    std::fs::write(
        plugin.join("plugin.toml"),
        format!("{manifest}x = \"nope\"\n"),
    )?;
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    assert_error(&app, "not one of its commands");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_commands_run_as_colon_commands() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[a|]#", ALL, "");
    // `:plugin/command args` at the prompt, like `:plugin-run`.
    keys(&mut app, ":text-tools/upper<ret>").await;
    assert_eq!(text(&app), "A");
    // And as a key binding.
    let command: helix_term::commands::MappableCommand = ":text-tools/lower".parse()?;
    assert_eq!(command.name(), "plugin-run");
    keys(&mut app, ":no-such/plugin<ret>").await;
    assert_error(&app, "no plugin named 'no-such'");
    Ok(())
}
