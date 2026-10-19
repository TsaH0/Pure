use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use helix_core::{Rope, Selection};

use super::*;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn testplugin_wasm() -> PathBuf {
    fixture("tests/fixtures/testplugin.wasm")
}

fn text_tools_wasm() -> PathBuf {
    fixture("tests/fixtures/text-tools-v0.1.wasm")
}

/// Compiled components are cached here so not every test compiles them.
fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("helix-plugin-test-cache")
}

/// Write a plugin directory with the given manifest fields.
fn plugin_dir(root: &Path, name: &str, wasm: &Path, permissions: &[&str]) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(wasm, dir.join("plugin.wasm")).unwrap();
    let permissions = permissions
        .iter()
        .map(|p| format!("\"{p}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.join("plugin.toml"),
        format!("name = \"{name}\"\nversion = \"1.0.0\"\napi = \"0.1\"\npermissions = [{permissions}]\n"),
    )
    .unwrap();
    dir
}

const ALL: &[&str] = &["read-buffers", "edit-buffers", "open-files", "run-commands"];

fn host() -> PluginHost {
    host_with_limits(Limits::default())
}

fn host_with_limits(limits: Limits) -> PluginHost {
    PluginHost::with_limits(Some(cache_dir()), limits).unwrap()
}

/// A host with `testplugin` loaded with all permissions and `config`.
fn testplugin(config: &str) -> (PluginHost, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let mut host = host();
    let dir = plugin_dir(root.path(), "testplugin", &testplugin_wasm(), ALL);
    host.load(&dir, config).unwrap();
    (host, root)
}

/// One document with id 1, selected range `anchor..head`.
fn snapshot(text: &str, anchor: usize, head: usize) -> Snapshot {
    let mut documents = BTreeMap::new();
    documents.insert(
        1,
        DocumentSnapshot {
            text: Rope::from(text),
            path: Some(PathBuf::from("/tmp/file.rs")),
            language: Some("rust".into()),
            selection: Some(Selection::single(anchor, head)),
        },
    );
    Snapshot {
        documents,
        current: Some(1),
        mode: Mode::Normal,
    }
}

/// Apply the edit actions to `text`, like the editor does.
fn apply(text: &str, actions: &[Action]) -> String {
    let mut rope = Rope::from(text);
    for action in actions {
        if let Action::Edit { transaction, .. } = action {
            assert!(transaction.apply(&mut rope));
        }
    }
    rope.to_string()
}

fn statuses(actions: &[Action]) -> Vec<String> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::Status(message) => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn run(
    host: &mut PluginHost,
    command: &str,
    args: &[&str],
    snapshot: Snapshot,
) -> anyhow::Result<Vec<Action>> {
    host.run_command(
        "testplugin",
        command,
        args.iter().map(|arg| arg.to_string()).collect(),
        snapshot,
    )
}

// -- manifests

#[test]
fn manifest_parsing() {
    let manifest = Manifest::parse(
        "name = \"my-plugin_2\"\nversion = \"0.1.0\"\napi = \"0.1\"\npermissions = [\"read-buffers\", \"run-commands\"]",
    )
    .unwrap();
    assert_eq!(manifest.name, "my-plugin_2");
    assert_eq!(manifest.wasm, PathBuf::from("plugin.wasm"));
    assert_eq!(
        manifest.permissions,
        vec![Permission::ReadBuffers, Permission::RunCommands]
    );

    let err = |source: &str| Manifest::parse(source).unwrap_err().to_string();
    assert!(
        err("name = \"Bad Name\"\nversion = \"1\"\napi = \"0.1\"").contains("invalid plugin name")
    );
    assert!(err("name = \"x\"\nversion = \"1\"\napi = \"9.9\"").contains("needs plugin API 9.9"));
    assert!(
        err("name = \"x\"\nversion = \"1\"\napi = \"0.1\"\nunknown = 1").contains("unknown field")
    );
    assert!(
        err("name = \"x\"\nversion = \"1\"\napi = \"0.1\"\npermissions = [\"network\"]")
            .contains("unknown variant")
    );
    assert!(err("version = \"1\"\napi = \"0.1\"").contains("missing field"));
}

#[test]
fn missing_wasm_file() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("nowasm");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "name = \"nowasm\"\nversion = \"1\"\napi = \"0.1\"",
    )
    .unwrap();
    let err = host().load(&dir, "{}").unwrap_err();
    assert!(format!("{err:#}").contains("plugin.wasm"), "{err:#}");
}

