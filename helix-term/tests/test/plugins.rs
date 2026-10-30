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
async fn plugins_run_commands_that_open_ui() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_plugins("#[h|]#ello\nworld", ALL, "");
    // A command waiting for the next key (`r`): the key goes to it.
    keys(&mut app, ":plugin-run testplugin/run replace<ret>").await;
    keys(&mut app, "j").await;
    assert_eq!(text(&app), "jello\nworld");
    // A command opening a prompt (`/`): it shows and takes the input.
    keys(&mut app, ":plugin-run testplugin/run search<ret>").await;
    keys(&mut app, "wor<ret>").await;
    assert_eq!(primary(&app), (6, 9));
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
/// Run the editor until `found` finds something (or give up).
async fn wait_for<T>(app: &mut Application, found: impl Fn(&Application) -> Option<T>) -> T {
    for _ in 0..100 {
        if let Some(value) = found(app) {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        app.editor.reset_idle_timer();
        run_event_loop_until_idle(app).await;
    }
    panic!("timed out; status: {:?}", app.editor.get_status());
}

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
        format!("name = \"testplugin3\"\nversion = \"1\"\napi = \"0.5\"\npermissions = [{permissions}]\n"),
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn competitest_example_judges_testcases_in_a_bottom_panel() -> anyhow::Result<()> {
    if std::process::Command::new("g++")
        .arg("--version")
        .output()
        .is_err()
    {
        return Ok(()); // no compiler here
    }
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    let source = work.join("sol.cpp");
    std::fs::write(
        &source,
        "#include <iostream>\nint main(){long a,b;std::cin>>a>>b;if(a<0)for(;;);std::cout<<a+b<<\"\\n\";}\n",
    )?;
    for (n, input, output) in [(0, "1 2", "3"), (1, "2 2", "5"), (2, "-1 0", "0")] {
        std::fs::write(work.join(format!("sol_input{n}.txt")), input)?;
        std::fs::write(work.join(format!("sol_output{n}.txt")), output)?;
    }
    let mut app = AppBuilder::new().with_file(&source, None).build().unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("competitest");
    std::fs::create_dir(&plugin)?;
    for name in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/competitest").join(name),
            plugin.join(name),
        )?;
    }
    let settings = format!(
        "[competitest]\nallow-process = true\nallow-files = \"write\"\nallow-dirs = [{:?}]\ntimeout = 700\n",
        work.display().to_string()
    );
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), &settings)?;
    keys(&mut app, ":plugin-run competitest/run<ret>").await;
    let mut status = String::new();
    for _ in 0..10 {
        status = wait_for_status(&mut app, |s| s.contains("passed") || s.contains("failed")).await;
        if status.contains("passed") || status.contains("failed") {
            break;
        }
    }
    assert_eq!(status, "CompetiTest: 1/3 passed");
    let panel = app
        .editor
        .panel(helix_view::sidebar::Dock::Bottom)
        .expect("a bottom panel");
    assert_eq!(panel.title, "CompetiTest: sol.cpp  1/3 passed");
    let rows: Vec<&str> = panel.items.iter().map(|i| i.text.as_str()).collect();
    assert!(
        rows[0].starts_with("TC 0") && rows[0].contains("accepted"),
        "{rows:?}"
    );
    assert!(rows[1].contains("wrong answer"), "{rows:?}");
    assert!(rows[2].contains("time limit"), "{rows:?}");
    // The panel takes the keys; enter on a testcase shows what it got.
    keys(&mut app, ":panel-focus bottom<ret>j<ret>").await;
    deliver_events(&mut app).await;
    let shown = text(&app);
    assert!(shown.starts_with("Testcase 1: ✘  wrong answer"), "{shown}");
    assert!(
        shown.contains("── Expected ──\n5\n── Got ──\n4\n"),
        "{shown}"
    );
    Ok(())
}

/// A port nothing listens on.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// POST `body` to 127.0.0.1:`port` like Competitive Companion (with
/// `content_type`); the response's status line.
fn post(port: u16, content_type: &str, body: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "POST / HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response.lines().next().unwrap_or_default().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn competitest_example_receives_testcases_from_competitive_companion() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    let source = work.join("sol.cpp");
    std::fs::write(&source, "int main() {}\n")?;
    for n in 0..3 {
        std::fs::write(work.join(format!("sol_input{n}.txt")), "old")?;
        std::fs::write(work.join(format!("sol_output{n}.txt")), "old")?;
    }
    let port = free_port();
    let settings = format!(
        "[competitest]\nallow-files = \"write\"\nallow-dirs = [{:?}]\nallow-listen = [{port}]\ncompanion-port = {port}\n",
        work.display().to_string()
    );
    let (mut app, _dir) = app_with_example_on("competitest", &source, &settings);
    keys(&mut app, ":plugin-run competitest/receive<ret>").await;
    assert!(
        status(&app)
            .0
            .starts_with("CompetiTest: waiting for testcases"),
        "{:?}",
        status(&app)
    );
    let listening = move || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok();
    wait_for(&mut app, |_| listening().then_some(())).await;
    let problem = r#"{"name": "A. Watermelon", "group": "Codeforces", "url": "https://codeforces.com/problemset/problem/4/A",
        "memoryLimit": 64, "timeLimit": 1000, "tests": [{"input": "8\n", "output": "YES\n"}, {"input": "5\n", "output": "NO\n"}],
        "batch": {"id": "b", "size": 1}}"#;
    let sent = std::thread::spawn(move || {
        // A web page can't send these; they never reach the plugin.
        let refused = post(port, "text/plain", problem);
        (refused, post(port, "application/json", problem))
    });
    let message = wait_for_status(&mut app, |s| s.contains("testcases of")).await;
    let (refused, accepted) = sent.join().unwrap();
    assert_eq!(refused, "HTTP/1.1 415 Unsupported Media Type");
    assert_eq!(accepted, "HTTP/1.1 200 OK");
    assert_eq!(
        message,
        "CompetiTest: 2 testcases of \"A. Watermelon\" for sol.cpp"
    );
    assert_eq!(std::fs::read_to_string(work.join("sol_input0.txt"))?, "8\n");
    assert_eq!(
        std::fs::read_to_string(work.join("sol_output1.txt"))?,
        "NO\n"
    );
    assert!(!work.join("sol_input2.txt").exists());
    // One problem was asked for: it stops listening.
    wait_for(&mut app, |_| (!listening()).then_some(())).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn competitest_example_needs_the_port_granted() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let source = helix_stdx::path::canonicalize(root.path()).join("sol.cpp");
    std::fs::write(&source, "int main() {}\n")?;
    let (mut app, _dir) = app_with_example_on("competitest", &source, "");
    keys(&mut app, ":plugin-run competitest/receive<ret>").await;
    let (message, severity) = status(&app);
    assert_eq!(severity, Severity::Error);
    assert!(
        message.contains("port 27121 is not in allow-listen for [competitest]"),
        "{message}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn competitest_example_reports_a_taken_port() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let source = helix_stdx::path::canonicalize(root.path()).join("sol.cpp");
    std::fs::write(&source, "int main() {}\n")?;
    let taken = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = taken.local_addr()?.port();
    let settings = format!("[competitest]\nallow-listen = [{port}]\ncompanion-port = {port}\n");
    let (mut app, _dir) = app_with_example_on("competitest", &source, &settings);
    keys(&mut app, ":plugin-run competitest/receive<ret>").await;
    let message = wait_for_status(&mut app, |s| s.contains("can't listen")).await;
    assert!(
        message.starts_with(&format!("CompetiTest: can't listen on 127.0.0.1:{port}")),
        "{message}"
    );
    assert_eq!(status(&app).1, Severity::Error);
    drop(taken);
    Ok(())
}

