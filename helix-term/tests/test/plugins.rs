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
        helix_plugin_dir().join("tests/fixtures/testplugin2.wasm"),
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
        helix_plugin_dir().join("tests/fixtures/testplugin2.wasm"),
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