#[test]
fn invalid_wasm_file() {
    let root = tempfile::tempdir().unwrap();
    let garbage = root.path().join("garbage.wasm");
    std::fs::write(&garbage, b"not wasm at all").unwrap();
    let dir = plugin_dir(root.path(), "garbage", &garbage, &[]);
    let mut host = PluginHost::new(None).unwrap();
    assert!(host.load(&dir, "{}").is_err());
    assert!(host.plugins().is_empty());
}

// -- loading

#[test]
fn load_registers_commands_and_events() {
    let (host, _root) = testplugin("{}");
    let plugin = host.plugin("testplugin").unwrap();
    assert!(plugin.commands().iter().any(|c| c.name == "status"));
    assert!(plugin.subscribes_to(EventKind::DocumentOpen));
    assert!(plugin.subscribes_to(EventKind::ModeSwitch));
    assert!(host.wants(EventKind::DocumentChange));
    assert!(host
        .commands()
        .iter()
        .any(|(plugin, command)| plugin == "testplugin" && command.name == "loop"));
    let info = host.info();
    assert_eq!(info[0].version, "1.0.0");
    assert_eq!(info[0].permissions.len(), 4);
}

#[test]
fn init_actions_are_returned() {
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(root.path(), "testplugin", &testplugin_wasm(), ALL);
    let actions = host().load(&dir, "{\"init-status\": true}").unwrap();
    assert_eq!(statuses(&actions), vec!["testplugin loaded"]);
}

#[test]
fn init_errors_prevent_loading() {
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(root.path(), "testplugin", &testplugin_wasm(), ALL);
    let mut host = host();
    let err = host.load(&dir, "{\"fail-init\": true}").unwrap_err();
    assert!(err.to_string().contains("init failed on purpose"), "{err}");
    assert!(host.plugins().is_empty());

    let err = host.load(&dir, "{\"bad-command-name\": true}").unwrap_err();
    assert!(err.to_string().contains("invalid command name"), "{err}");
    assert!(host.plugins().is_empty());
}

#[test]
fn duplicate_plugins_are_rejected() {
    let (mut host, root) = testplugin("{}");
    let err = host
        .load(&root.path().join("testplugin"), "{}")
        .unwrap_err();
    assert!(err.to_string().contains("already loaded"));
    assert_eq!(host.plugins().len(), 1);
}

#[test]
fn load_all_skips_disabled_and_reports_broken_plugins() {
    let root = tempfile::tempdir().unwrap();
    plugin_dir(root.path(), "one", &testplugin_wasm(), ALL);
    plugin_dir(root.path(), "two", &testplugin_wasm(), ALL);
    plugin_dir(root.path(), "off", &testplugin_wasm(), ALL);
    let broken = root.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("plugin.toml"), "this is not toml =").unwrap();
    // Not a plugin: no manifest.
    std::fs::create_dir_all(root.path().join("not-a-plugin")).unwrap();

    let mut host = host();
    let (_actions, errors) = host.load_all(root.path(), |name| {
        (name != "off").then(|| "{}".to_string())
    });
    let mut names: Vec<_> = host
        .plugins()
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["one", "two"]);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(format!("{:#}", errors[0]).contains("broken"));
}

#[test]
fn load_all_with_missing_directory() {
    let mut host = host();
    let (actions, errors) = host.load_all(Path::new("/nonexistent/helix/plugins"), |_| {
        Some("{}".into())
    });
    assert!(actions.is_empty() && errors.is_empty() && host.plugins().is_empty());
}

#[test]
fn compiled_plugins_are_cached() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    let dir = plugin_dir(root.path(), "testplugin", &testplugin_wasm(), ALL);

    let mut first = PluginHost::new(Some(cache.clone())).unwrap();
    first.load(&dir, "{}").unwrap();
    let files: Vec<_> = std::fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].extension().unwrap(), "cwasm");

    // Loading again uses the cache.
    let mut second = PluginHost::new(Some(cache.clone())).unwrap();
    second.load(&dir, "{}").unwrap();
    assert!(
        statuses(&run(&mut second, "status", &["cached"], snapshot("", 0, 0)).unwrap())
            == ["cached"]
    );

    // A corrupt cache file is ignored.
    std::fs::write(&files[0], b"corrupt").unwrap();
    let mut third = PluginHost::new(Some(cache)).unwrap();
    third.load(&dir, "{}").unwrap();
    assert_eq!(
        statuses(&run(&mut third, "status", &["recompiled"], snapshot("", 0, 0)).unwrap()),
        ["recompiled"]
    );
}

