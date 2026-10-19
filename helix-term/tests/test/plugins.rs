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
async fn process_grants_are_listed_and_invalid_grants_do_not_load_plugins() -> anyhow::Result<()> {
    let (mut app, dir) = app_with_plugins("#[a|]#", ALL, "");
    let path = dir.path().join("testplugin/plugin.toml");
    let source =
        std::fs::read_to_string(&path)?.replace("permissions = [", "permissions = [\"process\", ");
    std::fs::write(path, source)?;
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[testplugin]\nallow-process = ['rg', 'fd']\n",
    )?;
    assert!(helix_term::plugins::describe().contains("process: only rg, fd"));
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[testplugin]\nallow-process = true\n",
    )?;
    assert!(helix_term::plugins::describe().contains("process: granted"));
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), "")?;
    assert!(helix_term::plugins::describe().contains("process: not granted"));
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[testplugin]\nallow-process = 42\n",
    )?;
    assert_error(&app, "Invalid allow-process under [testplugin]");
    let names = helix_term::plugins::command_names();
    assert!(!names.iter().any(|name| name.starts_with("testplugin/")));
    assert!(names.contains(&"text-tools/upper".to_owned()));
    Ok(())
}

#[cfg(unix)]
fn app_with_process_plugin(settings: &str) -> (Application, tempfile::TempDir) {
    let (mut app, dir) = app_with_plugins("#[a|]#bc\n", ALL, "");
    let plugin = dir.path().join("testplugin2");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::copy(
        helix_plugin_dir().join("tests/fixtures/testplugin2.wasm"),
        plugin.join("plugin.wasm"),
    )
    .unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        format!(
            "name = 'testplugin2'\nversion = '1'\napi = '0.2'\npermissions = [{ALL}, 'process']\n"
        ),
    )
    .unwrap();
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings).unwrap();
    (app, dir)
}