/// An editor on `file` with example plugin `name` loaded with `settings`.
fn app_with_example_on(
    name: &str,
    file: &Path,
    settings: &str,
) -> (Application, tempfile::TempDir) {
    let mut app = AppBuilder::new().with_file(file, None).build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let plugin = dir.path().join(name);
    std::fs::create_dir(&plugin).unwrap();
    for part in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples").join(name).join(part),
            plugin.join(part),
        )
        .unwrap();
    }
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings).unwrap();
    (app, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn outline_example_lists_symbols_in_a_right_panel() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let file = root.path().join("lib.rs");
    std::fs::write(
        &file,
        "struct A;\n\nimpl A {\n    fn go(&self) {}\n}\n\nfn main() {}\n\nfn\nsplit() {}\n",
    )?;
    let (mut app, _dir) = app_with_example_on("outline", &file, "");
    keys(&mut app, ":outline/toggle<ret>").await;
    deliver_events(&mut app).await;
    let panel = app
        .editor
        .panel(helix_view::sidebar::Dock::Right)
        .expect("a right panel");
    let rows: Vec<(&str, u16)> = panel
        .items
        .iter()
        .map(|i| (i.text.as_str(), i.depth))
        .collect();
    assert_eq!(
        rows,
        // From the syntax tree: a line by line scan misses `split`.
        [
            ("struct A", 0),
            ("impl A", 0),
            ("fn go", 1),
            ("fn main", 0),
            ("fn split", 0)
        ]
    );
    assert_eq!(panel.title, "Outline: lib.rs");
    // It follows the cursor...
    keys(&mut app, "3j").await;
    deliver_events(&mut app).await;
    let panel = app.editor.panel(helix_view::sidebar::Dock::Right).unwrap();
    assert_eq!(panel.selected, 2, "on fn go");
    // ...and enter jumps.
    keys(&mut app, ":outline/focus<ret>Gk<ret>").await;
    deliver_events(&mut app).await;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(doc.text().char_to_line(cursor), 6);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn harpoon_example_marks_and_jumps() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let (a, b) = (root.path().join("a.txt"), root.path().join("b.txt"));
    std::fs::write(&a, "one\ntwo\nthree\n")?;
    std::fs::write(&b, "bee\n")?;
    let (mut app, _dir) = app_with_example_on("harpoon", &a, "");
    // Marks are kept per project in the plugin's data directory.
    keys(&mut app, ":harpoon/clear<ret>").await;
    keys(&mut app, "jj:harpoon/add<ret>").await;
    assert!(status(&app).0.ends_with("is mark 1"), "{:?}", status(&app));
    keys(
        &mut app,
        &format!(":open {}<ret>:harpoon/add<ret>", b.display()),
    )
    .await;
    assert!(status(&app).0.ends_with("is mark 2"), "{:?}", status(&app));
    // space 1 goes back to a.txt at its line.
    keys(&mut app, "<space>1").await;
    let (view, doc) = helix_view::current_ref!(app.editor);
    assert_eq!(
        doc.path(),
        Some(helix_stdx::path::canonicalize(&a).as_path())
    );
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(doc.text().char_to_line(cursor), 2);
    // The menu lists them; deleting a line and :w forgets that mark.
    keys(&mut app, ":harpoon/menu<ret>").await;
    deliver_events(&mut app).await;
    let menu = text(&app);
    assert!(
        menu.ends_with("a.txt\n") || menu.contains("a.txt\n"),
        "{menu}"
    );
    assert_eq!(menu.lines().count(), 2, "{menu}");
    keys(&mut app, "xd:w<ret>").await;
    deliver_events(&mut app).await;
    assert_eq!(status(&app).0, "harpoon: 1 marks");
    keys(&mut app, ":buffer-close!<ret><space>2").await;
    assert!(status(&app).0.contains("no mark 2"), "{:?}", status(&app));
    keys(&mut app, ":harpoon/clear<ret>").await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn illuminate_example_highlights_uses() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let file = root.path().join("x.txt");
    std::fs::write(&file, "count = count + 1\ncounter = count\n")?;
    let (mut app, _dir) = app_with_example_on("illuminate", &file, "");
    keys(&mut app, "l").await;
    deliver_events(&mut app).await;
    let doc = helix_view::doc!(app.editor);
    let ranges = doc.plugin_decorations.highlight_ranges("illuminate/uses");
    assert_eq!(ranges, [0..5, 8..13, 28..33]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn git_blame_example_annotates_the_cursor_line() -> anyhow::Result<()> {
    let git = |dir: &Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
    };
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    if git(&work, &["init", "-q"]).is_err() {
        return Ok(()); // no git here
    }
    std::fs::write(work.join("f.txt"), "first\nsecond\n")?;
    git(&work, &["add", "f.txt"])?;
    git(
        &work,
        &[
            "-c",
            "user.name=Ana Lima",
            "-c",
            "user.email=a@x",
            "commit",
            "-qm",
            "Add f",
        ],
    )?;
    let (mut app, _dir) = app_with_example_on(
        "git-blame",
        &work.join("f.txt"),
        "[git-blame]\nallow-process = [\"git\"]\n",
    );
    // A cursor move asks for the blame (the file opened before the plugin).
    keys(&mut app, "j").await;
    deliver_events(&mut app).await;
    let label = wait_for(&mut app, |app| {
        helix_view::doc!(app.editor)
            .plugin_decorations
            .annotation_texts("git-blame/blame")
            .first()
            .cloned()
    })
    .await;
    assert!(
        label.trim_start().starts_with("Ana Lima, just now • Add f"),
        "{label:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn api_demo_sorts_in_the_background_at_a_revision() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_example("api-demo", "");
    set_text(&mut app, "pear\napple\nfig\n");
    keys(&mut app, ":plugin-run api-demo/sort<ret>").await;
    let status = wait_for_status(&mut app, |s| s == "sorted" || s.contains("changed")).await;
    assert_eq!(status, "sorted");
    assert_eq!(text(&app), "apple\nfig\npear\n");
    Ok(())
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
async fn markdown_example_leaves_the_cursor_line_to_the_editor() -> anyhow::Result<()> {
    let file = tempfile::Builder::new().suffix(".md").tempfile()?;
    std::fs::write(file.path(), "# Title\n- item\n```\ncode\n```\n")?;
    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("markdown");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/markdown").join(file),
            plugin.join(file),
        )?;
    }
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    keys(&mut app, "ia<esc>u").await;
    deliver_events(&mut app).await;
    let decorations = &helix_view::doc!(app.editor).plugin_decorations;
    assert!(!decorations.is_empty());
    // The editor hides the rendering on the cursor's line: no cursor events.
    assert!(decorations.wants_cursor_line());
    // Code blocks get full-line backgrounds.
    let doc = helix_view::doc!(app.editor);
    let lines: Vec<_> = decorations
        .line_backgrounds(doc.text().slice(..), &app.editor.theme, 0..10, None)
        .into_keys()
        .collect();
    assert_eq!(lines, [2, 3, 4]);
    Ok(())
}

/// Line `line` of the focused document as drawn: with the view's
/// annotations (conceals, overlays, virtual text) applied.
fn shown_line(app: &Application, line: usize) -> String {
    use helix_core::doc_formatter::{DocumentFormatter, TextFormat};
    let (view, doc) = helix_view::current_ref!(app.editor);
    let annotations = view.text_annotations(doc, Some(&app.editor.theme));
    let text = doc.text().slice(..);
    let start = text.line_to_char(line);
    let format = TextFormat {
        viewport_width: 200,
        ..TextFormat::default()
    };
    let mut shown = String::new();
    for grapheme in DocumentFormatter::new_at_prev_checkpoint(text, &format, &annotations, start) {
        if grapheme.line_idx != line || grapheme.raw == helix_core::graphemes::Grapheme::Newline {
            break;
        }
        shown.push_str(&grapheme.raw.to_string());
    }
    shown
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_example_conceals_markup_like_render_markdown() -> anyhow::Result<()> {
    use helix_view::document::Mode;
    let file = tempfile::Builder::new().suffix(".md").tempfile()?;
    std::fs::write(
        file.path(),
        "a **b** `c`\nsee [docs](https://x.y/z) now\n- *it*\n",
    )?;
    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("markdown");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/markdown").join(file),
            plugin.join(file),
        )?;
    }
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    keys(&mut app, "ia<esc>u").await;
    deliver_events(&mut app).await;
    // What the renderer does each frame.
    let set_mode = |app: &Application, mode| {
        helix_view::doc!(app.editor)
            .plugin_decorations
            .set_render_mode(mode)
    };
    set_mode(&app, Mode::Normal);

    // The primary cursor's line is raw; the others are rendered.
    assert_eq!(shown_line(&app, 0), "a **b** `c`");
    assert_eq!(shown_line(&app, 1), "see docs now");
    assert_eq!(shown_line(&app, 2), "• it");

    // Moving the cursor moves the raw line; the plugin is not involved.
    keys(&mut app, "j").await;
    assert_eq!(shown_line(&app, 0), "a b c");
    assert_eq!(shown_line(&app, 1), "see [docs](https://x.y/z) now");

    // Other selections don't reveal their lines: `C` adds a selection on
    // line 2 and makes it primary, so line 1 is rendered again.
    keys(&mut app, "C").await;
    assert_eq!(
        helix_view::doc!(app.editor)
            .selection(app.editor.tree.focus)
            .len(),
        2
    );
    assert_eq!(shown_line(&app, 1), "see docs now");
    assert_eq!(shown_line(&app, 2), "- *it*");
    keys(&mut app, ",").await;

    // Nothing is rendered while inserting or selecting.
    set_mode(&app, Mode::Insert);
    assert_eq!(shown_line(&app, 0), "a **b** `c`");
    assert_eq!(shown_line(&app, 2), "- *it*");
    set_mode(&app, Mode::Select);
    assert_eq!(shown_line(&app, 0), "a **b** `c`");
    set_mode(&app, Mode::Normal);
    assert_eq!(shown_line(&app, 0), "a b c");

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

#[tokio::test(flavor = "multi_thread")]
async fn background_tasks_deliver_without_blocking_the_editor() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3_settings(ALL, "");
    keys(&mut app, ":testplugin3/background square 7<ret>").await;
    assert_eq!(
        wait_for_status(&mut app, |s| s.starts_with("background")).await,
        "background:49"
    );
    // The editor goes on as usual (a key press clears the status line).
    keys(&mut app, "ix<esc>").await;
    assert_eq!(text(&app), "xa\n");
    Ok(())
}