// -- commands and the host API

#[test]
fn unknown_plugin_and_command() {
    let (mut host, _root) = testplugin("{}");
    let err = host
        .run_command("nope", "status", vec![], Snapshot::default())
        .unwrap_err();
    assert!(err.to_string().contains("no plugin named 'nope'"));
    let err = run(&mut host, "nope", &[], Snapshot::default()).unwrap_err();
    assert!(err.to_string().contains("has no command 'nope'"));
}

#[test]
fn status_and_errors() {
    let (mut host, _root) = testplugin("{}");
    let actions = run(
        &mut host,
        "status",
        &["hello", "world"],
        Snapshot::default(),
    )
    .unwrap();
    assert_eq!(statuses(&actions), vec!["hello world"]);

    let actions = run(&mut host, "set-error", &["oops"], Snapshot::default()).unwrap();
    assert!(matches!(&actions[..], [Action::Error(message)] if message == "oops"));

    // A command returning an error doesn't disable the plugin.
    let err = run(&mut host, "fail", &["x"], Snapshot::default()).unwrap_err();
    assert_eq!(err.to_string(), "testplugin/fail: failed: x");
    assert!(host.plugin("testplugin").unwrap().disabled().is_none());
    assert!(run(
        &mut host,
        "status",
        &["still", "works"],
        Snapshot::default()
    )
    .is_ok());
}

#[test]
fn reading_editor_state() {
    let (mut host, _root) = testplugin("{}");
    let mut snap = snapshot("héllo\nworld\n", 2, 5);
    snap.mode = Mode::Insert;
    let actions = run(&mut host, "info", &[], snap).unwrap();
    assert_eq!(
        statuses(&actions),
        vec!["path=/tmp/file.rs lang=rust mode=insert docs=1 sel=2-5 len=12"]
    );

    // No current document.
    let err = run(&mut host, "info", &[], Snapshot::default()).unwrap_err();
    assert!(err.to_string().contains("no document"));
}

#[test]
fn primary_selection_comes_first() {
    let (mut host, _root) = testplugin("{}");
    let mut snap = snapshot("abcdef", 0, 0);
    let ranges = [helix_core::Range::new(0, 1), helix_core::Range::new(3, 4)];
    snap.documents.get_mut(&1).unwrap().selection = Some(Selection::new(ranges.to_vec().into(), 1));
    let actions = run(&mut host, "info", &[], snap).unwrap();
    assert!(
        statuses(&actions)[0].contains("sel=3-4,0-1"),
        "{:?}",
        statuses(&actions)
    );
}

#[test]
fn editing() {
    let (mut host, _root) = testplugin("{}");
    let actions = run(&mut host, "insert", &["XY"], snapshot("hello", 2, 2)).unwrap();
    assert_eq!(apply("hello", &actions), "heXYllo");

    // Unicode offsets are chars, not bytes.
    let actions = run(&mut host, "insert", &["→"], snapshot("éé", 1, 1)).unwrap();
    assert_eq!(apply("éé", &actions), "é→é");

    let actions = run(
        &mut host,
        "replace-all",
        &["new", "text"],
        snapshot("old\n", 0, 0),
    )
    .unwrap();
    assert_eq!(apply("old\n", &actions), "new text");
}

#[test]
fn edits_see_earlier_edits_of_the_same_call() {
    let (mut host, _root) = testplugin("{}");
    let actions = run(&mut host, "two-edits", &[], snapshot("xyz", 0, 0)).unwrap();
    assert_eq!(apply("xyz", &actions), "baxyz");
    assert_eq!(statuses(&actions), vec!["start=ba"]);
}