#[cfg(unix)]
async fn wait_process_status(app: &mut Application, expected: impl Fn(&str) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if app
            .editor
            .get_status()
            .is_some_and(|(message, _)| expected(message))
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "process callback did not arrive: {:?}",
            app.editor.get_status()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        app.editor.reset_idle_timer();
        run_event_loop_until_idle(app).await;
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn processes_return_both_streams_and_stdin_to_wasm_callbacks() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_process_plugin("[testplugin2]\nallow-process = ['sh', 'cat']\n");
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn -- sh -c 'echo hi; echo err <gt>&2; exit 3'<ret>",
    )
    .await;
    wait_process_status(&mut app, |message| message.starts_with("exit:")).await;
    assert_eq!(status(&app).0, "exit:Some(3);out:hi\n;err:err\n");
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn-stdin 'hello stdin' cat<ret>",
    )
    .await;
    wait_process_status(&mut app, |message| message.starts_with("exit:")).await;
    assert_eq!(status(&app).0, "exit:Some(0);out:hello stdin;err:");
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn process_callbacks_apply_edits_and_editor_commands() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_process_plugin("[testplugin2]\nallow-process = true\n");
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn-edit printf hi<ret>",
    )
    .await;
    wait_process_status(&mut app, |message| message == "process edited").await;
    assert_eq!(text(&app), "hiabc\n");
    keys(&mut app, "u").await;
    assert_eq!(text(&app), "abc\n");
    keys(&mut app, ":plugin-run testplugin2/spawn-command true<ret>").await;
    wait_process_status(&mut app, |message| message == "process command").await;
    assert_eq!(cursor(&app), 2);
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn process_timeouts_kill_and_missing_programs_complete_once() -> anyhow::Result<()> {
    let (mut app, _dir) = app_with_process_plugin("[testplugin2]\nallow-process = true\n");
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn-timeout -- 50 sh -c 'sleep 5'<ret>",
    )
    .await;
    wait_process_status(&mut app, |message| message.contains("timed out")).await;
    assert!(status(&app).0.starts_with("exit:None;"));
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn -- sh -c 'sleep 5'<ret>",
    )
    .await;
    let id = status(&app)
        .0
        .strip_prefix("request:")
        .expect("pending request")
        .parse::<u32>()?;
    keys(&mut app, &format!(":plugin-run testplugin2/kill {id}<ret>")).await;
    wait_process_status(&mut app, |message| message.contains("process killed")).await;
    assert!(status(&app).0.starts_with("exit:None;"));
    assert!(
        !helix_term::plugins::with_host(|host| host.request_is_pending(
            "testplugin2",
            id,
            helix_plugin::PendingKind::Process
        ))
        .unwrap()
    );
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn /pure-no-such-program<ret>",
    )
    .await;
    wait_process_status(&mut app, |message| message.contains("failed to run")).await;
    assert_eq!(status(&app).1, Severity::Error);
    assert!(helix_term::plugins::with_host(|host| host
        .plugin("testplugin2")
        .unwrap()
        .disabled()
        .is_none())
    .unwrap());
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn process_grant_failures_never_start_a_request() -> anyhow::Result<()> {
    let (mut app, dir) = app_with_process_plugin("");
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn -- sh -c 'echo bad'<ret>",
    )
    .await;
    assert_error(&app, "process not granted");
    helix_term::plugins::load_with_settings(
        &mut app.editor,
        dir.path(),
        "[testplugin2]\nallow-process = ['cat']\n",
    )?;
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn -- sh -c 'echo bad'<ret>",
    )
    .await;
    assert_error(&app, "not granted");
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn reload_and_disable_cancel_owned_processes_and_ignore_late_results() -> anyhow::Result<()> {
    let settings = "[testplugin2]\nallow-process = true\n";
    let (mut app, dir) = app_with_process_plugin(settings);
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn -- sh -c 'sleep 5; echo late'<ret>",
    )
    .await;
    let old_id = status(&app)
        .0
        .strip_prefix("request:")
        .expect("pending request")
        .parse::<u32>()?;
    helix_term::plugins::load_with_settings(&mut app.editor, dir.path(), settings)?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    app.editor.reset_idle_timer();
    run_event_loop_until_idle(&mut app).await;
    assert_eq!(status(&app).0, "0.2 init:{}");
    assert!(
        !helix_term::plugins::with_host(|host| host.request_is_pending(
            "testplugin2",
            old_id,
            helix_plugin::PendingKind::Process
        ))
        .unwrap()
    );
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn -- sh -c 'sleep 5; echo late'<ret>",
    )
    .await;
    keys(&mut app, ":plugin-run testplugin2/panic<ret>").await;
    assert!(helix_term::plugins::with_host(|host| host
        .plugin("testplugin2")
        .unwrap()
        .disabled()
        .is_some())
    .unwrap());
    keys(&mut app, ":plugin-run text-tools/count<ret>").await;
    assert_eq!(status(&app).0, "document: 1 lines, 1 words, 4 chars");
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn nested_process_callback_edits_do_not_retrigger_plugin_events() -> anyhow::Result<()> {
    let settings = "[testplugin2]\nallow-process = true\n[testplugin]\nedit-on-change = true\n";
    let (mut app, _dir) = app_with_process_plugin(settings);
    keys(
        &mut app,
        ":plugin-run testplugin2/spawn-nested printf hi<ret>",
    )
    .await;
    wait_process_status(&mut app, |message| message == "nested process edited").await;
    for _ in 0..3 {
        deliver_events(&mut app).await;
    }
    assert_eq!(text(&app), "hiAbc\n");
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn reloaded_interactive_requests_are_cancelled_before_they_launch() -> anyhow::Result<()> {
    let settings = "[testplugin2]\nallow-process = true\n";
    let (mut app, dir) = app_with_process_plugin(settings);
    let marker_dir = tempfile::tempdir()?;
    let marker = marker_dir.path().join("launched");
    let root = dir.path().to_path_buf();
    let script = format!("printf launched > '{}'", marker.display());
    helix_term::job::dispatch_with_jobs(move |editor, _, jobs| {
        helix_term::plugins::run_command(
            editor,
            jobs,
            "testplugin2/interactive",
            vec!["sh".into(), "-c".into(), script],
        )
        .unwrap();
        // Reload before the queued terminal request can be installed.
        helix_term::plugins::load_with_settings(editor, &root, settings).unwrap();
    })
    .await;
    keys(&mut app, "").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    app.editor.reset_idle_timer();
    run_event_loop_until_idle(&mut app).await;
    assert!(!marker.exists());
    assert_eq!(status(&app).0, "0.2 init:{}");
    Ok(())
}