fn app_with_v3_manifest(
    permissions: &str,
    extra: &str,
    settings: &str,
) -> (Application, tempfile::TempDir) {
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
        format!("name = \"testplugin3\"\nversion = \"1\"\napi = \"0.5\"\npermissions = [{permissions}]\n{extra}"),
    )
    .unwrap();
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings).unwrap();
    (app, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_diagnostics_show_like_a_language_servers() -> anyhow::Result<()> {
    let (mut app, dir) = app_with_v3_settings(ALL, "");
    keys(&mut app, ":plugin-run testplugin3/diagnose<ret>").await;
    let doc = helix_view::doc!(app.editor);
    let diagnostics = doc.diagnostics();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].message, "bad");
    assert_eq!(diagnostics[0].range.start, 0);
    assert_eq!(diagnostics[0].range.end, 1);
    assert_eq!(diagnostics[0].source.as_deref(), Some("testplugin3"));
    assert!(matches!(
        &diagnostics[0].provider,
        helix_core::diagnostic::DiagnosticProvider::Plugin { name } if &**name == "testplugin3"
    ));
    // Reloading drops them.
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    assert!(helix_view::doc!(app.editor).diagnostics().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_timers_fire_later() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3_settings(ALL, "");
    keys(&mut app, ":plugin-run testplugin3/timer 20<ret>").await;
    let message = wait_for_status(&mut app, |message| message.starts_with("timer-fired:")).await;
    assert!(message.starts_with("timer-fired:"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_timers_stop_on_reload() -> anyhow::Result<()> {
    let (mut app, dir) = app_with_v3_settings(ALL, "");
    let before = helix_term::plugins::running_timers();
    keys(&mut app, ":plugin-run testplugin3/timer 60000<ret>").await;
    assert_eq!(helix_term::plugins::running_timers(), before + 1);
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    assert_eq!(helix_term::plugins::running_timers(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_plugins_leave_nothing_behind() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3_settings(ALL, "");
    keys(&mut app, ":plugin-run testplugin3/diagnose<ret>").await;
    keys(&mut app, ":plugin-run testplugin3/timer 60000<ret>").await;
    assert_eq!(helix_view::doc!(app.editor).diagnostics().len(), 1);
    // It runs too long and is disabled: its diagnostics and timers go.
    keys(&mut app, ":plugin-run testplugin3/loop<ret>").await;
    assert_error(&app, "disabled");
    assert!(helix_view::doc!(app.editor).diagnostics().is_empty());
    assert_eq!(helix_term::plugins::running_timers(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_buffers_are_written_by_their_plugin() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3_settings(ALL, "");
    keys(&mut app, ":plugin-run testplugin3/buffer<ret>").await;
    deliver_events(&mut app).await;
    let doc = helix_view::doc!(app.editor);
    assert_eq!(doc.display_name(), "test://buffer");
    assert_eq!(doc.plugin_owner.as_deref(), Some("testplugin3"));
    assert_eq!(doc.text().to_string(), "one\n");
    assert!(!doc.is_modified());
    let log = helix_term::plugins::log_lines("testplugin3")?;
    assert!(
        log.iter()
            .any(|line| line.starts_with("buffer-open:test://buffer:")),
        "{log:?}"
    );
    keys(&mut app, "itwo<esc>").await;
    assert!(helix_view::doc!(app.editor).is_modified());
    keys(&mut app, ":w<ret>").await;
    assert_eq!(status(&app).0, "written:test://buffer:twoone");
    assert!(!helix_view::doc!(app.editor).is_modified());

    // Opening it again shows the same buffer.
    let id = helix_view::doc!(app.editor).id();
    keys(
        &mut app,
        ":buffer-next<ret>:plugin-run testplugin3/buffer<ret>",
    )
    .await;
    assert_eq!(helix_view::doc!(app.editor).id(), id);

    // Scratch buffers are ordinary ones.
    keys(&mut app, ":plugin-run testplugin3/scratch<ret>").await;
    let doc = helix_view::doc!(app.editor);
    assert_eq!(doc.text().to_string(), "scratch text\n");
    assert!(doc.plugin_owner.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_buffer_keys_only_apply_in_its_buffers() -> anyhow::Result<()> {
    let permissions = format!("{ALL}, \"registers\"");
    let (mut app, _dir) = app_with_v3_manifest(
        &permissions,
        "[keys.buffer.normal]\nx = \"registers\"\n",
        "",
    );
    // Elsewhere `x` keeps its binding (select the line).
    keys(&mut app, "x").await;
    assert_eq!(primary(&app), (0, 2));
    assert!(app
        .editor
        .get_status()
        .is_none_or(|(message, _)| !message.starts_with("registers")));

    keys(&mut app, ":plugin-run testplugin3/buffer<ret>").await;
    keys(&mut app, "x").await;
    assert_eq!(status(&app).0, "registers:x,y");
    // The register was written in the editor.
    let values: Vec<String> = app
        .editor
        .registers
        .read('a', &app.editor)
        .unwrap()
        .map(|value| value.into_owned())
        .collect();
    assert_eq!(values, ["x", "y"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_can_be_completion_sources() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3_settings(ALL, "[testplugin3]\ncompletion = true\n");
    // Typing asks the completion sources; ctrl-n takes the first item.
    keys(&mut app, "ial").await;
    deliver_events(&mut app).await;
    keys(&mut app, "<C-n><esc>").await;
    let text = text(&app);
    assert!(text.starts_with("alpha"), "{text:?}");
    let log = helix_term::plugins::log_lines("testplugin3")?;
    assert!(
        log.iter().any(|line| line.starts_with("completion:")),
        "{log:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn one_ctrl_c_leaves_insert_mode_with_the_completion_menu_open() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3_settings(ALL, "[testplugin3]\ncompletion = true\n");
    keys(&mut app, "ial").await;
    deliver_events(&mut app).await;
    // The menu previews `alpha`; ctrl-c drops the preview and, in the same
    // press, leaves insert mode.
    keys(&mut app, "<C-n>").await;
    assert!(text(&app).starts_with("alpha"), "{:?}", text(&app));
    keys(&mut app, "<C-c>").await;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    assert!(
        text(&app).starts_with("al") && !text(&app).starts_with("alpha"),
        "{:?}",
        text(&app)
    );
    // Without a menu it leaves insert mode too.
    keys(&mut app, "ix<C-c>").await;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_snippets_have_tab_stops() -> anyhow::Result<()> {
    let (mut app, _dir) =
        app_with_v3_settings(ALL, "[testplugin3]\ncompletion = true\nsnippets = true\n");
    keys(&mut app, "ifn").await;
    deliver_events(&mut app).await;
    // Accept `fn ${1:name}() {$0}`: `name` is selected, tab goes to `$0`.
    keys(&mut app, "<C-n><ret>").await;
    keys(&mut app, "foo<tab>x<esc>").await;
    let text = text(&app);
    assert!(text.starts_with("fn foo() {x}"), "{text:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn snippets_example_expands_with_tab_stops() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let file = root.path().join("main.rs");
    std::fs::write(&file, "")?;
    let mut app = AppBuilder::new().with_file(&file, None).build().unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("snippets");
    std::fs::create_dir(&plugin)?;
    for name in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/snippets").join(name),
            plugin.join(name),
        )?;
    }
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    keys(&mut app, "itest").await;
    deliver_events(&mut app).await;
    keys(&mut app, "<C-n><ret>").await;
    // `#[test]\nfn ${1:name}() {\n    $0\n}`
    keys(&mut app, "works<tab>assert!(true);<esc>").await;
    assert_eq!(text(&app), "#[test]\nfn works() {\n    assert!(true);\n}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_for_an_old_revision_are_refused() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_v3_settings(ALL, "");
    keys(&mut app, ":plugin-run testplugin3/rev<ret>").await;
    let (message, _) = status(&app);
    let after: u64 = message.rsplit(':').next().unwrap().parse()?;
    // The text changes: an edit computed for `after` no longer applies...
    keys(&mut app, "itext<esc>").await;
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/edit-at {after}<ret>"),
    )
    .await;
    let (message, _) = status(&app);
    assert!(message.starts_with("edit-at:err:"), "{message}");
    assert!(!text(&app).contains('@'));
    // ...one for the current revision does.
    let current = helix_view::doc!(app.editor).version();
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/edit-at {current}<ret>"),
    )
    .await;
    assert_eq!(status(&app).0, "edit-at:ok");
    assert!(text(&app).starts_with('@'));
    Ok(())
}

/// Replace the whole text of the current document, as if typed.
fn set_text(app: &mut Application, text: &str) {
    let (view, doc) = helix_view::current!(app.editor);
    let transaction = helix_core::Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
    );
    doc.apply(&transaction, view.id);
}

#[tokio::test(flavor = "multi_thread")]
async fn oil_example_edits_directories_as_buffers() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    std::fs::write(work.join("a.txt"), "a")?;
    std::fs::write(work.join("b.txt"), "b")?;
    std::fs::create_dir(work.join("sub"))?;
    std::fs::write(work.join("sub/inner.txt"), "inner")?;

    let mut app = AppBuilder::new()
        .with_file(work.join("a.txt"), None)
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("oil");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/oil").join(file),
            plugin.join(file),
        )?;
    }
    let settings = format!(
        "[oil]\nallow-files = \"write\"\nallow-dirs = [{:?}]\nconfirm = false\n",
        work.display().to_string()
    );
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), &settings)?;
    let a_txt = helix_view::doc!(app.editor).id();

    // `-` opens the file's directory, cursor on the file.
    keys(&mut app, "-").await;
    deliver_events(&mut app).await;
    let name = helix_view::doc!(app.editor).display_name().to_string();
    assert_eq!(name, format!("oil://{}/", work.display()));
    let listing = text(&app);
    let lines: Vec<&str> = listing.lines().collect();
    assert_eq!(lines.len(), 3, "{listing}");
    assert!(lines[0].ends_with(" sub/"), "{listing}");
    assert!(
        lines[1].ends_with(" a.txt") && lines[2].ends_with(" b.txt"),
        "{listing}"
    );
    let (view, doc) = helix_view::current_ref!(app.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(doc.text().char_to_line(cursor), 1, "cursor on a.txt");
    assert!(
        !doc.plugin_decorations.is_empty(),
        "icons and names are drawn"
    );

    // Rename a.txt, delete b.txt, create new.txt and a directory.
    let renamed = listing
        .lines()
        .map(|line| line.replace(" a.txt", " c.txt"))
        .filter(|line| !line.ends_with(" b.txt"))
        .chain(["new.txt".to_string(), "made/".to_string()])
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    set_text(&mut app, &renamed);
    assert!(helix_view::doc!(app.editor).is_modified());
    keys(&mut app, ":w<ret>").await;
    // The changes, then the refresh a moment later (a timer).
    deliver_events(&mut app).await;
    deliver_events(&mut app).await;
    assert!(!work.join("a.txt").exists());
    assert_eq!(std::fs::read_to_string(work.join("c.txt"))?, "a");
    assert!(!work.join("b.txt").exists());
    assert!(work.join("new.txt").is_file());
    assert!(work.join("made").is_dir());
    // The buffer of a.txt followed the rename.
    assert_eq!(
        app.editor.documents[&a_txt].path().map(|p| p.to_path_buf()),
        Some(work.join("c.txt"))
    );
    // The listing was read again and is saved.
    let doc = helix_view::doc!(app.editor);
    assert!(!doc.is_modified());
    let listing = text(&app);
    assert!(
        listing.contains(" made/") && listing.contains(" new.txt"),
        "{listing}"
    );

    // enter opens a directory, `-` goes back up.
    keys(&mut app, "gg<ret>").await;
    deliver_events(&mut app).await;
    let name = helix_view::doc!(app.editor).display_name().to_string();
    assert_eq!(name, format!("oil://{}/", work.join("made").display()));
    keys(&mut app, "-").await;
    deliver_events(&mut app).await;
    let name = helix_view::doc!(app.editor).display_name().to_string();
    assert_eq!(name, format!("oil://{}/", work.display()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn directory_handler_opens_directories_with_a_plugin() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    std::fs::write(work.join("a.txt"), "a")?;
    let mut config = test_config();
    config.editor.directory_handler = Some("oil/open".into());
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(work.join("a.txt"), None)
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("oil");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/oil").join(file),
            plugin.join(file),
        )?;
    }
    let settings = format!(
        "[oil]\nallow-files = \"write\"\nallow-dirs = [{:?}]\n",
        work.display().to_string()
    );
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), &settings)?;
    keys(&mut app, &format!(":open {}<ret>", work.display())).await;
    deliver_events(&mut app).await;
    let name = helix_view::doc!(app.editor).display_name().to_string();
    assert_eq!(name, format!("oil://{}/", work.display()));
    assert!(text(&app).contains(" a.txt"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reload_during_the_startup_load_opens_the_waiting_directory() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    std::fs::write(work.join("a.txt"), "a")?;
    let mut config = test_config();
    config.editor.directory_handler = Some("oil/open".into());
    let mut app = AppBuilder::new().with_config(config).build().unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("oil");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/oil").join(file),
            plugin.join(file),
        )?;
    }
    // `pure <dir>` while plugins load in the background: the directory waits.
    helix_term::plugins::start_loading_for_tests();
    assert!(helix_term::plugins::open_directory(
        &app.editor,
        work.clone()
    ));
    // A manual reload replaces that load (whose result is then dropped).
    let settings = format!(
        "[oil]\nallow-files = \"write\"\nallow-dirs = [{:?}]\n",
        work.display().to_string()
    );
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), &settings)?;
    keys(&mut app, "").await;
    deliver_events(&mut app).await;
    let name = helix_view::doc!(app.editor).display_name().to_string();
    assert_eq!(name, format!("oil://{}/", work.display()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_operations_report_results_in_order() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    let settings = format!(
        "[testplugin3]\nallow-files = \"write\"\nallow-dirs = [{:?}]\n",
        work.display().to_string()
    );
    let (mut app, _dir) = app_with_v3_settings(&format!("{ALL}, \"filesystem\""), &settings);
    let p = |name: &str| work.join(name).display().to_string();
    let ops = format!(
        "create:{} rename:{}:{} copy:{}:{} trash:{}",
        p("a"),
        p("a"),
        p("b"),
        p("b"),
        p("c"),
        p("c")
    );
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/files {ops}<ret>"),
    )
    .await;
    let status = wait_for_status(&mut app, |s| s.starts_with("files-done:")).await;
    assert_eq!(status, "files-done:ok;ok;ok;ok");
    assert!(!work.join("a").exists());
    assert!(work.join("b").is_file());
    assert!(!work.join("c").exists(), "trashed, not left behind");

    // A failure stops the rest; nothing after it happens.
    app.editor.clear_status();
    let ops = format!("rename:{}:{} delete:{}", p("missing"), p("x"), p("b"));
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/files {ops}<ret>"),
    )
    .await;
    let status = wait_for_status(&mut app, |s| s.starts_with("files-done:")).await;
    assert!(
        status.starts_with("files-done:err(Failed to move"),
        "{status}"
    );
    assert!(
        status.ends_with(";err(skipped after an earlier failure)"),
        "{status}"
    );
    assert!(
        work.join("b").is_file(),
        "the delete after the failure didn't run"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn file_operations_never_replace_or_follow_into_existing_files() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    let outside = tempfile::tempdir()?;
    let target = outside.path().join("target.txt");
    std::fs::write(&target, "keep")?;
    std::os::unix::fs::symlink(&target, work.join("link"))?;
    std::fs::write(work.join("x"), "x")?;
    std::fs::write(work.join("y"), "y")?;
    assert!(std::process::Command::new("mkfifo")
        .arg(work.join("fifo"))
        .status()?
        .success());
    let settings = format!(
        "[testplugin3]\nallow-files = \"write\"\nallow-dirs = [{:?}]\n",
        work.display().to_string()
    );
    let (mut app, _dir) = app_with_v3_settings(&format!("{ALL}, \"filesystem\""), &settings);
    let p = |name: &str| work.join(name).display().to_string();
    for (ops, expected) in [
        // A symlink to a file outside: not followed and emptied.
        (format!("create:{}", p("link")), "files-done:err("),
        // An existing destination is never replaced.
        (format!("rename:{}:{}", p("x"), p("y")), "files-done:err("),
        (format!("copy:{}:{}", p("x"), p("y")), "files-done:err("),
        // A FIFO isn't copied (it would block).
        (format!("copy:{}:{}", p("fifo"), p("z")), "files-done:err("),
    ] {
        keys(
            &mut app,
            &format!(":plugin-run testplugin3/files {ops}<ret>"),
        )
        .await;
        let status = wait_for_status(&mut app, |s| s.starts_with("files-done:")).await;
        assert!(status.starts_with(expected), "{ops}: {status}");
        app.editor.clear_status();
    }
    assert_eq!(std::fs::read_to_string(&target)?, "keep");
    assert_eq!(std::fs::read_to_string(work.join("y"))?, "y");
    assert_eq!(std::fs::read_to_string(work.join("x"))?, "x");
    assert!(!work.join("z").exists());

    // The editor's own create (LSP CreateFile with overwrite) still empties
    // a file, but not through a symlink.
    assert!(app.editor.create_path(&work.join("link"), false).is_err());
    assert_eq!(std::fs::read_to_string(&target)?, "keep");
    app.editor.create_path(&work.join("x"), false)?;
    assert_eq!(std::fs::read_to_string(work.join("x"))?, "");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn oil_example_swaps_names_and_fills_new_directories() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    std::fs::write(work.join("a.txt"), "a")?;
    std::fs::write(work.join("b.txt"), "b")?;
    std::fs::write(work.join("c.txt"), "c")?;
    let mut app = AppBuilder::new()
        .with_file(work.join("a.txt"), None)
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("oil");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/oil").join(file),
            plugin.join(file),
        )?;
    }
    let settings = format!(
        "[oil]\nallow-files = \"write\"\nallow-dirs = [{:?}]\nconfirm = false\n",
        work.display().to_string()
    );
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), &settings)?;
    keys(&mut app, "-").await;
    deliver_events(&mut app).await;
    // a.txt ↔ b.txt, and c.txt into a new directory twice over (moved and
    // copied there).
    let listing = text(&app);
    let id = |name: &str| {
        listing
            .lines()
            .find(|line| line.ends_with(&format!(" {name}")))
            .and_then(|line| line.split_whitespace().next())
            .unwrap()
            .to_string()
    };
    let edited = format!(
        "{} b.txt\n{} a.txt\n{} new/c.txt\n{} new/c2.txt\n",
        id("a.txt"),
        id("b.txt"),
        id("c.txt"),
        id("c.txt")
    );
    set_text(&mut app, &edited);
    keys(&mut app, ":w<ret>").await;
    deliver_events(&mut app).await;
    deliver_events(&mut app).await;
    assert_eq!(std::fs::read_to_string(work.join("a.txt"))?, "b");
    assert_eq!(std::fs::read_to_string(work.join("b.txt"))?, "a");
    assert_eq!(std::fs::read_to_string(work.join("new/c.txt"))?, "c");
    assert_eq!(std::fs::read_to_string(work.join("new/c2.txt"))?, "c");
    assert!(!work.join("c.txt").exists());
    assert!(!work.join(".oil-swap-a.txt").exists());
    assert!(
        !helix_view::doc!(app.editor).is_modified(),
        "{:?}",
        app.editor.get_status()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn oil_example_can_delete_to_the_trash() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    std::fs::write(work.join("keep.txt"), "k")?;
    std::fs::write(work.join("old.txt"), "o")?;
    let mut app = AppBuilder::new()
        .with_file(work.join("keep.txt"), None)
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("oil");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/oil").join(file),
            plugin.join(file),
        )?;
    }
    let settings = format!(
        "[oil]\nallow-files = \"write\"\nallow-dirs = [{:?}]\nconfirm = false\ntrash = true\n",
        work.display().to_string()
    );
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), &settings)?;
    keys(&mut app, "-").await;
    deliver_events(&mut app).await;
    let listing = text(&app);
    let kept: String = listing
        .lines()
        .filter(|line| !line.ends_with(" old.txt"))
        .map(|line| format!("{line}\n"))
        .collect();
    set_text(&mut app, &kept);
    keys(&mut app, ":w<ret>").await;
    let status = wait_for_status(&mut app, |s| s.starts_with("oil:")).await;
    assert!(status.contains("applied"), "{status}");
    assert!(!work.join("old.txt").exists());
    assert!(work.join("keep.txt").exists());
    assert!(!helix_view::doc!(app.editor).is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn tree_example_shows_a_sidebar() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    std::fs::create_dir(work.join("src"))?;
    std::fs::write(work.join("src/main.rs"), "fn main() {}\n")?;
    std::fs::write(work.join("README.md"), "hi\n")?;
    let mut app = AppBuilder::new()
        .with_file(work.join("README.md"), None)
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("tree");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/tree").join(file),
            plugin.join(file),
        )?;
    }
    let settings = format!(
        "[tree]\nallow-files = \"read\"\nallow-dirs = [{:?}]\n",
        work.display().to_string()
    );
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), &settings)?;
    // The tree shows the workspace (the repository, in tests).
    keys(&mut app, ":tree/toggle<ret>").await;
    let sidebar = app
        .editor
        .panel(helix_view::sidebar::Dock::Left)
        .unwrap_or_else(|| panic!("no sidebar: {:?}", app.editor.get_status()));
    assert_eq!(sidebar.owner, "tree");
    assert!(sidebar.focused);
    assert!(!sidebar.items.is_empty());

    // The view got narrower to make room.
    let view = helix_view::view!(app.editor);
    assert!(view.area.x >= 12, "{:?}", view.area);

    // Keys move the selection while the sidebar has them; esc gives them back.
    let before = app
        .editor
        .panel(helix_view::sidebar::Dock::Left)
        .unwrap()
        .selected;
    keys(&mut app, "k").await;
    let after = app
        .editor
        .panel(helix_view::sidebar::Dock::Left)
        .unwrap()
        .selected;
    assert_eq!(after, before.saturating_sub(1));
    keys(&mut app, "<esc>").await;
    assert!(
        !app.editor
            .panel(helix_view::sidebar::Dock::Left)
            .unwrap()
            .focused
    );

    // Enter on a file opens it and gives the keys to the editor.
    keys(&mut app, ":tree/focus<ret>").await;
    let sidebar = app
        .editor
        .panel_mut(helix_view::sidebar::Dock::Left)
        .unwrap();
    let (index, path) = sidebar
        .items
        .iter()
        .enumerate()
        .find_map(|(i, item)| match &item.command {
            Some((command, args)) if command == "open" => Some((i, args[0].clone())),
            _ => None,
        })
        .expect("a file in the tree");
    sidebar.selected = index;
    keys(&mut app, "<ret>").await;
    assert_eq!(
        helix_view::doc!(app.editor)
            .path()
            .map(|p| p.display().to_string()),
        Some(path)
    );
    assert!(
        !app.editor
            .panel(helix_view::sidebar::Dock::Left)
            .unwrap()
            .focused
    );

    keys(&mut app, ":tree/toggle<ret>").await;
    assert!(
        app.editor.panel(helix_view::sidebar::Dock::Left).is_none(),
        "toggled off"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_commands_that_start_programs_need_allow_process() -> anyhow::Result<()> {
    // Without `allow-process = true`, `:sh` from a plugin is refused.
    let (mut app, _dir) = app_with_plugins("#[a|]#", ALL, "");
    keys(&mut app, ":plugin-run testplugin/run :sh echo hi<ret>").await;
    assert_error(&app, "allow-process = true");
    // Ordinary commands still work.
    keys(&mut app, ":plugin-run testplugin/run :echo fine<ret>").await;
    assert_eq!(status(&app).0, "fine");

    // A list of programs isn't enough: `:sh` runs anything.
    let (mut app, _dir) =
        app_with_plugins("#[a|]#", ALL, "[testplugin]\nallow-process = [\"echo\"]\n");
    keys(&mut app, ":plugin-run testplugin/run :sh echo hi<ret>").await;
    assert_error(&app, "allow-process = true");

    let (mut app, _dir) = app_with_plugins("#[a|]#", ALL, "[testplugin]\nallow-process = true\n");
    keys(&mut app, ":plugin-run testplugin/run :sh echo granted<ret>").await;
    deliver_events(&mut app).await;
    // It ran (its output is in a popup), not refused.
    assert!(app
        .editor
        .get_status()
        .is_none_or(|(message, _)| !message.contains("allow-process")));
    Ok(())
}

/// An editor on `text` (test selection syntax) with the user's `config`
/// and testplugin3 (API 0.5) loaded with `settings`.
fn app_v5(
    text: &str,
    config: Option<helix_term::config::Config>,
    settings: &str,
) -> (Application, tempfile::TempDir) {
    let mut builder = AppBuilder::new().with_input_text(text);
    if let Some(config) = config {
        builder = builder.with_config(config);
    }
    let mut app = builder.build().unwrap();
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
        format!("name = \"testplugin3\"\nversion = \"1\"\napi = \"0.5\"\npermissions = [{ALL}]\n"),
    )
    .unwrap();
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings).unwrap();
    (app, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_key_bindings_replace_defaults_and_get_the_count() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne two\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin edit-w normal w<ret>",
    )
    .await;
    keys(&mut app, "3w").await;
    assert_eq!(status(&app).0, "key:edit-w:w:Some(3)");
    // The plugin's edit (which moves the cursor by one), not the motion.
    assert_eq!(text(&app), "Kone two\n");
    assert_eq!(cursor(&app), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn passed_through_keys_do_what_they_would_without_the_binding() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne two\nthree\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin pass-w normal w<ret>",
    )
    .await;
    keys(&mut app, "w").await;
    assert_eq!(status(&app).0, "key:pass-w:w:None");
    assert!(cursor(&app) > 0, "w moved: {}", cursor(&app));
    // A sequence: `g g` still goes to the top.
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin pass-gg normal g g<ret>",
    )
    .await;
    keys(&mut app, "jgg").await;
    assert_eq!(status(&app).0, "key:pass-gg:g g:None");
    assert_eq!(cursor(&app), 0);
    // In insert mode a passed-through key types itself.
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin pass-j insert j<ret>",
    )
    .await;
    keys(&mut app, "ij<esc>").await;
    let log = helix_term::plugins::log_lines("testplugin3")?;
    assert!(
        log.iter().any(|line| line == "event:key:pass-j:j:None"),
        "{log:?}"
    );
    assert!(text(&app).starts_with("jone"), "{:?}", text(&app));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_users_keys_beat_plugin_bindings_unless_overridden() -> anyhow::Result<()> {
    use helix_term::keymap::{merge_keys, KeyTrie};
    let mut config = helpers::test_config();
    let user: std::collections::HashMap<helix_view::document::Mode, KeyTrie> =
        toml::from_str("[normal]\nC-k = \"goto_file_end\"\n").unwrap();
    merge_keys(&mut config.keys, user);
    let (mut app, _dir) = app_v5("#[o|]#ne\ntwo\n", Some(config.clone()), "");
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin edit-k normal C-k<ret>",
    )
    .await;
    keys(&mut app, "<C-k>").await;
    // The user's command ran; the plugin wasn't asked.
    assert_eq!(text(&app), "one\ntwo\n");
    assert!(cursor(&app) > 0);
    // Without the grant the override layer is refused...
    keys(
        &mut app,
        ":plugin-run testplugin3/bind override edit-k normal C-k<ret>",
    )
    .await;
    let (message, severity) = status(&app);
    assert_eq!(severity, Severity::Error);
    assert!(message.contains("allow-key-override"), "{message}");
    // ...with it, the plugin's binding wins.
    let (mut app, _dir) = app_v5(
        "#[o|]#ne\ntwo\n",
        Some(config),
        "[testplugin3]\nallow-key-override = true\n",
    );
    keys(
        &mut app,
        ":plugin-run testplugin3/bind override edit-k normal C-k<ret>",
    )
    .await;
    keys(&mut app, "<C-k>").await;
    assert_eq!(status(&app).0, "key:edit-k:C-k:None");
    assert_eq!(text(&app), "Kone\ntwo\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn document_scoped_bindings_apply_only_there() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne two\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin edit-w normal w @doc<ret>",
    )
    .await;
    keys(&mut app, "w").await;
    assert_eq!(text(&app), "Kone two\n");
    // Another document: `w` is the motion again.
    keys(&mut app, ":new<ret>ione two<esc>gg").await;
    keys(&mut app, "w").await;
    assert_eq!(text(&app), "one two\n");
    assert!(cursor(&app) > 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_key_callback_passes_the_keys_through_and_unbinds() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne two\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin spin-w normal w<ret>",
    )
    .await;
    keys(&mut app, "w").await;
    // Disabled for running too long; `w` still moved.
    let (message, severity) = status(&app);
    assert_eq!(severity, Severity::Error);
    assert!(message.contains("testplugin3"), "{message}");
    assert!(cursor(&app) > 0);
    // Its bindings are gone.
    assert!(helix_term::keymap::plugin_layers::bindings().is_empty());
    Ok(())
}