#[test]
fn selections() {
    let (mut host, _root) = testplugin("{}");
    let actions = run(&mut host, "select-all", &[], snapshot("hello", 0, 0)).unwrap();
    match &actions[..] {
        [Action::SetSelection { doc: 1, selection }] => {
            assert_eq!(selection.primary(), helix_core::Range::new(0, 5));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn invalid_edits_are_rejected() {
    let (mut host, _root) = testplugin("{}");
    let err = run(&mut host, "bad-edit", &[], snapshot("abc", 0, 0)).unwrap_err();
    assert!(
        err.to_string().contains("for a document of 3 chars"),
        "{err}"
    );
    let err = run(&mut host, "overlap", &[], snapshot("abcdef", 0, 0)).unwrap_err();
    assert!(err.to_string().contains("overlap"), "{err}");
    // The plugin stays usable.
    assert!(host.plugin("testplugin").unwrap().disabled().is_none());
}

#[test]
fn open_and_run_commands() {
    let (mut host, _root) = testplugin("{}");
    let actions = run(&mut host, "open", &["src/main.rs"], Snapshot::default()).unwrap();
    assert!(matches!(&actions[..], [Action::OpenFile(path)] if path == Path::new("src/main.rs")));
    let actions = run(&mut host, "run", &[":write"], Snapshot::default()).unwrap();
    assert!(matches!(&actions[..], [Action::RunCommand(command)] if command == ":write"));
}

#[test]
fn permissions_are_enforced() {
    let root = tempfile::tempdir().unwrap();
    let mut host = host();
    // Read-only plugin.
    let dir = plugin_dir(
        root.path(),
        "testplugin",
        &testplugin_wasm(),
        &["read-buffers"],
    );
    host.load(&dir, "{}").unwrap();

    assert!(run(&mut host, "read", &[], snapshot("abc", 0, 0)).is_ok());
    for (command, args, permission) in [
        ("insert", vec!["x"], "edit-buffers"),
        ("select-all", vec![], "edit-buffers"),
        ("open", vec!["file"], "open-files"),
        ("run", vec![":write"], "run-commands"),
    ] {
        let err = run(&mut host, command, &args, snapshot("abc", 0, 0)).unwrap_err();
        assert!(
            err.to_string()
                .contains(&format!("permission denied: add \"{permission}\"")),
            "{command}: {err}"
        );
    }

    // No permissions at all: reading fails too, status messages still work.
    let mut host = PluginHost::new(Some(cache_dir())).unwrap();
    let dir = plugin_dir(root.path(), "testplugin", &testplugin_wasm(), &[]);
    // Reuse the directory with a manifest without permissions.
    host.load(&dir, "{}").unwrap();
    let err = run(&mut host, "read", &[], snapshot("abc", 0, 0)).unwrap_err();
    assert!(err.to_string().contains("\"read-buffers\""), "{err}");
    assert!(run(&mut host, "status", &["ok"], Snapshot::default()).is_ok());
}

// -- misbehaving plugins

fn assert_disabled(host: &mut PluginHost, reason: &str) {
    let plugin = host.plugin("testplugin").unwrap();
    let disabled = plugin.disabled().expect("plugin should be disabled");
    assert!(disabled.contains(reason), "{disabled}");
    // Disabled plugins don't run anymore.
    let err = run(host, "status", &["x"], Snapshot::default()).unwrap_err();
    assert!(err.to_string().contains("is disabled"), "{err}");
    assert!(!host
        .commands()
        .iter()
        .any(|(plugin, _)| plugin == "testplugin"));
}

#[test]
fn infinite_loops_time_out() {
    let limits = Limits {
        command: Duration::from_millis(200),
        ..Limits::default()
    };
    let root = tempfile::tempdir().unwrap();
    let mut host = host_with_limits(limits);
    host.load(
        &plugin_dir(root.path(), "testplugin", &testplugin_wasm(), ALL),
        "{}",
    )
    .unwrap();
    // A second plugin is not affected.
    host.load(
        &plugin_dir(root.path(), "other", &testplugin_wasm(), ALL),
        "{}",
    )
    .unwrap();

    let start = Instant::now();
    let err = run(&mut host, "loop", &[], Snapshot::default()).unwrap_err();
    let elapsed = start.elapsed();
    assert!(err.to_string().contains("took longer than 200 ms"), "{err}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert_disabled(&mut host, "took longer than");

    let actions = host
        .run_command("other", "status", vec!["alive".into()], Snapshot::default())
        .unwrap();
    assert_eq!(statuses(&actions), vec!["alive"]);
}

#[test]
fn panics_disable_the_plugin() {
    let (mut host, _root) = testplugin("{}");
    let err = run(&mut host, "panic", &[], Snapshot::default()).unwrap_err();
    assert!(err.to_string().contains("crashed"), "{err}");
    assert_disabled(&mut host, "crashed");
}

#[test]
fn memory_is_limited() {
    let limits = Limits {
        memory: 64 << 20,
        ..Limits::default()
    };
    let root = tempfile::tempdir().unwrap();
    let mut host = host_with_limits(limits);
    host.load(
        &plugin_dir(root.path(), "testplugin", &testplugin_wasm(), ALL),
        "{}",
    )
    .unwrap();
    let err = run(&mut host, "alloc", &[], Snapshot::default()).unwrap_err();
    assert!(err.to_string().contains("crashed"), "{err}");
    assert_disabled(&mut host, "crashed");
}

#[test]
fn actions_of_failed_calls_are_dropped() {
    let (mut host, _root) = testplugin("{}");
    // `two-edits` succeeds, `bad-edit` after an earlier successful call fails
    // and returns nothing.
    assert!(run(&mut host, "bad-edit", &[], snapshot("abc", 0, 0)).is_err());
    let actions = run(&mut host, "status", &["clean"], Snapshot::default()).unwrap();
    assert_eq!(actions.len(), 1);
}

// -- events

#[test]
fn events_are_delivered_to_subscribers() {
    let (mut host, _root) = testplugin("{\"status-on-open\": true}");
    let snap = snapshot("abc", 0, 0);
    let results = host.dispatch(Event::DocumentOpen(1), &snap);
    assert_eq!(results.len(), 1);
    let (name, actions) = &results[0];
    assert_eq!(name, "testplugin");
    assert_eq!(
        statuses(actions.as_ref().unwrap()),
        vec!["opened /tmp/file.rs"]
    );

    host.dispatch(Event::DocumentChange(1), &snap);
    host.dispatch(
        Event::ModeSwitch {
            old: Mode::Normal,
            new: Mode::Insert,
        },
        &snap,
    );
    let actions = run(&mut host, "events", &[], Snapshot::default()).unwrap();
    assert_eq!(
        statuses(&actions),
        vec!["events: open:1,change:1,mode:normal>insert"]
    );
}

#[test]
fn unsubscribed_plugins_get_no_events() {
    let (mut host, _root) = testplugin("{\"no-events\": true}");
    assert!(!host.wants(EventKind::DocumentOpen));
    assert!(host
        .dispatch(Event::DocumentOpen(1), &snapshot("", 0, 0))
        .is_empty());
    let actions = run(&mut host, "events", &[], Snapshot::default()).unwrap();
    assert_eq!(statuses(&actions), vec!["events: "]);
}

#[test]
fn event_handlers_can_edit_but_not_run_commands() {
    let (mut host, _root) = testplugin("{\"edit-on-change\": true, \"run-on-open\": true}");
    let snap = snapshot("abc", 0, 0);
    let results = host.dispatch(Event::DocumentChange(1), &snap);
    let actions = results[0].1.as_ref().unwrap();
    assert_eq!(apply("abc", actions), "abc!");

    host.dispatch(Event::DocumentOpen(1), &snap);
    let actions = run(&mut host, "events", &[], Snapshot::default()).unwrap();
    assert!(
        statuses(&actions)[0]
            .contains("run-error:run-command is only available while running a plugin command"),
        "{:?}",
        statuses(&actions)
    );
}

#[test]
fn slow_event_handlers_are_disabled() {
    let limits = Limits {
        event: Duration::from_millis(100),
        ..Limits::default()
    };
    let root = tempfile::tempdir().unwrap();
    let mut host = host_with_limits(limits);
    host.load(
        &plugin_dir(root.path(), "testplugin", &testplugin_wasm(), ALL),
        "{\"loop-on-change\": true}",
    )
    .unwrap();
    let results = host.dispatch(Event::DocumentChange(1), &snapshot("", 0, 0));
    let err = results[0].1.as_ref().unwrap_err();
    assert!(err.to_string().contains("took longer than 100 ms"), "{err}");
    assert_disabled(&mut host, "took longer than");
    assert!(!host.wants(EventKind::DocumentChange));
    // Not called anymore.
    assert!(host
        .dispatch(Event::DocumentChange(1), &snapshot("", 0, 0))
        .is_empty());
}

// -- the example plugin

fn text_tools(api: &str) -> (PluginHost, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let mut host = host();
    let dir = root.path().join("text-tools");
    std::fs::create_dir_all(&dir).unwrap();
    let wasm = if api == "0.1" {
        text_tools_wasm()
    } else {
        fixture("examples/text-tools/plugin.wasm")
    };
    std::fs::copy(wasm, dir.join("plugin.wasm")).unwrap();
    let manifest = std::fs::read_to_string(fixture("examples/text-tools/plugin.toml"))
        .unwrap()
        .replace("api = \"0.2\"", &format!("api = \"{api}\""));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    host.load(&dir, "{}").unwrap();
    (host, root)
}

fn text_tools_run(host: &mut PluginHost, command: &str, snapshot: Snapshot) -> Vec<Action> {
    host.run_command("text-tools", command, vec![], snapshot)
        .unwrap()
}

#[test]
fn example_text_tools() {
    check_text_tools("0.2");
}

#[test]
fn legacy_example_text_tools() {
    check_text_tools("0.1");
}

fn check_text_tools(api: &str) {
    let (mut host, _root) = text_tools(api);
    let names: Vec<_> = host.commands().into_iter().map(|(_, c)| c.name).collect();
    assert_eq!(names, ["upper", "lower", "sort-lines", "count"]);

    let actions = text_tools_run(&mut host, "upper", snapshot("hello world", 0, 5));
    assert_eq!(apply("hello world", &actions), "HELLO world");
    assert!(matches!(actions.last(), Some(Action::SetSelection { .. })));

    let actions = text_tools_run(&mut host, "lower", snapshot("ÄBC DEF", 0, 3));
    assert_eq!(apply("ÄBC DEF", &actions), "äbc DEF");

    // A cursor changes the char under it.
    let actions = text_tools_run(&mut host, "upper", snapshot("abc", 1, 1));
    assert_eq!(apply("abc", &actions), "aBc");

    // Whole document when nothing is selected.
    let text = "pear\napple\nfig\n";
    let actions = text_tools_run(&mut host, "sort-lines", snapshot(text, 0, 0));
    assert_eq!(apply(text, &actions), "apple\nfig\npear\n");
    assert_eq!(statuses(&actions), vec!["sorted 3 lines"]);

    // Only the selected lines.
    let text = "z\nc\nb\na\n";
    // `c` up to and including the `a`.
    let actions = text_tools_run(&mut host, "sort-lines", snapshot(text, 2, 7));
    assert_eq!(apply(text, &actions), "z\na\nb\nc\n");

    let actions = text_tools_run(&mut host, "count", snapshot("one two\nthree\n", 0, 0));
    assert_eq!(
        statuses(&actions),
        vec!["document: 2 lines, 3 words, 14 chars"]
    );
    // A one-char selection is Helix's cursor.
    let actions = text_tools_run(&mut host, "count", snapshot("one two\nthree\n", 3, 4));
    assert_eq!(
        statuses(&actions),
        vec!["document: 2 lines, 3 words, 14 chars"]
    );
    let actions = text_tools_run(&mut host, "sort-lines", snapshot("b\na\n", 0, 1));
    assert_eq!(apply("b\na\n", &actions), "a\nb\n");

    let actions = text_tools_run(&mut host, "count", snapshot("one two\nthree\n", 0, 7));
    assert_eq!(
        statuses(&actions),
        vec!["selection: 1 lines, 2 words, 7 chars"]
    );
}

#[test]
fn example_needs_no_extra_permissions() {
    let manifest = Manifest::parse(
        &std::fs::read_to_string(fixture("examples/text-tools/plugin.toml")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest.permissions,
        vec![Permission::ReadBuffers, Permission::EditBuffers]
    );
}

// -- running inside the editor's async runtime

/// The editor calls plugins from within its Tokio runtime; WASI must still
/// work there (a panicking plugin writes its message to stderr through WASI).
#[tokio::test(flavor = "multi_thread")]
async fn plugins_work_inside_a_tokio_runtime() {
    let (mut host, _root) = testplugin("{}");
    let actions = run(&mut host, "status", &["in tokio"], Snapshot::default()).unwrap();
    assert_eq!(statuses(&actions), vec!["in tokio"]);
    let err = run(&mut host, "panic", &[], Snapshot::default()).unwrap_err();
    assert!(err.to_string().contains("crashed"), "{err}");
}

#[tokio::test(flavor = "current_thread")]
async fn plugins_work_inside_a_single_threaded_runtime() {
    let (mut host, _root) = testplugin("{}");
    let err = run(&mut host, "panic", &[], Snapshot::default()).unwrap_err();
    assert!(err.to_string().contains("crashed"), "{err}");
}

#[test]
fn both_api_versions_share_host_state() {
    let (mut host, root) = testplugin("{}");
    let dir = plugin_dir(
        root.path(),
        "testplugin2",
        &fixture("tests/fixtures/testplugin2.wasm"),
        ALL,
    );
    let manifest_path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.2\"");
    std::fs::write(manifest_path, manifest).unwrap();
    let actions = host.load(&dir, "{\"answer\":42}").unwrap();
    assert_eq!(statuses(&actions), ["0.2 init:{\"answer\":42}"]);
    assert_eq!(host.plugins().len(), 2);
    let actions = host
        .run_command("testplugin2", "inspect", vec![], snapshot("héllo", 0, 1))
        .unwrap();
    assert_eq!(apply("héllo", &actions), "v2:héllo");
    assert_eq!(statuses(&actions), ["héllo:Mode::Normal"]);
    let results = host.dispatch(
        Event::ModeSwitch {
            old: Mode::Normal,
            new: Mode::Insert,
        },
        &Snapshot::default(),
    );
    let actions = &results
        .into_iter()
        .find(|(name, _)| name == "testplugin2")
        .unwrap()
        .1
        .unwrap();
    assert_eq!(statuses(actions), ["Mode::Normal->Mode::Insert"]);
    assert!(run(&mut host, "status", &["old"], Snapshot::default()).is_ok());
}

#[test]
fn manifest_selects_the_component_world() {
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(root.path(), "wrong-world", &testplugin_wasm(), ALL);
    let path = dir.join("plugin.toml");
    let source = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.2\"");
    std::fs::write(path, source).unwrap();
    assert!(host()
        .load(&dir, "{}")
        .unwrap_err()
        .to_string()
        .contains("failed to start"));
    let err = Manifest::parse("name = \"x\"\nversion = \"1\"\napi = \"9.9\"").unwrap_err();
    assert!(err.to_string().contains("supports 0.1 and 0.2"));
}

fn testplugin2() -> (PluginHost, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin2",
        &fixture("tests/fixtures/testplugin2.wasm"),
        ALL,
    );
    let path = dir.join("plugin.toml");
    let source = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.2\"");
    std::fs::write(path, source).unwrap();
    let mut host = host();
    host.load(&dir, "{}").unwrap();
    (host, root)
}

fn process_done(id: u32, stdout: &str) -> Callback {
    Callback::ProcessDone {
        id,
        result: Ok(ProcessOutput {
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }),
    }
}

#[test]
fn callbacks_are_delivered_once_and_check_their_kind() {
    let (mut host, _root) = testplugin2();
    let id = arm_request(&mut host, PendingKind::Process).unwrap();
    let wrong_kind = Callback::PickerDone {
        id,
        result: PickerResult::Cancelled,
    };
    assert!(host
        .callback("testplugin2", wrong_kind, &Snapshot::default())
        .unwrap()
        .is_empty());
    let actions = host
        .callback(
            "testplugin2",
            process_done(id, "hello"),
            &Snapshot::default(),
        )
        .unwrap();
    let messages = statuses(&actions);
    assert_eq!(messages.len(), 1);
    assert!(
        messages[0].starts_with(&format!("process:{id}:")),
        "{messages:?}"
    );
    assert!(messages[0].contains("hello"), "{messages:?}");
    assert!(host
        .callback(
            "testplugin2",
            process_done(id, "duplicate"),
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
    assert!(host
        .callback("missing", process_done(id, "late"), &Snapshot::default())
        .unwrap()
        .is_empty());
    let picker = arm_request(&mut host, PendingKind::Picker).unwrap();
    let actions = host
        .callback(
            "testplugin2",
            Callback::PickerDone {
                id: picker,
                result: PickerResult::Cancelled,
            },
            &Snapshot::default(),
        )
        .unwrap();
    assert!(statuses(&actions)[0].starts_with(&format!("picker:{picker}:")));
}

#[test]
fn request_ids_are_not_reused_after_clear_or_host_recreation() {
    let (mut host, root) = testplugin2();
    let old = arm_request(&mut host, PendingKind::Process).unwrap();
    host.clear();
    host.load(&root.path().join("testplugin2"), "{}").unwrap();
    let new = arm_request(&mut host, PendingKind::Process).unwrap();
    assert_ne!(old, new);
    assert!(host
        .callback(
            "testplugin2",
            process_done(old, "stale"),
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
    drop(host);
    let (mut host, _root) = testplugin2();
    let recreated = arm_request(&mut host, PendingKind::Process).unwrap();
    assert_ne!(new, recreated);
    assert!(host
        .callback(
            "testplugin2",
            process_done(new, "stale"),
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
}

#[test]
fn ui_is_restricted_to_commands_and_callbacks() {
    use v0_2::helix::plugin::{process, ui};
    let (mut host, _root) = testplugin2();
    let state = host.plugins[0].store.data_mut();
    for context in [
        CallContext::Init,
        CallContext::Event,
        CallContext::Command,
        CallContext::Callback,
    ] {
        state.context = context;
        let command = state.run_command("goto_line_end".into());
        let interactive = process::Host::run_interactive(
            state,
            vec!["true".into()],
            process::InteractiveOptions {
                cwd: None,
                chooser_file: false,
                wait_for_key: false,
            },
        );
        let picker = ui::Host::show_picker(
            state,
            ui::Picker {
                title: "Test".into(),
                columns: vec![],
                items: vec![],
                initial: None,
            },
        );
        if context.allows_ui() {
            assert!(command.is_ok());
            assert!(interactive.unwrap_err().contains("not implemented"));
            assert!(picker.unwrap_err().contains("not implemented"));
        } else {
            assert!(command.unwrap_err().contains("only available"));
            assert!(interactive.unwrap_err().contains("only available"));
            assert!(picker.unwrap_err().contains("only available"));
        }
    }
}

#[test]
fn legacy_plugins_cannot_reserve_callbacks() {
    let (mut host, _root) = testplugin("{}");
    assert!(host
        .register_request("testplugin", PendingKind::Process)
        .is_err());
    assert!(host
        .callback(
            "testplugin",
            process_done(123, "ignored"),
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
}

fn arm_request(host: &mut PluginHost, kind: PendingKind) -> anyhow::Result<u32> {
    let id = host.register_request("testplugin2", kind)?;
    let command = match kind {
        PendingKind::Process => "arm-process",
        PendingKind::Picker => "arm-picker",
        PendingKind::Prompt => unreachable!(),
    };
    host.run_command(
        "testplugin2",
        command,
        vec![id.to_string()],
        Snapshot::default(),
    )?;
    Ok(id)
}

#[test]
fn sdk_callbacks_use_fresh_snapshots_and_can_run_commands() {
    let (mut host, _root) = testplugin2();
    let id = arm_request(&mut host, PendingKind::Process).unwrap();
    let actions = host
        .callback(
            "testplugin2",
            process_done(id, "read"),
            &snapshot("fresh snapshot", 0, 1),
        )
        .unwrap();
    assert_eq!(statuses(&actions), ["fresh snapshot"]);
    let id = arm_request(&mut host, PendingKind::Process).unwrap();
    let actions = host
        .callback(
            "testplugin2",
            process_done(id, "command"),
            &Snapshot::default(),
        )
        .unwrap();
    assert!(matches!(&actions[..], [Action::RunCommand(command)] if command == "goto_line_end"));
    let id = arm_request(&mut host, PendingKind::Process).unwrap();
    let actions = host
        .callback(
            "testplugin2",
            process_done(id, "fail"),
            &Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::Error(message)] if message == "plugin callback: callback failed on purpose")
    );
}

#[test]
fn callback_deadlines_disable_the_plugin_and_clear_pending_requests() {
    let (mut host, _root) = testplugin2();
    host.limits.command = Duration::from_millis(100);
    let id = arm_request(&mut host, PendingKind::Process).unwrap();
    let other = arm_request(&mut host, PendingKind::Picker).unwrap();
    let err = host
        .callback(
            "testplugin2",
            process_done(id, "spin"),
            &Snapshot::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("took longer than"));
    assert!(host.plugins[0].store.data().pending.is_empty());
    assert!(host.plugin("testplugin2").unwrap().disabled().is_some());
    assert!(host
        .callback(
            "testplugin2",
            Callback::PickerDone {
                id: other,
                result: PickerResult::Cancelled
            },
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
}

#[test]
fn sdk_ui_requests_are_refused_from_wasm_event_handlers() {
    let (mut host, root) = testplugin2();
    host.clear();
    host.load(&root.path().join("testplugin2"), "{\"event-ui\":true}")
        .unwrap();
    let results = host.dispatch(Event::DocumentOpen(1), &snapshot("text", 0, 1));
    let actions = results
        .into_iter()
        .find(|(name, _)| name == "testplugin2")
        .unwrap()
        .1
        .unwrap();
    let messages = statuses(&actions);
    assert_eq!(messages.len(), 3);
    assert!(messages[0].starts_with("run-interactive is only available"));
    assert!(messages[1].starts_with("show-picker is only available"));
    assert!(messages[2].starts_with("run-command is only available"));
}