/// testplugin3 as a completion source, with insert-mode keys driving the
/// menu, and the menu opened on "al".
async fn app_with_menu_keys() -> (Application, tempfile::TempDir) {
    let (mut app, dir) = app_v5("#[\n|]#", None, "[testplugin3]\ncompletion = true\n");
    for (name, key) in [
        ("menu-next", "C-l"),
        ("menu-previous", "C-b"),
        ("menu-confirm", "C-o"),
        ("menu-abort", "C-g"),
        ("menu-close", "C-e"),
        ("menu-select-1", "C-t"),
        ("menu-select-none", "C-y"),
        ("menu-select-9", "C-v"),
    ] {
        keys(
            &mut app,
            &format!(":plugin-run testplugin3/bind plugin {name} insert {key}<ret>"),
        )
        .await;
    }
    keys(
        &mut app,
        ":plugin-run testplugin3/subscribe completion-menu<ret>",
    )
    .await;
    keys(&mut app, "ial").await;
    deliver_events(&mut app).await;
    (app, dir)
}

fn log(app: &Application) -> Vec<String> {
    let _ = app;
    helix_term::plugins::log_lines("testplugin3").unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_navigate_and_confirm_the_completion_menu() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_menu_keys().await;
    // The menu as the plugin sees it, then the preview of what it selected.
    keys(&mut app, "<C-l>").await;
    deliver_events(&mut app).await;
    assert_eq!(status(&app).0, "menu:alpha,alphabet:None:false|Ok(())");
    assert_eq!(text(&app), "alpha\n");
    keys(&mut app, "<C-l>").await;
    deliver_events(&mut app).await;
    assert_eq!(status(&app).0, "menu:alpha,alphabet:Some(0):true|Ok(())");
    assert_eq!(text(&app), "alphabet\n");
    // Back to the typed text; an item that isn't there is refused.
    keys(&mut app, "<C-y>").await;
    deliver_events(&mut app).await;
    assert_eq!(text(&app), "al\n");
    keys(&mut app, "<C-v>").await;
    assert!(status(&app)
        .0
        .contains("Err(\"no completion item 9 (the menu has 2)\")"));
    // Select and confirm: the item is in, the menu closed, still inserting.
    keys(&mut app, "<C-t>").await;
    deliver_events(&mut app).await;
    keys(&mut app, "<C-o>").await;
    deliver_events(&mut app).await;
    assert_eq!(text(&app), "alphabet\n");
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Insert);
    keys(&mut app, "<C-l>").await;
    assert_eq!(status(&app).0, "menu:none|Ok(())");
    // The events, in order.
    let log = log(&app);
    let events: Vec<&String> = log
        .iter()
        .filter(|line| line.starts_with("event:e2:Event::CompletionMenu"))
        .collect();
    assert_eq!(
        events,
        [
            "event:e2:Event::CompletionMenu(CompletionEvent::Opened(2))",
            "event:e2:Event::CompletionMenu(CompletionEvent::Confirmed(\"alphabet\"))",
            "event:e2:Event::CompletionMenu(CompletionEvent::Closed(CloseReason::Confirmed))",
        ],
        "{log:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn aborting_drops_the_preview_and_closing_keeps_it() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_menu_keys().await;
    keys(&mut app, "<C-t>").await;
    deliver_events(&mut app).await;
    assert_eq!(text(&app), "alphabet\n");
    keys(&mut app, "<C-g>").await;
    deliver_events(&mut app).await;
    assert_eq!(text(&app), "al\n");
    // Undo history is intact: one undo removes the typed text.
    keys(&mut app, "<esc>u").await;
    assert_eq!(text(&app), "\n");
    // Close with a preview keeps the previewed text (as typing would).
    keys(&mut app, "ial").await;
    deliver_events(&mut app).await;
    keys(&mut app, "<C-t>").await;
    deliver_events(&mut app).await;
    keys(&mut app, "<C-e>").await;
    deliver_events(&mut app).await;
    assert_eq!(text(&app), "alphabet\n");
    let log = log(&app);
    assert!(
        log.iter().any(|l| l
            == "event:e2:Event::CompletionMenu(CompletionEvent::Closed(CloseReason::Aborted))"),
        "{log:?}"
    );
    assert!(
        log.iter().any(|l| l
            == "event:e2:Event::CompletionMenu(CompletionEvent::Closed(CloseReason::Dismissed))"),
        "{log:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn leaving_insert_mode_reports_why_the_menu_closed() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_menu_keys().await;
    keys(&mut app, "<esc>").await;
    deliver_events(&mut app).await;
    let log = log(&app);
    assert!(
        log.iter().any(|l| l
            == "event:e2:Event::CompletionMenu(CompletionEvent::Closed(CloseReason::ModeChange))"),
        "{log:?}"
    );
    // ctrl-c aborts (and leaves insert mode in the same press).
    keys(&mut app, "oal").await;
    deliver_events(&mut app).await;
    keys(&mut app, "<C-c>").await;
    deliver_events(&mut app).await;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    let log = helix_term::plugins::log_lines("testplugin3")?;
    assert_eq!(
        log.iter()
            .filter(|l| *l
                == "event:e2:Event::CompletionMenu(CompletionEvent::Closed(CloseReason::Aborted))")
            .count(),
        1,
        "{log:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn completion_keys_drive_the_menu() -> anyhow::Result<()> {
    use helix_view::editor::CompletionKeyAction;
    use helix_view::input::KeyEvent;
    let mut config = helpers::test_config();
    config.editor.completion_keys = [
        ("C-y".parse::<KeyEvent>()?, CompletionKeyAction::Confirm),
        ("C-e".parse::<KeyEvent>()?, CompletionKeyAction::NormalMode),
    ]
    .into_iter()
    .collect();
    let (mut app, _dir) = app_v5(
        "#[\n|]#",
        Some(config),
        "[testplugin3]\ncompletion = true\n",
    );
    // Abort and leave insert mode in one key: the preview goes.
    keys(&mut app, "ial").await;
    deliver_events(&mut app).await;
    keys(&mut app, "<C-n>").await;
    assert_eq!(text(&app), "alpha\n");
    keys(&mut app, "<C-e>").await;
    assert_eq!(text(&app), "al\n");
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    // Confirm without selecting: the first item.
    keys(&mut app, "oal").await;
    deliver_events(&mut app).await;
    keys(&mut app, "<C-y><esc>").await;
    assert_eq!(text(&app), "al\nalpha\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_command_hooks_see_and_may_cancel_commands() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/subscribe pre-command @cmd move_char_right @cmd echo<ret>",
    )
    .await;
    // Asked, and it goes ahead.
    keys(&mut app, "l").await;
    assert_eq!(cursor(&app), 1);
    let log = helix_term::plugins::log_lines("testplugin3")?;
    assert!(
        log.iter()
            .any(|l| l == "event:pre-command:move_char_right:None:None"),
        "{log:?}"
    );
    // With the register and count it was given.
    keys(&mut app, "\"a2l").await;
    let log = helix_term::plugins::log_lines("testplugin3")?;
    assert!(
        log.iter()
            .any(|l| l == "event:pre-command:move_char_right:Some('a'):Some(2)"),
        "{log:?}"
    );
    keys(&mut app, "2h").await;
    // Other commands don't ask the plugin.
    keys(&mut app, "h").await;
    let log = helix_term::plugins::log_lines("testplugin3")?;
    assert!(
        !log.iter()
            .any(|l| l.starts_with("event:pre-command:move_char_left")),
        "{log:?}"
    );
    // Cancelled: the motion doesn't happen, nor the typable command.
    keys(
        &mut app,
        ":plugin-run testplugin3/subscribe pre-command @cmd move_char_right @cancel-commands<ret>",
    )
    .await;
    keys(&mut app, "l").await;
    assert_eq!(cursor(&app), 0);
    let (message, severity) = status(&app);
    assert_eq!(severity, Severity::Error);
    assert_eq!(
        message,
        "'move_char_right' cancelled by testplugin3: no move_char_right"
    );
    keys(&mut app, ":echo hello<ret>").await;
    let (message, _) = status(&app);
    assert!(
        message.contains("'echo' cancelled by testplugin3: no echo"),
        "{message}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_save_hooks_edit_or_cancel_the_write() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "mark-on-save\n")?;
    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("testplugin3");
    std::fs::create_dir(&plugin)?;
    std::fs::copy(
        helix_plugin_dir().join("tests/fixtures/testplugin3.wasm"),
        plugin.join("plugin.wasm"),
    )?;
    std::fs::write(
        plugin.join("plugin.toml"),
        format!("name = \"testplugin3\"\nversion = \"1\"\napi = \"0.5\"\npermissions = [{ALL}]\n"),
    )?;
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    keys(&mut app, ":plugin-run testplugin3/subscribe pre-save<ret>").await;
    keys(&mut app, ":w<ret>").await;
    // The plugin's edit is saved with the document.
    assert_eq!(
        std::fs::read_to_string(file.path())?,
        "mark-on-save\n// saved\n"
    );
    keys(
        &mut app,
        ":plugin-run testplugin3/subscribe pre-save @cancel-saves<ret>",
    )
    .await;
    keys(&mut app, "ix<esc>:w<ret>").await;
    let (message, severity) = status(&app);
    assert_eq!(severity, Severity::Error);
    assert!(
        message.contains("saving cancelled by testplugin3: not now"),
        "{message}"
    );
    assert_eq!(
        std::fs::read_to_string(file.path())?,
        "mark-on-save\n// saved\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_hear_about_focus_plugins_and_shutdown() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5(
        "#[o|]#ne\n",
        None,
        "[testplugin3]\nsubscribe-plugins = true\n",
    );
    for event in ["buffer-enter", "buffer-leave", "view-enter", "view-leave"] {
        keys(
            &mut app,
            &format!(":plugin-run testplugin3/subscribe {event}<ret>"),
        )
        .await;
    }
    keys(&mut app, ":vnew<ret>").await;
    deliver_events(&mut app).await;
    let log = helix_term::plugins::log_lines("testplugin3")?;
    for event in ["ViewLeave", "ViewEnter", "BufferLeave", "BufferEnter"] {
        assert!(
            log.iter()
                .any(|l| l.starts_with(&format!("event:e2:Event::{event}"))),
            "{event}: {log:?}"
        );
    }
    // Loaded with the plugin itself, then told about it.
    assert!(
        log.iter()
            .any(|l| l == "event:e2:Event::PluginChange((\"testplugin3\", PluginState::Loaded))"),
        "{log:?}"
    );
    helix_term::plugins::shutdown(&mut app.editor);
    let log = helix_term::plugins::log_lines("testplugin3")?;
    assert!(
        log.iter().any(|l| l == "event:e2:Event::Shutdown"),
        "{log:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_run_native_and_typable_commands_with_results() -> anyhow::Result<()> {
    let (mut app, dir) = app_v5("#[o|]#ne two\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/exec #3 move_char_right<ret>",
    )
    .await;
    let status = wait_for_status(&mut app, |s| s.starts_with("command-done:")).await;
    assert!(status.ends_with(":Ok(())"), "{status}");
    assert_eq!(cursor(&app), 3);
    // Typable commands get their arguments as they are: no expansion.
    let path = dir.path().join("out %{cursor_line}.txt");
    keys(
        &mut app,
        &format!(
            ":plugin-run testplugin3/exec write '{}'<ret>",
            path.display()
        ),
    )
    .await;
    // (The write's own message replaces the plugin's status.)
    let done = wait_for(&mut app, |_| {
        helix_term::plugins::log_lines("testplugin3")
            .ok()?
            .into_iter()
            .filter(|line| line.starts_with("event:command-done:"))
            .nth(1)
    })
    .await;
    assert!(done.ends_with(":Ok(())"), "{done}");
    wait_for(&mut app, |_| {
        std::fs::read_to_string(&path)
            .ok()
            .filter(|text| !text.is_empty())
    })
    .await;
    assert_eq!(std::fs::read_to_string(&path)?, "one two\n");
    // A failure is reported to the plugin.
    keys(
        &mut app,
        ":plugin-run testplugin3/exec set-option no-such-option 1<ret>",
    )
    .await;
    let status = wait_for_status(&mut app, |s| s.starts_with("command-done:")).await;
    assert!(status.contains("Err("), "{status}");
    // Programs only with allow-process.
    keys(
        &mut app,
        ":plugin-run testplugin3/exec run-shell-command true<ret>",
    )
    .await;
    assert_error(&app, "allow-process");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_batches_stop_at_the_first_failure() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne two\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/batch move_char_right ; set-option no-such-option 1 ; mode:insert<ret>",
    )
    .await;
    let status = wait_for_status(&mut app, |s| s.starts_with("batch-done:")).await;
    assert!(
        status.contains(":[Ok(()), Err(\"'set-option': "),
        "{status}"
    );
    assert!(status.ends_with(r#", Err("skipped")]"#), "{status}");
    // The first step ran; nothing is rolled back; the mode didn't change.
    assert_eq!(cursor(&app), 1);
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    keys(
        &mut app,
        ":plugin-run testplugin3/batch mode:insert ; move_char_right<ret>",
    )
    .await;
    let status = wait_for_status(&mut app, |s| s.starts_with("batch-done:")).await;
    assert!(status.ends_with(":[Ok(()), Ok(())]"), "{status}");
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Insert);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_switch_modes_and_see_views_and_ui() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne two\n", None, "");
    keys(&mut app, ":plugin-run testplugin3/set-mode select<ret>").await;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Select);
    keys(
        &mut app,
        "<esc>:vsplit<ret>:plugin-run testplugin3/views<ret>",
    )
    .await;
    let (views, _) = status(&app);
    let views: Vec<&str> = views.strip_prefix("views:").unwrap().split(',').collect();
    assert_eq!(views.len(), 2, "{views:?}");
    assert_eq!(
        views.iter().filter(|view| view.ends_with('*')).count(),
        1,
        "{views:?}"
    );
    keys(&mut app, ":plugin-run testplugin3/ui-state<ret>").await;
    let (ui, _) = status(&app);
    assert!(ui.starts_with("ui:Mode::Normal:menu=false:"), "{ui}");
    assert!(ui.contains(":picker=false:popup=false:panel=None:"), "{ui}");
    keys(&mut app, ":plugin-run testplugin3/count-commands<ret>").await;
    assert_eq!(
        status(&app).0,
        "commands:native=true:typable=true:plugin=true:true:true:true"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_call_each_others_services() -> anyhow::Result<()> {
    let (mut app, dir) = app_v5("#[o|]#ne\n", None, "[svc-b]\nprovide-echo = true\n");
    // A second copy of the test plugin, named svc-b, that testplugin3
    // depends on.
    let provider = dir.path().join("svc-b");
    std::fs::create_dir(&provider)?;
    std::fs::copy(
        dir.path().join("testplugin3/plugin.wasm"),
        provider.join("plugin.wasm"),
    )?;
    std::fs::write(
        provider.join("plugin.toml"),
        format!("name = \"svc-b\"\nversion = \"1\"\napi = \"0.5\"\npermissions = [{ALL}]\n"),
    )?;
    let manifest = dir.path().join("testplugin3/plugin.toml");
    let text = std::fs::read_to_string(&manifest)?;
    std::fs::write(&manifest, format!("{text}dependencies = [\"svc-b\"]\n"))?;
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[svc-b]\nprovide-echo = true\n",
    )?;
    keys(
        &mut app,
        ":plugin-run testplugin3/call svc-b.echo 1.0.0 echo '[1, 2]'<ret>",
    )
    .await;
    let status = wait_for_status(&mut app, |s| s.starts_with("service-reply:")).await;
    assert!(status.ends_with(r#":Ok("[1, 2]")"#), "{status}");
    keys(
        &mut app,
        ":plugin-run testplugin3/call svc-b.echo 1.0.0 fail 1<ret>",
    )
    .await;
    let status = wait_for_status(&mut app, |s| s.starts_with("service-reply:")).await;
    assert!(status.ends_with(r#":Err("failed on purpose")"#), "{status}");
    Ok(())
}

/// The current status message's text after `prefix`.
fn status_after<'a>(status: &'a str, prefix: &str) -> &'a str {
    status
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("{status}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_marks_follow_edits() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne two\n", None, "");
    let mut ids = Vec::new();
    for args in ["4", "4 after", "2"] {
        keys(
            &mut app,
            &format!(":plugin-run testplugin3/mark {args}<ret>"),
        )
        .await;
        ids.push(status_after(&status(&app).0, "mark:").to_string());
    }
    let (before, after, inside) = (&ids[0], &ids[1], &ids[2]);
    // Typing before them moves them.
    keys(&mut app, "iXX<esc>").await;
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/marks {before}<ret>"),
    )
    .await;
    assert_eq!(
        status(&app).0,
        format!("marks:{inside}@4,{before}@6,{after}@6:Some(6)")
    );
    // Text inserted right at them: gravity decides.
    keys(&mut app, "gh6li!<esc>").await;
    assert_eq!(text(&app), "XXone !two\n");
    keys(&mut app, ":plugin-run testplugin3/marks<ret>").await;
    assert_eq!(
        status(&app).0,
        format!("marks:{inside}@4,{before}@6,{after}@7:")
    );
    // Deleting the text around a mark removes it.
    keys(&mut app, "gh2lv2ld").await;
    assert_eq!(text(&app), "XX !two\n");
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/marks {inside}<ret>"),
    )
    .await;
    assert_eq!(status(&app).0, format!("marks:{before}@3,{after}@4:None"));
    // Removed explicitly.
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/unmark {before}<ret>"),
    )
    .await;
    keys(&mut app, ":plugin-run testplugin3/marks<ret>").await;
    assert_eq!(status(&app).0, format!("marks:{after}@4:"));
    // Out of range.
    keys(&mut app, ":plugin-run testplugin3/mark 99<ret>").await;
    assert_error(&app, "invalid position 99");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_virtual_lines_take_rows_below_their_lines() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne\ntwo\nthree\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/vlines 0 first-row 0 second-row 1 below-two<ret>",
    )
    .await;
    let screen = app.terminal_buffer().clone();
    let rows: Vec<String> = (screen.area.y..screen.area.bottom())
        .map(|y| {
            (screen.area.x..screen.area.right())
                .map(|x| screen[(x, y)].symbol.to_string())
                .collect::<String>()
        })
        .collect();
    let find = |text: &str| {
        rows.iter()
            .position(|row| row.contains(text))
            .unwrap_or_else(|| panic!("{text} not on screen: {rows:#?}"))
    };
    let one = find("one");
    assert_eq!(find("first-row"), one + 1);
    assert_eq!(find("second-row"), one + 2);
    assert_eq!(find("two"), one + 3);
    assert_eq!(find("below-two"), one + 4);
    assert_eq!(find("three"), one + 5);
    // They move with their line.
    keys(&mut app, "Ozero<esc>").await;
    let screen = app.terminal_buffer().clone();
    let row_of = |text: &str| {
        (screen.area.y..screen.area.bottom())
            .find(|&y| {
                (screen.area.x..screen.area.right())
                    .map(|x| screen[(x, y)].symbol.to_string())
                    .collect::<String>()
                    .contains(text)
            })
            .unwrap()
    };
    assert_eq!(row_of("first-row"), row_of("one") + 1);
    assert_eq!(row_of("one"), row_of("zero") + 1);
    keys(&mut app, ":plugin-run testplugin3/vlines 9 nope<ret>").await;
    assert_error(&app, "invalid line 9");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_patch_parts_of_decoration_layers() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne two three\n", None, "");
    let ranges = |app: &Application| {
        helix_view::doc!(app.editor)
            .plugin_decorations
            .highlight_ranges("testplugin3/hl")
    };
    keys(
        &mut app,
        ":plugin-run testplugin3/patch-hl 0 14 0 3 4 7 8 13<ret>",
    )
    .await;
    assert_eq!(ranges(&app), [0..3, 4..7, 8..13]);
    // Only the part in 4..8 changes.
    keys(&mut app, ":plugin-run testplugin3/patch-hl 4 8 5 6<ret>").await;
    assert_eq!(ranges(&app), [0..3, 5..6, 8..13]);
    keys(&mut app, ":plugin-run testplugin3/patch-hl 4 8 9 10<ret>").await;
    assert_error(&app, "doesn't start in 4..8");
    let texts = |app: &Application| {
        helix_view::doc!(app.editor)
            .plugin_decorations
            .annotation_texts("testplugin3/ann")
    };
    keys(
        &mut app,
        ":plugin-run testplugin3/patch-ann 0 14 3 a 7 b 14 c<ret>",
    )
    .await;
    assert_eq!(texts(&app), ["a", "b", "c"]);
    keys(&mut app, ":plugin-run testplugin3/patch-ann 5 14 9 d<ret>").await;
    assert_eq!(texts(&app), ["a", "d"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_floats_take_keys_and_run_their_commands() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/float Pick alpha beta gamma<ret>",
    )
    .await;
    let screen = app.terminal_buffer().clone();
    let shown: String = (screen.area.y..screen.area.bottom())
        .flat_map(|y| (screen.area.x..screen.area.right()).map(move |x| (x, y)))
        .map(|(x, y)| screen[(x, y)].symbol.to_string())
        .collect();
    assert!(shown.contains("Pick") && shown.contains("beta"), "{shown}");
    // The float has the keys: j moves its selection, enter runs the item.
    keys(&mut app, "j<ret>").await;
    assert_eq!(status(&app).0, "float-selected:Some(1):beta");
    assert_eq!(text(&app), "one\n");
    keys(&mut app, ":plugin-run testplugin3/float-close<ret>").await;
    assert!(app
        .editor
        .panels
        .iter()
        .all(|panel| panel.dock != helix_view::sidebar::Dock::Float));
    Ok(())
}

/// `app_v5` with the test plugin asking for `permissions` too.
fn app_v5_granted(
    text: &str,
    permissions: &str,
    settings: &str,
) -> (Application, tempfile::TempDir) {
    let (mut app, dir) = app_v5(text, None, settings);
    std::fs::write(
        dir.path().join("testplugin3/plugin.toml"),
        format!("name = \"testplugin3\"\nversion = \"1\"\napi = \"0.5\"\npermissions = [{ALL}, {permissions}]\n"),
    )
    .unwrap();
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings).unwrap();
    (app, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_sessions_take_input_while_running() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5_granted(
        "#[o|]#ne\n",
        r#""process""#,
        "[testplugin3]\nallow-process = true\n",
    );
    keys(&mut app, ":plugin-run testplugin3/session cat<ret>").await;
    let id = status_after(&status(&app).0, "session:").to_string();
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/session-write {id} hello there<ret>"),
    )
    .await;
    let chunk = wait_for_status(&mut app, |s| s.starts_with("chunk:")).await;
    assert!(chunk.starts_with("chunk:hello there\n"), "{chunk}");
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/session-write {id} again<ret>"),
    )
    .await;
    let chunk = wait_for_status(&mut app, |s| s.starts_with("chunk:again")).await;
    assert!(chunk.starts_with("chunk:again\n"), "{chunk}");
    // Closing the input ends `cat`.
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/session-close {id}<ret>"),
    )
    .await;
    let done = wait_for_status(&mut app, |s| s.starts_with("done:")).await;
    assert!(done.starts_with("done:Some(0)"), "{done}");
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/session-write {id} late<ret>"),
    )
    .await;
    assert_error(&app, &format!("no running session {id}"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_session_input_is_bounded() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5_granted(
        "#[o|]#ne\n",
        r#""process""#,
        "[testplugin3]\nallow-process = true\n",
    );
    // `sleep` never reads its input: the pipe fills, the rest waits.
    keys(&mut app, ":plugin-run testplugin3/session sleep 30<ret>").await;
    let id = status_after(&status(&app).0, "session:").to_string();
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/session-write {id} big:1048576<ret>"),
    )
    .await;
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/session-write {id} big:1000000<ret>"),
    )
    .await;
    assert_error(&app, "busy");
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/session-write {id} big:1048577<ret>"),
    )
    .await;
    assert_error(&app, "at most 1 MiB per write");
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/session-close {id}<ret>"),
    )
    .await;
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/session-write {id} x<ret>"),
    )
    .await;
    assert_error(&app, "input is closed");
    // Programs need the grant as for spawn.
    let (mut app, _dir) = app_v5_granted("#[o|]#ne\n", r#""process""#, "");
    keys(&mut app, ":plugin-run testplugin3/session cat<ret>").await;
    assert_error(&app, "allow-process");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_progress_shows_in_the_statusline() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne\n", None, "");
    let progress = |app: &Application| {
        app.editor
            .plugin_statusline
            .iter()
            .filter(|(key, _)| key.starts_with("testplugin3/progress-"))
            .map(|(_, text)| text.clone())
            .collect::<Vec<_>>()
    };
    keys(
        &mut app,
        ":plugin-run testplugin3/progress Indexing files 40<ret>",
    )
    .await;
    assert_eq!(progress(&app), ["Indexing: files 40%"]);
    // Shown by default.
    let screen = app.terminal_buffer().clone();
    let shown: String = (screen.area.y..screen.area.bottom())
        .flat_map(|y| (screen.area.x..screen.area.right()).map(move |x| (x, y)))
        .map(|(x, y)| screen[(x, y)].symbol.to_string())
        .collect();
    assert!(shown.contains("Indexing: files 40%"));
    keys(&mut app, ":plugin-run testplugin3/progress end<ret>").await;
    assert!(progress(&app).is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugins_watch_paths_they_may_read() -> anyhow::Result<()> {
    let watched = tempfile::tempdir()?;
    let dir = watched.path().canonicalize()?;
    let settings = format!(
        "[testplugin3]\nallow-files = \"read\"\nallow-dirs = [\"{}\"]\n",
        dir.display()
    );
    let (mut app, _plugins) = app_v5_granted("#[o|]#ne\n", r#""filesystem""#, &settings);
    keys(
        &mut app,
        &format!(":plugin-run testplugin3/watch {} r<ret>", dir.display()),
    )
    .await;
    let id = status_after(&status(&app).0, "watching:").to_string();
    // (A file in a directory made just now could come before its watch.)
    std::fs::write(dir.join("new.txt"), "hi")?;
    let line = wait_for(&mut app, |_| {
        helix_term::plugins::log_lines("testplugin3")
            .ok()?
            .into_iter()
            .find(|line| line.contains("FileChange") && line.contains("new.txt"))
    })
    .await;
    assert!(line.contains(&format!("FileChange(({id}, [")), "{line}");
    // Outside the granted directories: refused.
    keys(&mut app, ":plugin-run testplugin3/watch /etc<ret>").await;
    assert_error(&app, "outside the directories");
    keys(&mut app, ":plugin-run testplugin3/watch relative<ret>").await;
    assert_error(&app, "absolute");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_requests_need_the_grant() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5_granted("#[o|]#ne\n", r#""lsp""#, "");
    keys(&mut app, ":plugin-run testplugin3/lsp-servers<ret>").await;
    assert_error(&app, "allow-lsp = true");
    let (mut app, _dir) = app_v5_granted(
        "#[o|]#ne\n",
        r#""lsp""#,
        "[testplugin3]\nallow-lsp = true\n",
    );
    keys(&mut app, ":plugin-run testplugin3/lsp-servers<ret>").await;
    assert_eq!(status(&app).0, "servers:");
    keys(
        &mut app,
        ":plugin-run testplugin3/lsp-request textDocument/hover {}<ret>",
    )
    .await;
    assert_error(&app, "no language server on this document");
    // Without the permission in plugin.toml, the grant isn't enough.
    let (mut app, _dir) = app_v5("#[o|]#ne\n", None, "[testplugin3]\nallow-lsp = true\n");
    keys(&mut app, ":plugin-run testplugin3/lsp-servers<ret>").await;
    assert_error(&app, "\"lsp\"");
    Ok(())
}

/// The virtual lines on the current document: `(line, text)`.
fn virtual_rows(app: &Application) -> Vec<(usize, String)> {
    let doc = helix_view::doc!(app.editor);
    doc.plugin_decorations
        .virtual_rows(doc.text().slice(..))
        .into_iter()
        .map(|(line, text, _)| (line, text.to_string()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn lens_example_shows_uses_and_follows_settings_changes() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let file = root.path().join("main.rs");
    std::fs::write(
        &file,
        "fn main() {\n    helper();\n    helper();\n}\n\nfn helper() {}\n",
    )?;
    let (mut app, _plugins) = app_with_example_on("lens", &file, "");
    let rows = wait_for(&mut app, |app| {
        let rows = virtual_rows(app);
        (rows.len() == 2).then_some(rows)
    })
    .await;
    assert_eq!(
        rows,
        [
            (0, "    ↳ unused in this file".to_string()),
            (5, "    ↳ 2 uses".to_string())
        ]
    );
    // The settings change: the plugin hears about it, no reload.
    let settings: toml::Table = toml::from_str("[lens]\nmin-uses = 1\n")?;
    helix_term::plugins::settings_changed(&mut app.editor, &settings);
    assert_eq!(virtual_rows(&app), [(5, "    ↳ 2 uses".to_string())]);
    // Settings the schema refuses are reported, and change nothing.
    let settings: toml::Table = toml::from_str("[lens]\nmin-uses = \"lots\"\n")?;
    helix_term::plugins::settings_changed(&mut app.editor, &settings);
    assert_error(&app, "min-uses: expected integer");
    assert_eq!(virtual_rows(&app).len(), 1);
    // Its key, bound from init.
    keys(&mut app, "<space>L").await;
    assert!(virtual_rows(&app).is_empty());
    keys(&mut app, "<space>L").await;
    assert_eq!(virtual_rows(&app).len(), 1);
    // A changed grant reloads the plugins.
    let settings: toml::Table = toml::from_str("[lens]\nmin-uses = 1\nallow-lsp = true\n")?;
    helix_term::plugins::settings_changed(&mut app.editor, &settings);
    assert!(
        status(&app).0.contains("Plugins reloaded"),
        "{:?}",
        status(&app)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_settings_schemas_are_checked_at_load() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let file = root.path().join("main.rs");
    std::fs::write(&file, "fn main() {}\n")?;
    let (app, _plugins) = app_with_example_on("lens", &file, "[lens]\nmin-use = 2\n");
    assert_error(
        &app,
        "unknown setting 'min-use' (the plugin has: enabled, min-uses, scope)",
    );
    assert!(helix_term::plugins::plugin_names().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_doctor_and_trace_report_what_plugins_do() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5_granted("#[o|]#ne\n", r#""process""#, "");
    let report = helix_term::plugins::doctor();
    assert!(report.contains("## testplugin3 1 (API 0.5)"), "{report}");
    assert!(
        report.contains("**Not granted:** process (needs allow-process)"),
        "{report}"
    );
    keys(&mut app, ":plugin-doctor<ret>").await;
    keys(&mut app, ":plugin-trace testplugin3<ret>").await;
    assert_eq!(
        status(&app).0,
        "Tracing 1 plugin(s): every call and its time go to :plugin-log"
    );
    keys(&mut app, ":plugin-run testplugin3/caps<ret>").await;
    assert!(helix_term::plugins::log_lines("testplugin3")?
        .iter()
        .any(|line| line.starts_with("trace: command call, ")));
    keys(&mut app, ":plugin-trace nope<ret>").await;
    assert_error(&app, "no such plugin loaded");
    keys(&mut app, ":plugin-trace off<ret>").await;
    assert!(status(&app).0.starts_with("Stopped tracing"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn one_escape_closes_a_plugin_popup_and_leaves_insert_mode() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne\n", None, "");
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin popup-key insert C-q<ret>",
    )
    .await;
    // A popup that appears while typing...
    keys(&mut app, "i<C-q>").await;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Insert);
    // ...closes with the same Esc that leaves insert mode.
    keys(&mut app, "<esc>").await;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    keys(&mut app, ":plugin-run testplugin3/popup-events<ret>").await;
    let events = status(&app).0;
    assert!(
        events.contains("closed:[") && !events.contains("closed:[]"),
        "{events}"
    );
    // Ctrl-c does the same.
    keys(&mut app, "i<C-q><C-c>").await;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    // In normal mode Esc still just closes the popup.
    keys(&mut app, ":plugin-run testplugin3/popup - shown<ret>").await;
    keys(&mut app, "<esc>").await;
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn one_escape_closes_an_info_popup_and_leaves_insert_mode() -> anyhow::Result<()> {
    use helix_term::ui::{Popup, Text};
    let mut app = AppBuilder::new().with_input_text("#[o|]#ne\n").build()?;
    for (open_in_insert, escape) in [(true, "<esc>"), (true, "<C-c>"), (false, "<esc>")] {
        if open_in_insert {
            keys(&mut app, "i").await;
        }
        helix_term::job::dispatch(|_, compositor| {
            // What hover and the other documentation popups are.
            let popup = Popup::new("hover", Text::new("documentation".into())).auto_close(true);
            compositor.replace_or_push("hover", popup);
        })
        .await;
        keys(&mut app, "").await;
        keys(&mut app, escape).await;
        // One press leaves insert mode and closes the popup.
        assert_eq!(
            app.editor.mode(),
            helix_view::document::Mode::Normal,
            "{escape}"
        );
        let open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let seen = open.clone();
        helix_term::job::dispatch(move |_, compositor| {
            seen.store(
                compositor.find_id::<Popup<Text>>("hover").is_some(),
                std::sync::atomic::Ordering::Relaxed,
            );
        })
        .await;
        keys(&mut app, "").await;
        assert!(
            !open.load(std::sync::atomic::Ordering::Relaxed),
            "{escape}: the popup is still open"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn one_ctrl_c_leaves_insert_mode_with_signature_help_open() -> anyhow::Result<()> {
    use helix_term::ui::{Popup, Text};
    let mut app = AppBuilder::new().with_input_text("#[o|]#ne\n").build()?;
    for cancel in ["<C-c>", "<esc>"] {
        keys(&mut app, "i").await;
        helix_term::job::dispatch(|_, compositor| {
            // Set up like the language server's signature help.
            let popup = Popup::new(
                "signature-help",
                Text::new("struct Document { rope }".into()),
            )
            .ignore_escape_key(true)
            .pass_ctrl_c(true);
            compositor.replace_or_push("signature-help", popup);
        })
        .await;
        keys(&mut app, "").await;
        keys(&mut app, cancel).await;
        assert_eq!(
            app.editor.mode(),
            helix_view::document::Mode::Normal,
            "{cancel}"
        );
        let open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let seen = open.clone();
        helix_term::job::dispatch(move |_, compositor| {
            seen.store(
                compositor
                    .find_id::<Popup<Text>>("signature-help")
                    .is_some(),
                std::sync::atomic::Ordering::Relaxed,
            );
        })
        .await;
        keys(&mut app, "").await;
        assert!(
            !open.load(std::sync::atomic::Ordering::Relaxed),
            "{cancel}: signature help is still open"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn oil_example_reuses_one_buffer_while_moving_between_directories() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let work = helix_stdx::path::canonicalize(root.path());
    std::fs::write(work.join("a.txt"), "a")?;
    std::fs::create_dir_all(work.join("sub/deeper"))?;
    std::fs::write(work.join("sub/inner.txt"), "inner")?;
    std::fs::write(work.join("sub/deeper/note.txt"), "note")?;
    let mut app = AppBuilder::new()
        .with_file(work.join("a.txt"), None)
        .build()
        .unwrap();
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("oil");
    std::fs::create_dir(&plugin)?;
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            helix_plugin_dir().join("examples/oil").join(file),
            plugin.join(file),
        )?;
    }
    let settings = format!(
        "[oil]\nallow-files = \"write\"\nallow-dirs = [{:?}]\nconfirm = false\n",
        work.display().to_string()
    );
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), &settings)?;
    let oil_buffers = |app: &Application| {
        app.editor
            .documents()
            .filter(|doc| doc.display_name().starts_with("oil://"))
            .map(|doc| doc.display_name().to_string())
            .collect::<Vec<_>>()
    };

    keys(&mut app, "-").await;
    // Drawn in the first frame: decorated as soon as it opens.
    assert!(!helix_view::doc!(app.editor).plugin_decorations.is_empty());
    // Into sub/ (the first line), into deeper/, up, up, into sub/ again.
    for step in ["gg<ret>", "gg<ret>", "-", "-", "gg<ret>"] {
        keys(&mut app, step).await;
        deliver_events(&mut app).await;
        assert_eq!(
            oil_buffers(&app).len(),
            1,
            "after {step}: {:?}",
            oil_buffers(&app)
        );
        assert!(!helix_view::doc!(app.editor).plugin_decorations.is_empty());
    }
    assert_eq!(
        oil_buffers(&app),
        [format!("oil://{}/sub/", work.display())]
    );
    // A directory with unsaved changes stays open (the cut half of a move).
    keys(&mut app, "ggxd").await;
    assert!(helix_view::doc!(app.editor).is_modified());
    keys(&mut app, "-").await;
    deliver_events(&mut app).await;
    let mut open = oil_buffers(&app);
    open.sort();
    assert_eq!(
        open,
        [
            format!("oil://{}/", work.display()),
            format!("oil://{}/sub/", work.display())
        ]
    );
    Ok(())
}

/// API 0.5 `syntax`: plugins query the document's own syntax tree, as it is
/// after the latest edit.
#[tokio::test(flavor = "multi_thread")]
async fn plugins_query_the_syntax_tree() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("fn #[m|]#ain() {}\n", None, "");
    keys(&mut app, ":set-language rust<ret>").await;
    keys(
        &mut app,
        ":plugin-run testplugin3/syntax '(function_item name: (identifier) @name)'<ret>",
    )
    .await;
    assert_eq!(status(&app).0, "syntax:lang=rust 0:name=identifier@3-7");
    // Renamed: the next call sees the new tree.
    keys(&mut app, "ecrun<esc>").await;
    assert_eq!(text(&app), "fn run() {}\n");
    keys(
        &mut app,
        ":plugin-run testplugin3/syntax '(function_item name: (identifier) @name)'<ret>",
    )
    .await;
    assert_eq!(status(&app).0, "syntax:lang=rust 0:name=identifier@3-6");
    Ok(())
}

/// API 0.5 `http`: even an allowed host can't be a local address (here an
/// IP literal, so no network is needed); the reply says why.
#[tokio::test(flavor = "multi_thread")]
async fn plugin_https_requests_never_reach_local_addresses() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5_granted(
        "#[o|]#ne\n",
        r#""network""#,
        "[testplugin3]\nallow-network = [\"127.0.0.1\", \"::1\"]\n",
    );
    for url in ["https://127.0.0.1/", "https://[::1]:8443/x"] {
        keys(
            &mut app,
            &format!(":plugin-run testplugin3/http GET {url}<ret>"),
        )
        .await;
        for _ in 0..50 {
            if status(&app).0.starts_with("service-reply") {
                break;
            }
            deliver_events(&mut app).await;
        }
        let (message, _) = status(&app);
        assert!(message.contains("not public"), "{url}: {message}");
    }
    // Hosts that aren't allowed fail right away.
    keys(
        &mut app,
        ":plugin-run testplugin3/http GET https://example.com/<ret>",
    )
    .await;
    assert!(
        status(&app).0.contains("allow-network"),
        "{}",
        status(&app).0
    );
    Ok(())
}

/// The `script` example: a Rhai script from plugins.toml defines a command,
/// binds a key and handles saves; `:script/eval` runs a line.
#[tokio::test(flavor = "multi_thread")]
async fn script_example_runs_a_user_script() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one two\n")?;
    let settings = r#"[script]
source = '''
define("upper", "Uppercase the selections", "upper");
bind("normal", "space u", "upper");
on("save", "saved");
fn upper() { replace_selections(selected().map(|s| s.to_upper())); }
fn saved() { status(`saved ${text().len()} chars`); }
'''
"#;
    let (mut app, _dir) = app_with_example_on("script", file.path(), settings);
    keys(&mut app, "e:script/upper<ret>").await;
    assert_eq!(text(&app), "ONE two\n");
    // The key the script bound.
    keys(&mut app, "e<space>u").await;
    assert_eq!(text(&app), "ONE TWO\n", "{:?}", app.editor.get_status());
    keys(&mut app, ":w<ret>").await;
    deliver_events(&mut app).await;
    assert_eq!(status(&app).0, "saved 8 chars");

    keys(&mut app, ":script/eval 6 * 7<ret>").await;
    assert_eq!(status(&app).0, "42");
    // The script's functions are there too.
    keys(&mut app, ":script/eval selected()<ret>").await;
    assert_eq!(status(&app).0, r#"[" TWO"]"#);
    // An endless loop is stopped; the plugin keeps working.
    keys(&mut app, ":script/eval loop {}<ret>").await;
    let (message, severity) = status(&app);
    assert_eq!(severity, Severity::Error);
    assert!(message.contains("Too many operations"), "{message}");
    keys(&mut app, ":script/eval 1 + 1<ret>").await;
    assert_eq!(status(&app).0, "2");
    Ok(())
}

/// API 0.5 `commands.feed-keys` and `keymaps.next-key`: plugins play keys
/// as if typed, and take the key after theirs (as `f` and `r` do).
#[tokio::test(flavor = "multi_thread")]
async fn plugins_feed_keys_and_take_the_next_key() -> anyhow::Result<()> {
    let (mut app, _dir) = app_v5("#[o|]#ne two\n", None, "");
    // Keys go through the keymap: `w` then `d`.
    keys(&mut app, ":plugin-run testplugin3/feed wd<ret>").await;
    assert_eq!(text(&app), "two\n");
    // Insert mode is left at the end.
    keys(&mut app, ":plugin-run testplugin3/feed iX<ret>").await;
    assert_eq!(text(&app), "Xtwo\n");
    assert_eq!(app.editor.mode(), helix_view::document::Mode::Normal);
    // A program-starting command in fed keys needs allow-process.
    keys(
        &mut app,
        ":plugin-run testplugin3/feed-ret :sh<space>true<ret>",
    )
    .await;
    assert!(
        status(&app).0.contains("allow-process"),
        "{}",
        status(&app).0
    );
    // A command line the keys leave open is closed: the next key edits.
    keys(&mut app, ":plugin-run testplugin3/feed :echo<space>hi<ret>").await;
    keys(&mut app, "gg;d").await;
    assert_eq!(text(&app), "two\n");
    keys(&mut app, "u").await;
    // Keys being fed can't feed more: `K` asks to feed `x`.
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin feed:x normal K<ret>",
    )
    .await;
    keys(&mut app, ":plugin-run testplugin3/feed K<ret>").await;
    assert!(
        status(&app).0.contains("can't feed more keys"),
        "{}",
        status(&app).0
    );

    // A key bound to `then-next` asks for the next key; that one goes to
    // the plugin, not the keymap.
    keys(
        &mut app,
        ":plugin-run testplugin3/bind plugin then-next normal Q<ret>",
    )
    .await;
    keys(&mut app, "Q").await;
    keys(&mut app, "d").await;
    assert_eq!(status(&app).0, "key:next:d:None");
    assert_eq!(text(&app), "Xtwo\n");
    keys(&mut app, "Q<esc>").await;
    assert_eq!(status(&app).0, "key:next:esc:None");
    keys(&mut app, "QA").await;
    assert_eq!(status(&app).0, "key:next:A:None");
    Ok(())
}

/// The `find-char` example: `f`/`F`/`t`/`T` replaced by a plugin, staying
/// on the line like Vim's, with repeat.
#[tokio::test(flavor = "multi_thread")]
async fn find_char_example_replaces_f_and_t() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("a,b,c\nx,y\n")?;
    let (mut app, _dir) = app_with_example_on("find-char", file.path(), "");
    let selection = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let range = doc.selection(view.id).primary();
        (range.anchor, range.head)
    };
    // From `a` to the first `,` (inclusive, as Helix's `f`).
    keys(&mut app, "f,").await;
    assert_eq!(selection(&app), (0, 2));
    // A count: the second `,`; `t` stops before it.
    keys(&mut app, "gg2t,").await;
    assert_eq!(selection(&app), (0, 3));
    // Only on the line: there is no `y` on the first one.
    keys(&mut app, "ggfy").await;
    assert_eq!(selection(&app), (0, 1));
    // Repeat goes on from the last find; backward with `F`.
    keys(&mut app, "ggf,:find-char/repeat<ret>").await;
    assert_eq!(selection(&app), (1, 4));
    keys(&mut app, "F,").await;
    assert_eq!(selection(&app), (4, 1));
    // Escape cancels.
    keys(&mut app, "ggf<esc>").await;
    assert_eq!(selection(&app), (0, 1));
    Ok(())
}

/// The `vim-registers` example: Vim's numbered registers from a plugin.
#[tokio::test(flavor = "multi_thread")]
async fn vim_registers_example_fills_numbered_registers() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one\ntwo\nthree four\n")?;
    let (mut app, _dir) = app_with_example_on("vim-registers", file.path(), "");
    let register = |app: &Application, name: char| -> Vec<String> {
        app.editor
            .registers
            .read(name, &app.editor)
            .map(|values| values.map(|v| v.into_owned()).collect())
            .unwrap_or_default()
    };
    keys(&mut app, "xy;").await;
    assert_eq!(register(&app, '0'), ["one\n"]);
    keys(&mut app, "xd").await;
    keys(&mut app, "xd").await;
    assert_eq!(register(&app, '1'), ["two\n"]);
    assert_eq!(register(&app, '2'), ["one\n"]);
    keys(&mut app, "ed").await;
    assert_eq!(register(&app, '-'), ["three"]);
    // A named register leaves them alone.
    keys(&mut app, "x\"ad").await;
    assert_eq!(register(&app, 'a'), [" four\n"]);
    assert_eq!(register(&app, '1'), ["two\n"]);
    Ok(())
}
