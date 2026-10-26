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
            modified: true,
            owner: None,
            revision: 0,
            language_servers: Vec::new(),
        },
    );
    Snapshot {
        documents,
        current: Some(1),
        mode: Mode::Normal,
        workspace: Some(PathBuf::from("/work")),
        cwd: PathBuf::from("/work/src"),
        config_dir: PathBuf::from("/config"),
        registers: BTreeMap::new(),
        panels: Vec::new(),
        completion_menu: None,
        views: vec![crate::v05::ViewInfo {
            id: 1,
            document: 1,
            area: (0, 0, 80, 24),
            first_line: 0,
            last_line: 2,
            focused: true,
        }],
        ui: crate::v05::UiState::default(),
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
    for wasm in ["../../victim.wasm", "/tmp/x.wasm", "dir/x.wasm", ".", ""] {
        let source = format!("name = \"x\"\nversion = \"1\"\napi = \"0.1\"\nwasm = \"{wasm}\"");
        assert!(
            err(&source).contains("wasm must be a file next to"),
            "{wasm}"
        );
    }
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
        (name != "off").then(|| PluginSettings::new("{}"))
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
        Some(PluginSettings::new("{}"))
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
    let wasm = match api {
        "0.1" => text_tools_wasm(),
        "0.2" => fixture("tests/fixtures/text-tools-v0.2.wasm"),
        _ => fixture("examples/text-tools/plugin.wasm"),
    };
    std::fs::copy(wasm, dir.join("plugin.wasm")).unwrap();
    let mut manifest: toml::Table = toml::from_str(
        &std::fs::read_to_string(fixture("examples/text-tools/plugin.toml")).unwrap(),
    )
    .unwrap();
    manifest.insert("api".into(), toml::Value::String(api.into()));
    std::fs::write(dir.join("plugin.toml"), toml::to_string(&manifest).unwrap()).unwrap();
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
fn sdk_05_example_text_tools() {
    check_text_tools("0.5");
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
        &fixture("tests/fixtures/testplugin2-v0.2.wasm"),
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
    let err = format!("{:#}", host().load(&dir, "{}").unwrap_err());
    assert!(err.contains("built for another API version"), "{err}");
    let err = Manifest::parse("name = \"x\"\nversion = \"1\"\napi = \"9.9\"").unwrap_err();
    assert!(err.to_string().contains("supports 0.1 to 0.5"));
}

fn testplugin2() -> (PluginHost, tempfile::TempDir) {
    testplugin2_with(ALL)
}

fn testplugin2_with(permissions: &[&str]) -> (PluginHost, tempfile::TempDir) {
    testplugin2_component(
        permissions,
        &fixture("tests/fixtures/testplugin2-v0.2.wasm"),
    )
}

fn testplugin2_component(
    permissions: &[&str],
    component: &Path,
) -> (PluginHost, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(root.path(), "testplugin2", component, permissions);
    let path = dir.join("plugin.toml");
    let source = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.2\"");
    std::fs::write(path, source).unwrap();
    let mut host = host();
    host.load(&dir, "{}").unwrap();
    (host, root)
}

#[test]
fn fresh_sdk_02_rolls_back_failed_picker_item_changes() {
    let (mut host, _root) = testplugin2_component(ALL, &fixture("tests/fixtures/testplugin2.wasm"));
    for failure in ["push-items-fail", "set-items-fail", "mixed-items-fail"] {
        let actions = host
            .run_command(
                "testplugin2",
                "data-picker",
                args(&["first", "second"]),
                Snapshot::default(),
            )
            .unwrap();
        let id = actions
            .iter()
            .find_map(|action| match action {
                Action::ShowPicker { id, .. } => Some(*id),
                _ => None,
            })
            .unwrap();
        assert!(host
            .run_command(
                "testplugin2",
                failure,
                vec![id.to_string()],
                Snapshot::default()
            )
            .is_err());
        host.run_command(
            "testplugin2",
            "push-items",
            vec![id.to_string()],
            Snapshot::default(),
        )
        .unwrap();
        let actions = host
            .callback(
                "testplugin2",
                Callback::PickerDone {
                    id,
                    result: PickerResult::Accepted((vec![2], OpenAction::Replace)),
                },
                &Snapshot::default(),
            )
            .unwrap();
        assert_eq!(statuses(&actions), ["data:[\"new item\"]"], "{failure}");
    }
}

#[test]
fn fresh_sdk_02_discards_failed_live_picker_registries() {
    let (mut host, _root) = testplugin2_component(ALL, &fixture("tests/fixtures/testplugin2.wasm"));
    assert!(host
        .run_command("testplugin2", "live-fail", vec![], Snapshot::default())
        .is_err());
    let actions = host
        .run_command("testplugin2", "query-failed", vec![], Snapshot::default())
        .unwrap();
    assert_eq!(statuses(&actions), ["checked"]);
    assert!(!actions
        .iter()
        .any(|action| matches!(action, Action::Error(_))));
    assert!(host.log_lines("testplugin2").unwrap().is_empty());
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
    assert!(host.has_request("testplugin2", id, PendingKind::Process));
    assert!(!host.has_request("other", id, PendingKind::Process));
    assert!(!host.has_request("testplugin2", id, PendingKind::Picker));
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
    assert!(!host.has_request("testplugin2", id, PendingKind::Process));
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
                columns: vec!["name".into()],
                items: vec![],
                initial: None,
                live: false,
            },
        );
        if context.allows_ui() {
            assert!(command.is_ok());
            // Past the context check: testplugin2 has no `process` permission.
            assert!(interactive.unwrap_err().contains("permission denied"));
            assert!(picker.is_ok());
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
        PendingKind::Prompt
        | PendingKind::Popup
        | PendingKind::Background
        | PendingKind::Timer
        | PendingKind::Completion
        | PendingKind::Files
        | PendingKind::Listen
        | PendingKind::Command
        | PendingKind::Batch
        | PendingKind::Service
        | PendingKind::Lsp
        | PendingKind::Watch => unreachable!(),
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

const WITH_PROCESS: &[&str] = &[
    "read-buffers",
    "edit-buffers",
    "open-files",
    "run-commands",
    "process",
];

fn args(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

fn spawn_id(actions: &[Action]) -> u32 {
    match actions {
        [Action::Spawn { id, .. }] => *id,
        other => panic!("expected one spawn, got {other:?}"),
    }
}

#[test]
fn processes_need_the_permission_and_the_users_grant() {
    let (mut host, _root) = testplugin2();
    let err = host
        .run_command(
            "testplugin2",
            "spawn",
            args(&["echo", "hi"]),
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("permission denied"), "{err}");

    let (mut host, _root) = testplugin2_with(WITH_PROCESS);
    let err = host
        .run_command(
            "testplugin2",
            "spawn",
            args(&["echo", "hi"]),
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("allow-process"), "{err}");
    assert_eq!(host.info()[0].process, ProcessGrant::None);

    host.set_process_grant("testplugin2", ProcessGrant::Only(args(&["echo"])));
    let err = host
        .run_command(
            "testplugin2",
            "spawn",
            args(&["sh", "-c", "echo hi"]),
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("'sh' is not in allow-process"),
        "{err}"
    );
    // The program must match exactly, not just by name.
    let err = host
        .run_command(
            "testplugin2",
            "spawn",
            args(&["/tmp/echo"]),
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("not in allow-process"), "{err}");

    let actions = host
        .run_command(
            "testplugin2",
            "spawn",
            args(&["echo", "hi"]),
            Snapshot::default(),
        )
        .unwrap();
    match &actions[..] {
        [Action::Spawn {
            plugin,
            argv,
            cwd,
            stdin,
            timeout,
            ..
        }] => {
            assert_eq!(plugin, "testplugin2");
            assert_eq!(argv, &args(&["echo", "hi"]));
            assert_eq!((cwd, stdin), (&None, &None));
            assert_eq!(*timeout, DEFAULT_PROCESS_TIMEOUT);
        }
        other => panic!("expected a spawn, got {other:?}"),
    }
}

#[test]
fn spawn_results_reach_the_plugin_once() {
    let (mut host, _root) = testplugin2_with(WITH_PROCESS);
    host.set_process_grant("testplugin2", ProcessGrant::Any);
    let id = spawn_id(
        &host
            .run_command("testplugin2", "spawn", args(&["echo"]), Snapshot::default())
            .unwrap(),
    );
    let done = Callback::ProcessDone {
        id,
        result: Ok(ProcessOutput {
            exit_code: Some(3),
            stdout: "out".into(),
            stderr: "err".into(),
        }),
    };
    assert!(host
        .callback(
            "testplugin2",
            Callback::ProcessOutput {
                id,
                chunk: "unexpected".into(),
            },
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
    let actions = host
        .callback("testplugin2", done.clone(), &Snapshot::default())
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::Status(s)] if s == "exit:Some(3);out:out;err:err"),
        "{actions:?}"
    );
    assert!(host
        .callback("testplugin2", done, &Snapshot::default())
        .unwrap()
        .is_empty());
}

#[test]
fn kill_and_process_limit() {
    let (mut host, _root) = testplugin2_with(WITH_PROCESS);
    host.set_process_grant("testplugin2", ProcessGrant::Any);
    let mut ids = Vec::new();
    for _ in 0..MAX_PROCESSES {
        let actions = host
            .run_command("testplugin2", "spawn", args(&["true"]), Snapshot::default())
            .unwrap();
        ids.push(spawn_id(&actions));
    }
    let err = host
        .run_command("testplugin2", "spawn", args(&["true"]), Snapshot::default())
        .unwrap_err();
    assert!(err.to_string().contains("at most"), "{err}");

    // Killing a running program asks the editor to kill it; unknown ids are
    // ignored.
    let actions = host
        .run_command(
            "testplugin2",
            "kill",
            vec![ids[0].to_string()],
            Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::Kill { id, .. }] if *id == ids[0]),
        "{actions:?}"
    );
    let actions = host
        .run_command(
            "testplugin2",
            "kill",
            vec!["999999".into()],
            Snapshot::default(),
        )
        .unwrap();
    assert!(actions.is_empty());

    // A finished program frees its slot.
    host.callback(
        "testplugin2",
        process_done(ids[1], ""),
        &Snapshot::default(),
    )
    .unwrap();
    host.run_command("testplugin2", "spawn", args(&["true"]), Snapshot::default())
        .unwrap();
}

#[test]
fn pickers_and_push_items() {
    let (mut host, _root) = testplugin2();
    let actions = host
        .run_command(
            "testplugin2",
            "picker",
            args(&["a", "b"]),
            Snapshot::default(),
        )
        .unwrap();
    let id = match &actions[..] {
        [Action::ShowPicker { id, picker, .. }] => {
            assert_eq!(picker.items.len(), 2);
            assert_eq!(picker.items[1].data, "b");
            *id
        }
        other => panic!("expected a picker, got {other:?}"),
    };
    let actions = host
        .run_command(
            "testplugin2",
            "push-items",
            vec![id.to_string()],
            Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::PushItems { items, .. }] if items.len() == 1),
        "{actions:?}"
    );
    let err = host
        .run_command(
            "testplugin2",
            "push-items",
            vec!["999999".into()],
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("no open picker"), "{err}");

    // Choosing delivers the indices and the split; the picker is then gone.
    let actions = host
        .callback(
            "testplugin2",
            Callback::PickerDone {
                id,
                result: PickerResult::Accepted((vec![1], OpenAction::Vsplit)),
            },
            &Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::Status(s)] if s == "choice:[1]:Some(OpenAction::Vsplit)"),
        "{actions:?}"
    );
    let err = host
        .run_command(
            "testplugin2",
            "push-items",
            vec![id.to_string()],
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("no open picker"), "{err}");
}

#[test]
fn prompts_and_popups() {
    let (mut host, _root) = testplugin2();
    let actions = host
        .run_command(
            "testplugin2",
            "popup",
            args(&["**hi**", "there"]),
            Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::Popup { markdown: md, centered: false, owner: None }] if md == "**hi** there"),
        "{actions:?}"
    );

    let actions = host
        .run_command(
            "testplugin2",
            "prompt",
            args(&["one", "two"]),
            Snapshot::default(),
        )
        .unwrap();
    let id = match &actions[..] {
        [Action::ShowPrompt { id, prompt, .. }] => {
            assert_eq!(prompt.message, "answer:");
            assert_eq!(prompt.initial, "init");
            assert_eq!(prompt.completions, args(&["one", "two"]));
            *id
        }
        other => panic!("expected a prompt, got {other:?}"),
    };
    let answer = |result: Option<&str>| Callback::PromptDone {
        id,
        result: result.map(str::to_owned),
    };
    let actions = host
        .callback("testplugin2", answer(Some("two")), &Snapshot::default())
        .unwrap();
    assert_eq!(statuses(&actions), ["answer:Some(\"two\")"]);
    // Answered once.
    assert!(host
        .callback("testplugin2", answer(None), &Snapshot::default())
        .unwrap()
        .is_empty());
}

#[test]
fn live_pickers_answer_every_query() {
    let (mut host, _root) = testplugin2();
    let actions = host
        .run_command("testplugin2", "live", vec![], Snapshot::default())
        .unwrap();
    let id = match &actions[..] {
        [Action::ShowPicker { id, picker, .. }] => {
            assert!(picker.live);
            *id
        }
        other => panic!("expected a picker, got {other:?}"),
    };
    for query in ["a", "ab"] {
        let actions = host
            .callback(
                "testplugin2",
                Callback::PickerQuery {
                    id,
                    query: query.into(),
                    generation: None,
                },
                &Snapshot::default(),
            )
            .unwrap();
        match &actions[..] {
            [Action::SetItems { items, .. }] => {
                let data: Vec<_> = items.iter().map(|item| item.data.as_str()).collect();
                assert_eq!(
                    data,
                    [
                        format!("{query}-1"),
                        format!("{query}-2"),
                        format!("{query}-3")
                    ]
                );
            }
            other => panic!("expected new items, got {other:?}"),
        }
    }
    // The choice refers to the latest items.
    let actions = host
        .callback(
            "testplugin2",
            Callback::PickerDone {
                id,
                result: PickerResult::Accepted((vec![2], OpenAction::Replace)),
            },
            &Snapshot::default(),
        )
        .unwrap();
    assert_eq!(statuses(&actions), ["live:[\"ab-3\"]"]);
}

#[test]
fn directories_and_modified_documents() {
    let (mut host, _root) = testplugin2();
    let actions = host
        .run_command("testplugin2", "dirs", vec![], snapshot("x", 0, 0))
        .unwrap();
    // No data root was set: no directory of its own.
    assert_eq!(
        statuses(&actions),
        ["workspace:Some(\"/work\");cwd:/work/src;config:/config;data:;modified:true"]
    );
}

/// testplugin2 with `filesystem`, the user's `files` grant, a workspace with
/// `file.txt` and a data root.
fn testplugin2_files(files: FileGrant) -> (PluginHost, tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let data = root.path().join("data");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("file.txt"), "content").unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin2",
        &fixture("tests/fixtures/testplugin2-v0.2.wasm"),
        &["read-buffers", "filesystem"],
    );
    let manifest = dir.join("plugin.toml");
    let source = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.2\"");
    std::fs::write(manifest, source).unwrap();
    let mut host = host();
    host.set_dirs(Some(workspace.clone()), Some(data.clone()));
    host.load_with(
        &dir,
        PluginSettings {
            config: "{}".into(),
            files,
            ..Default::default()
        },
    )
    .unwrap();
    (host, root, workspace, data)
}

#[test]
fn workspace_files_need_the_users_grant() {
    let read = |host: &mut PluginHost, path: &Path| {
        let actions = host
            .run_command(
                "testplugin2",
                "read-file",
                vec![path.to_string_lossy().into_owned()],
                Snapshot::default(),
            )
            .unwrap();
        statuses(&actions).remove(0)
    };

    let (mut host, _root, workspace, _data) = testplugin2_files(FileGrant::None);
    assert!(read(&mut host, &workspace.join("file.txt")).starts_with("read-error:"));
    assert_eq!(host.info()[0].files, FileGrant::None);

    let (mut host, _root, workspace, _data) = testplugin2_files(FileGrant::Read);
    assert_eq!(read(&mut host, &workspace.join("file.txt")), "read:content");
    // Only the workspace.
    assert!(read(&mut host, Path::new("/etc/hostname")).starts_with("read-error:"));
    // Read only: writing fails.
    let actions = host
        .run_command(
            "testplugin2",
            "write-data",
            vec![workspace.join("new.txt").to_string_lossy().into_owned()],
            Snapshot::default(),
        )
        .unwrap();
    let status = statuses(&actions).remove(0);
    assert!(status.starts_with("own:mine;other:Some(Err("), "{status}");
    assert!(!workspace.join("new.txt").exists());

    let (mut host, _root, workspace, data) = testplugin2_files(FileGrant::Write);
    let actions = host
        .run_command(
            "testplugin2",
            "write-data",
            vec![workspace.join("new.txt").to_string_lossy().into_owned()],
            Snapshot::default(),
        )
        .unwrap();
    assert_eq!(statuses(&actions), ["own:mine;other:Some(Ok(()))"]);
    assert_eq!(
        std::fs::read_to_string(workspace.join("new.txt")).unwrap(),
        "theirs"
    );
    assert_eq!(
        std::fs::read_to_string(data.join("testplugin2/note.txt")).unwrap(),
        "mine"
    );
}

#[test]
fn plugins_own_directory_needs_no_grant() {
    let (mut host, _root, _workspace, data) = testplugin2_files(FileGrant::None);
    let actions = host
        .run_command("testplugin2", "write-data", vec![], Snapshot::default())
        .unwrap();
    assert_eq!(statuses(&actions), ["own:mine;other:None"]);
    assert!(data.join("testplugin2/note.txt").is_file());
}

#[test]
fn new_events_reach_0_2_plugins() {
    let (mut host, _root) = testplugin2();
    for (event, expected) in [
        (Event::DocumentSave(7), "save:7"),
        (Event::DocumentClose(8), "close:8"),
        (Event::SelectionChange(9), "selection:9"),
    ] {
        assert!(host.wants(event.kind()));
        let results = host.dispatch(event, &Snapshot::default());
        let actions = results[0].1.as_ref().unwrap();
        assert_eq!(statuses(actions), [expected]);
    }
}

/// Property tests of the validation of what plugins send (M9.1).
mod properties {
    use quickcheck::{QuickCheck, TestResult};

    use super::*;

    /// The text after applying sorted, non-overlapping `edits`, simply.
    fn model(text: &str, edits: &[(usize, usize, String)]) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut edits = edits.to_vec();
        edits.sort_by_key(|(from, to, _)| (*from, *to));
        let mut out = String::new();
        let mut pos = 0;
        for (from, to, insert) in edits {
            out.extend(&chars[pos..from]);
            out.push_str(&insert);
            pos = to;
        }
        out.extend(&chars[pos..]);
        out
    }

    fn valid(len: usize, edits: &[(usize, usize, String)]) -> bool {
        let mut sorted = edits.to_vec();
        sorted.sort_by_key(|(from, to, _)| (*from, *to));
        let mut end = 0;
        for (from, to, _) in sorted {
            if from > to || to > len || from < end {
                return false;
            }
            end = to;
        }
        true
    }

    #[test]
    fn edits_apply_like_the_model_or_are_rejected() {
        fn property(text: String, raw: Vec<(u8, u8, String)>) -> TestResult {
            let len = text.chars().count();
            // Offsets around the text's length, some out of range.
            let edits: Vec<(usize, usize, String)> = raw
                .into_iter()
                .take(6)
                .map(|(a, b, s)| (a as usize % (len + 3), b as usize % (len + 3), s))
                .collect();
            let rope = Rope::from(text.as_str());
            let wit: Vec<types::Edit> = edits
                .iter()
                .map(|(from, to, text)| types::Edit {
                    from: *from as u32,
                    to: *to as u32,
                    text: text.clone(),
                })
                .collect();
            match transaction_from_edits(&rope, wit) {
                Ok(transaction) => {
                    if !valid(len, &edits) {
                        return TestResult::error("accepted invalid edits");
                    }
                    let mut result = rope.clone();
                    transaction.apply(&mut result);
                    TestResult::from_bool(result == model(&text, &edits).as_str())
                }
                Err(_) => TestResult::from_bool(!valid(len, &edits)),
            }
        }
        QuickCheck::new()
            .tests(2000)
            .quickcheck(property as fn(String, Vec<(u8, u8, String)>) -> TestResult);
    }

    #[test]
    fn selections_are_validated_not_panicking() {
        fn property(text: String, raw: Vec<(u8, u8)>) -> bool {
            let len = text.chars().count();
            let ranges: Vec<types::Range> = raw
                .iter()
                .take(8)
                .map(|(a, h)| types::Range {
                    anchor: (*a as usize % (len + 3)) as u32,
                    head: (*h as usize % (len + 3)) as u32,
                })
                .collect();
            let rope = Rope::from(text.as_str());
            let in_bounds = !ranges.is_empty()
                && ranges
                    .iter()
                    .all(|r| r.anchor as usize <= len && r.head as usize <= len);
            match selection_from_ranges(&rope, &ranges) {
                Ok(selection) => {
                    in_bounds
                        && selection
                            .ensure_invariants(rope.slice(..))
                            .ranges()
                            .iter()
                            .all(|r| r.to() <= len)
                }
                Err(_) => !in_bounds,
            }
        }
        QuickCheck::new()
            .tests(2000)
            .quickcheck(property as fn(String, Vec<(u8, u8)>) -> bool);
    }

    #[test]
    fn text_ranges_never_panic() {
        fn property(text: String, from: u16, to: u16) -> bool {
            let (mut host, _root) = testplugin2();
            let state = host.plugins[0].store.data_mut();
            state.snapshot = snapshot(&text, 0, 0);
            let len = text.chars().count();
            let result = state.text_range(1, from.into(), to.into());
            match result {
                Ok(slice) => {
                    (from as usize) <= (to as usize)
                        && (to as usize) <= len
                        && slice.chars().count() == (to - from) as usize
                }
                Err(_) => from > to || to as usize > len,
            }
        }
        QuickCheck::new()
            .tests(50)
            .quickcheck(property as fn(String, u16, u16) -> bool);
    }
}

/// Startup and event costs (M9.2). Run with
/// `cargo test -p helix-plugin --release -- --ignored --nocapture benchmarks`.
#[test]
#[ignore]
fn benchmarks() {
    let cache = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let example: toml::Table = toml::from_str(
        &std::fs::read_to_string(fixture("examples/text-tools/plugin.toml")).unwrap(),
    )
    .unwrap();
    let api = example["api"].as_str().unwrap();
    eprintln!("text-tools API {api}; event workload: frozen API 0.2 selection-change");
    let plugins = 20;
    for i in 0..plugins {
        let name = format!("tools{i}");
        let dir = plugin_dir(
            root.path(),
            &name,
            &fixture("examples/text-tools/plugin.wasm"),
            ALL,
        );
        let manifest = dir.join("plugin.toml");
        let source = std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("api = \"0.1\"", &format!("api = \"{api}\""));
        std::fs::write(manifest, source).unwrap();
    }
    for (label, cold) in [("cold cache", true), ("warm cache", false)] {
        for count in [1, 5, 20] {
            let cache_dir = if cold {
                // A fresh cache: every plugin is compiled.
                cache.path().join(format!("cold-{count}"))
            } else {
                cache.path().join("cold-20")
            };
            let started = Instant::now();
            let mut host = PluginHost::new(Some(cache_dir)).unwrap();
            let (_, errors) = host.load_all(root.path(), |name| {
                let n: usize = name.trim_start_matches("tools").parse().unwrap();
                (n < count).then(|| PluginSettings::new("{}"))
            });
            assert!(errors.is_empty(), "{errors:?}");
            eprintln!(
                "load {count:>2} plugins ({label}): {:>7.1} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    let (mut host, _root) = testplugin2();
    let snapshot = snapshot(&"some text\n".repeat(10_000), 0, 0);
    let rounds = 200;
    let started = Instant::now();
    for _ in 0..rounds {
        host.dispatch(Event::DocumentChange(1), &snapshot);
    }
    eprintln!(
        "dispatch one event to one plugin: {:>7.3} ms",
        started.elapsed().as_secs_f64() * 1000.0 / rounds as f64
    );
}

/// Random input through the real ABI (see testplugin2's `fuzz`): the host
/// may refuse it but must never crash or disable the plugin, and what it
/// accepts must apply cleanly.
#[test]
fn fuzzing_through_the_abi() {
    let (mut host, _root) = testplugin2();
    for seed in 1..300 {
        let text = "héllo\nwörld\r\n".repeat((seed % 4) as usize);
        let result = host.run_command(
            "testplugin2",
            "fuzz",
            vec![seed.to_string()],
            snapshot(&text, 0, 0),
        );
        let actions = result.unwrap_or_else(|err| panic!("seed {seed}: {err}"));
        // Every edit the host accepted applies to the text in order.
        let mut rope = Rope::from(text.as_str());
        for action in actions {
            match action {
                Action::Edit { transaction, .. } => {
                    assert!(transaction.apply(&mut rope), "seed {seed}");
                }
                Action::SetSelection { selection, .. } => {
                    let selection = selection.ensure_invariants(rope.slice(..));
                    assert!(selection
                        .ranges()
                        .iter()
                        .all(|r| r.to() <= rope.len_chars()));
                }
                _ => {}
            }
        }
        assert!(host.plugins[0].disabled().is_none(), "seed {seed}");
    }
}

#[test]
fn dispatch_to_calls_only_the_named_subscriber_with_the_supplied_snapshot() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    for name in ["first", "second"] {
        let dir = plugin_dir(root.path(), name, &testplugin_wasm(), ALL);
        host.load(&dir, r#"{"edit-on-change":true}"#).unwrap();
    }
    assert_eq!(
        host.subscribers(EventKind::DocumentChange),
        ["first", "second"]
    );
    let first = host
        .dispatch_to("first", Event::DocumentChange(1), &snapshot("abc", 0, 0))
        .unwrap()
        .unwrap();
    let mut updated = Rope::from("abc");
    for action in first {
        if let Action::Edit { transaction, .. } = action {
            assert!(transaction.apply(&mut updated));
        }
    }
    assert_eq!(updated.to_string(), "abc!");
    let second = host
        .dispatch_to(
            "second",
            Event::DocumentChange(1),
            &snapshot(&updated.to_string(), 0, 0),
        )
        .unwrap()
        .unwrap();
    for action in second {
        if let Action::Edit { transaction, .. } = action {
            assert!(transaction.apply(&mut updated));
        }
    }
    assert_eq!(updated.to_string(), "abc!!");
    assert!(host
        .dispatch_to("missing", Event::DocumentChange(1), &Snapshot::default())
        .is_none());
    for name in ["first", "second"] {
        let actions = host
            .run_command(name, "events", vec![], Snapshot::default())
            .unwrap();
        assert_eq!(statuses(&actions), ["events: change:1"]);
    }
}

/// Bound the test itself as well as the guest call: a regression must fail
/// promptly instead of leaving the test runner blocked in a WASI import.
fn bounded_wasi_test(test: impl FnOnce() + Send + 'static) {
    let (send, recv) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(test));
        let _ = send.send(result);
    });
    match recv
        .recv_timeout(Duration::from_secs(15))
        .expect("WASI call or host shutdown hung")
    {
        Ok(()) => {}
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[test]
fn wasi_sleep_is_bounded_in_commands_for_both_abis() {
    bounded_wasi_test(|| {
        for version in ["0.1", "0.2"] {
            let (mut host, _root) = if version == "0.1" {
                testplugin("{}")
            } else {
                testplugin2()
            };
            host.limits.command = Duration::from_millis(80);
            let name = if version == "0.1" {
                "testplugin"
            } else {
                "testplugin2"
            };
            let start = std::time::Instant::now();
            let error = host
                .run_command(name, "sleep", vec![], Snapshot::default())
                .unwrap_err();
            assert!(
                error.to_string().contains("took longer than 80 ms"),
                "{error:#}"
            );
            assert!(start.elapsed() < Duration::from_secs(2));
            assert!(host.plugin(name).unwrap().disabled().is_some());
            assert_eq!(host.ticker.busy.load(Ordering::Acquire), 0);
            // Dropping the runtime must not wait for the guest's minute timer.
            drop(host);
        }
    });
}

#[test]
fn wasi_sleep_is_bounded_in_init_events_and_callbacks() {
    bounded_wasi_test(|| {
        let (mut host, root) = testplugin2();
        host.clear();
        host.limits.init = Duration::from_millis(80);
        let error = host
            .load(&root.path().join("testplugin2"), r#"{"sleep-init":true}"#)
            .unwrap_err();
        assert!(
            error.to_string().contains("took longer than 80 ms"),
            "{error:#}"
        );
        host.limits.init = Duration::from_secs(5);
        host.load(
            &root.path().join("testplugin2"),
            r#"{"sleep-on-open":true}"#,
        )
        .unwrap();
        host.limits.event = Duration::from_millis(80);
        let error = host
            .dispatch_to(
                "testplugin2",
                Event::DocumentOpen(1),
                &snapshot("abc", 0, 0),
            )
            .unwrap()
            .unwrap_err();
        assert!(
            error.to_string().contains("took longer than 80 ms"),
            "{error:#}"
        );
        host.clear();
        host.load(&root.path().join("testplugin2"), "{}").unwrap();
        host.limits.command = Duration::from_millis(80);
        let id = arm_request(&mut host, PendingKind::Process).unwrap();
        let error = host
            .callback(
                "testplugin2",
                process_done(id, "sleep"),
                &Snapshot::default(),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("took longer than 80 ms"),
            "{error:#}"
        );
        assert!(host
            .plugin("testplugin2")
            .unwrap()
            .store
            .data()
            .pending
            .is_empty());
    });
}

#[cfg(unix)]
#[test]
fn wasi_fifo_open_and_host_shutdown_are_bounded() {
    bounded_wasi_test(|| {
        let (mut host, root, workspace, _) = testplugin2_files(FileGrant::Read);
        let fifo = workspace.join("blocked.fifo");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        host.limits.command = Duration::from_millis(80);
        let start = std::time::Instant::now();
        let error = host
            .run_command(
                "testplugin2",
                "read-file",
                vec![fifo.to_string_lossy().into_owned()],
                Snapshot::default(),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("took longer than 80 ms"),
            "{error:#}"
        );
        drop(host);
        assert!(start.elapsed() < Duration::from_secs(2));
        // Release the OS open after verifying shutdown, so this test leaves
        // no detached blocking operation behind. Open both ends without waiting.
        let release = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
            .unwrap();
        drop(release);
        drop(root);
    });
}

#[test]
fn picker_item_limit_is_cumulative_and_resets_on_replacement_and_close() {
    let (mut host, _root) = testplugin2();
    let state = host.plugins[0].store.data_mut();
    state.context = CallContext::Command;
    let item = || PickerItem {
        columns: vec![],
        data: String::new(),
        preview: None,
    };
    let id = state
        .show_picker(
            PickerSpec {
                title: "Boundary".into(),
                columns: vec!["Name".into()],
                items: (0..MAX_PICKER_ITEMS - 1).map(|_| item()).collect(),
                initial: None,
                live: false,
            },
            false,
        )
        .unwrap();
    // Release the recorded payload, retaining the count of the open picker.
    state.actions.clear();
    state.push_items(id, None, vec![item()]).unwrap();
    assert!(state
        .push_items(id, None, vec![item()])
        .unwrap_err()
        .contains("at most"));
    assert_eq!(
        state.actions.len(),
        1,
        "a refused push must not record an action"
    );
    state.set_items(id, None, vec![item()]).unwrap();
    state.push_items(id, None, vec![item()]).unwrap();
    assert_eq!(state.picker_items[&id], 2);
    host.callback(
        "testplugin2",
        Callback::PickerDone {
            id,
            result: PickerResult::Cancelled,
        },
        &Snapshot::default(),
    )
    .unwrap();
    let state = host.plugins[0].store.data_mut();
    assert!(!state.picker_items.contains_key(&id));
    assert!(state.push_items(id, None, vec![item()]).is_err());
}

#[test]
fn failed_commands_restore_picker_item_accounting() {
    let (mut host, _root) = testplugin2();
    let actions = host
        .run_command(
            "testplugin2",
            "picker",
            args(&["one", "two"]),
            Snapshot::default(),
        )
        .unwrap();
    let id = match actions[0] {
        Action::ShowPicker { id, .. } => id,
        _ => panic!("expected picker"),
    };
    let error = host
        .run_command(
            "testplugin2",
            "push-items-fail",
            vec![id.to_string()],
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("failed after push"));
    assert_eq!(host.plugins[0].store.data().picker_items[&id], 2);
    host.run_command(
        "testplugin2",
        "push-items",
        vec![id.to_string()],
        Snapshot::default(),
    )
    .unwrap();
    assert_eq!(host.plugins[0].store.data().picker_items[&id], 3);
}

#[test]
fn obsolete_cache_versions_are_removed_without_touching_other_plugins() {
    let host = host();
    let cache = tempfile::tempdir().unwrap();
    let names = || {
        let mut names: Vec<_> = std::fs::read_dir(cache.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    compile(
        &host.engine,
        Some(cache.path()),
        "owner",
        &testplugin_wasm(),
    )
    .unwrap();
    let original = names().pop().unwrap();
    let obsolete = cache.path().join("owner-0000000000000000.cwasm");
    let other = cache.path().join("owner-other-0000000000000000.cwasm");
    let unrelated = cache.path().join("owner-not-a-hash.cwasm");
    for path in [&obsolete, &other, &unrelated] {
        std::fs::write(path, "keep unless obsolete").unwrap();
    }
    // A warm cache hit also removes old entries.
    compile(
        &host.engine,
        Some(cache.path()),
        "owner",
        &testplugin_wasm(),
    )
    .unwrap();
    assert!(!obsolete.exists());
    assert!(other.exists());
    assert!(unrelated.exists());
    assert!(cache.path().join(&original).exists());
    // Changed component contents replace the old version, without leaving tmp files.
    compile(
        &host.engine,
        Some(cache.path()),
        "owner",
        &text_tools_wasm(),
    )
    .unwrap();
    assert!(!cache.path().join(&original).exists());
    assert_eq!(names().len(), 3);
}

#[test]
fn failed_cache_rename_removes_the_unfinished_write() {
    let host = host();
    let cache = tempfile::tempdir().unwrap();
    compile(
        &host.engine,
        Some(cache.path()),
        "owner",
        &testplugin_wasm(),
    )
    .unwrap();
    let target = std::fs::read_dir(cache.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_file(&target).unwrap();
    // Force the real writer's rename to fail after it created its temp file.
    std::fs::create_dir(&target).unwrap();
    compile(
        &host.engine,
        Some(cache.path()),
        "owner",
        &testplugin_wasm(),
    )
    .unwrap();
    let remaining: Vec<_> = std::fs::read_dir(cache.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(remaining, [target]);
}

fn load_v3(host: &mut PluginHost, root: &Path, permissions: &[&str]) {
    let dir = plugin_dir(
        root,
        "testplugin3",
        &fixture("tests/fixtures/testplugin3.wasm"),
        permissions,
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.5\"");
    std::fs::write(path, manifest).unwrap();
    host.load(&dir, "{}").unwrap();
}

fn load_v3_with(host: &mut PluginHost, root: &Path, permissions: &[&str], config: &str) {
    let dir = plugin_dir(
        root,
        "testplugin3",
        &fixture("tests/fixtures/testplugin3.wasm"),
        permissions,
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.5\"");
    std::fs::write(path, manifest).unwrap();
    host.load(&dir, config).unwrap();
}

#[test]
fn api_03_diagnostics_are_validated() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), ALL);
    let actions = host
        .run_command("testplugin3", "diagnose", vec![], snapshot("hello", 0, 0))
        .unwrap();
    match &actions[..] {
        [Action::Diagnostics {
            plugin,
            doc: 1,
            diagnostics,
        }] => {
            assert_eq!(plugin, "testplugin3");
            assert_eq!(diagnostics[0].range, 0..1);
            assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
            assert_eq!(diagnostics[0].code.as_deref(), Some("T1"));
        }
        other => panic!("{other:?}"),
    }
    let err = host
        .run_command(
            "testplugin3",
            "diagnose-bad",
            vec![],
            snapshot("hello", 0, 0),
        )
        .unwrap_err();
    assert!(err.to_string().contains("invalid range"), "{err}");
}

#[test]
fn api_03_timers_fire_once_and_can_be_cancelled() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), ALL);
    let actions = host
        .run_command(
            "testplugin3",
            "timer",
            vec!["5".into()],
            Snapshot::default(),
        )
        .unwrap();
    let id = match &actions[..] {
        [Action::SetTimer { id, delay, .. }] => {
            assert_eq!(*delay, Duration::from_millis(5));
            *id
        }
        other => panic!("{other:?}"),
    };
    let fired = host
        .callback("testplugin3", Callback::Timer { id }, &Snapshot::default())
        .unwrap();
    assert!(matches!(&fired[..], [Action::Status(s)] if *s == format!("timer-fired:{id}")));
    // Only once.
    let again = host
        .callback("testplugin3", Callback::Timer { id }, &Snapshot::default())
        .unwrap();
    assert!(again.is_empty());

    // Set and cancelled in one call: never started.
    let actions = host
        .run_command("testplugin3", "timer-cancel", vec![], Snapshot::default())
        .unwrap();
    assert!(actions.is_empty(), "{actions:?}");

    // Cancelled by a later call: the editor is told to stop it.
    let actions = host
        .run_command(
            "testplugin3",
            "timer",
            vec!["60000".into()],
            Snapshot::default(),
        )
        .unwrap();
    let [Action::SetTimer { id, .. }] = &actions[..] else {
        panic!("{actions:?}");
    };
    let id = *id;
    let actions = host
        .run_command(
            "testplugin3",
            "cancel-timer",
            vec![id.to_string()],
            Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::CancelTimer { id: cancelled, .. }] if *cancelled == id),
        "{actions:?}"
    );
    let fired = host
        .callback("testplugin3", Callback::Timer { id }, &Snapshot::default())
        .unwrap();
    assert!(fired.is_empty(), "a cancelled timer must not fire");
}

#[test]
fn api_03_registers_need_the_permission() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), ALL);
    assert!(!host.wants_registers());
    let err = host
        .run_command("testplugin3", "registers", vec![], Snapshot::default())
        .unwrap_err();
    assert!(err.to_string().contains("permission denied"), "{err}");

    let mut host = self::host();
    let root = tempfile::tempdir().unwrap();
    let mut permissions = ALL.to_vec();
    permissions.push("registers");
    load_v3(&mut host, root.path(), &permissions);
    assert!(host.wants_registers());
    let actions = host
        .run_command("testplugin3", "registers", vec![], Snapshot::default())
        .unwrap();
    assert!(matches!(
        &actions[..],
        [Action::SetRegister { name: 'a', values }, Action::Status(status)]
            if values == &["x", "y"] && status == "registers:x,y"
    ));
    let err = host
        .run_command("testplugin3", "read-clipboard", vec![], Snapshot::default())
        .unwrap_err();
    assert!(err.to_string().contains("clipboard"), "{err}");
}

#[test]
fn api_03_completion_requests_are_answered_once() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3_with(&mut host, root.path(), ALL, "{\"completion\": true}");
    assert!(host.wants(EventKind::Completion));
    let (id, actions) = host
        .request_completion("testplugin3", 1, 3, &snapshot("alp", 3, 3))
        .unwrap();
    let actions = actions.unwrap();
    match &actions[..] {
        [Action::Completions {
            id: answered,
            items,
            ..
        }] => {
            assert_eq!(*answered, id);
            assert_eq!(items[0].label, "alpha");
            assert_eq!(items[1].kind.as_deref(), Some("test"));
        }
        other => panic!("{other:?}"),
    }
    assert!(!host.has_request("testplugin3", id, PendingKind::Completion));
    // A new request replaces an unanswered one.
    let old = host
        .register_request("testplugin3", PendingKind::Completion)
        .unwrap();
    let (new, _) = host
        .request_completion("testplugin3", 1, 3, &snapshot("alp", 3, 3))
        .unwrap();
    assert!(!host.has_request("testplugin3", old, PendingKind::Completion));
    assert_ne!(old, new);
}

#[test]
fn api_04_capabilities_revisions_cancel_and_snippets() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3_with(
        &mut host,
        root.path(),
        ALL,
        "{\"completion\": true, \"snippets\": true}",
    );
    let status = |host: &mut PluginHost, command: &str, args: Vec<String>, snap: Snapshot| {
        statuses(
            &host
                .run_command("testplugin3", command, args, snap)
                .unwrap(),
        )
    };
    // testplugin3 is built for 0.5: it sees the 0.5 additions too.
    let caps: Vec<&str> = CAPABILITIES
        .iter()
        .chain(CAPABILITIES_05)
        .copied()
        .collect();
    assert_eq!(
        status(&mut host, "caps", vec![], Snapshot::default()),
        [format!("caps:{}", caps.join(","))]
    );
    assert!(caps.starts_with(&[
        "cancel-request",
        "revisions",
        "snippet-completions",
        "panels",
        "listen"
    ]));

    // A document's revision; its own edits count within the call.
    let mut snap = snapshot("text", 0, 0);
    snap.documents.get_mut(&1).unwrap().revision = 7;
    assert_eq!(status(&mut host, "rev", vec![], snap.clone()), ["rev:7:8"]);
    // Edits for the revision the text is at apply; for an older one not.
    let actions = host
        .run_command("testplugin3", "edit-at", vec!["7".into()], snap.clone())
        .unwrap();
    assert!(actions.iter().any(|a| matches!(a, Action::Edit { .. })));
    let actions = host
        .run_command("testplugin3", "edit-at", vec!["6".into()], snap.clone())
        .unwrap();
    assert!(!actions.iter().any(|a| matches!(a, Action::Edit { .. })));
    assert!(
        statuses(&actions)[0].contains("at revision 7, not 6"),
        "{actions:?}"
    );

    // Cancelling a timer started earlier stops it; its callback never runs.
    let actions = host
        .run_command(
            "testplugin3",
            "timer",
            vec!["60000".into()],
            Snapshot::default(),
        )
        .unwrap();
    let [Action::SetTimer { id, .. }] = &actions[..] else {
        panic!("{actions:?}");
    };
    let id = *id;
    let actions = host
        .run_command(
            "testplugin3",
            "cancel",
            vec![id.to_string()],
            Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::CancelTimer { id: cancelled, .. }] if *cancelled == id)
    );
    assert!(!host.has_request("testplugin3", id, PendingKind::Timer));
    // A file batch: the editor is told to skip the rest.
    let files = host
        .register_request("testplugin3", PendingKind::Files)
        .unwrap();
    let actions = host
        .run_command(
            "testplugin3",
            "cancel",
            vec![files.to_string()],
            Snapshot::default(),
        )
        .unwrap();
    assert!(matches!(&actions[..], [Action::CancelFiles { id, .. }] if *id == files));
    let late = host
        .callback(
            "testplugin3",
            Callback::FilesDone {
                id: files,
                results: vec![],
            },
            &Snapshot::default(),
        )
        .unwrap();
    assert!(
        late.is_empty(),
        "a cancelled request's callback doesn't run"
    );
    // Unknown ids are ignored.
    assert!(host
        .run_command(
            "testplugin3",
            "cancel",
            vec!["999".into()],
            Snapshot::default()
        )
        .unwrap()
        .is_empty());

    // Snippet completions are marked as such.
    let (_, actions) = host
        .request_completion("testplugin3", 1, 0, &snapshot("", 0, 0))
        .unwrap();
    match &actions.unwrap()[..] {
        [Action::Completions { items, .. }] => {
            assert!(items[0].snippet);
            assert_eq!(items[0].insert.as_deref(), Some("fn ${1:name}() {$0}"));
        }
        other => panic!("{other:?}"),
    }
}

/// Components that can't start for API reasons get a clear error.
#[test]
fn missing_imports_say_a_newer_pure_is_needed() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin3",
        &fixture("tests/fixtures/testplugin3.wasm"),
        ALL,
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.3\"");
    std::fs::write(path, manifest).unwrap();
    // Built for 0.4, declared 0.3.
    let err = format!("{:#}", host.load(&dir, "{}").unwrap_err());
    assert!(err.contains("built for another API version"), "{err}");
    // What Wasmtime says on a Pure without an import the plugin uses.
    let missing = anyhow::anyhow!(
        "component imports instance `helix:plugin/revisions@0.4.0`, but a matching \
         implementation was not found in the linker"
    );
    let err = format!("{:#}", crate::explain_start_error("x", "0.4", missing));
    assert!(err.contains("needs a newer Pure"), "{err}");
    assert!(err.contains("`helix:plugin/revisions@0.4.0`"), "{err}");
}

/// API 0.4 components built before `requests`, `revisions` and `snippets`
/// were added keep loading and working.
#[test]
fn frozen_api_04_components_still_work() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin3",
        &fixture("tests/fixtures/testplugin3-v0.4.wasm"),
        ALL,
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.4\"");
    std::fs::write(path, manifest).unwrap();
    host.load(&dir, "{}").unwrap();
    let actions = host
        .run_command("testplugin3", "diagnose", vec![], snapshot("hello", 0, 0))
        .unwrap();
    assert!(matches!(&actions[..], [Action::Diagnostics { .. }]));
}

/// The last 0.4 build (with `requests`, `panels` and `listener`) keeps
/// loading next to 0.5 plugins, and its 0.4 imports work.
#[test]
fn frozen_api_041_components_still_work() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin3",
        &fixture("tests/fixtures/testplugin3-v0.4.1.wasm"),
        ALL,
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.4\"");
    std::fs::write(path, manifest).unwrap();
    host.load(&dir, "{}").unwrap();
    let actions = host
        .run_command("testplugin3", "caps", vec![], Snapshot::default())
        .unwrap();
    let statuses = statuses(&actions);
    assert_eq!(statuses.len(), 1);
    assert!(
        statuses[0].starts_with("caps:cancel-request,"),
        "{statuses:?}"
    );
}

#[test]
fn api_03_plugin_buffers() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), ALL);
    let actions = host
        .run_command("testplugin3", "buffer", vec![], Snapshot::default())
        .unwrap();
    assert!(matches!(
        &actions[..],
        [Action::OpenScratch { owner: Some((plugin, name)), text, .. }]
            if plugin == "testplugin3" && name == "test://buffer" && text == "one\n"
    ));
    let actions = host
        .run_command("testplugin3", "scratch", vec![], Snapshot::default())
        .unwrap();
    assert!(matches!(
        &actions[..],
        [Action::OpenScratch { owner: None, language: Some(language), .. }] if language == "markdown"
    ));

    // Writing: only the owner's buffers can be marked saved.
    let mut snap = snapshot("two\n", 0, 0);
    snap.documents.get_mut(&1).unwrap().owner =
        Some(("testplugin3".into(), "test://buffer".into()));
    let actions = host.buffer_write("testplugin3", 1, &snap).unwrap();
    assert!(matches!(
        &actions[..],
        [Action::Status(status), Action::MarkSaved(1)] if status == "written:test://buffer:two"
    ));
    snap.documents.get_mut(&1).unwrap().owner = Some(("other".into(), "x".into()));
    let actions = host.buffer_write("testplugin3", 1, &snap).unwrap();
    assert!(matches!(&actions[..], [Action::Status(status)] if status == "written::two"));
}

#[test]
fn api_03_live_generations_drop_stale_queries_and_batches_before_accounting() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), ALL);
    assert!(host
        .run_command("testplugin3", "live-fail", vec![], Snapshot::default())
        .is_err());
    let state = host.plugins.last().unwrap().store.data();
    assert!(state.pending.is_empty());
    assert!(state.picker_generations.is_empty());
    let actions = host
        .run_command("testplugin3", "live", vec![], Snapshot::default())
        .unwrap();
    let id = match actions[0] {
        Action::ShowPicker {
            id,
            generation: Some(0),
            ..
        } => id,
        ref other => panic!("expected live picker: {other:?}"),
    };
    assert!(host
        .run_command(
            "testplugin3",
            "items",
            args(&["-", "set"]),
            Snapshot::default()
        )
        .unwrap_err()
        .to_string()
        .contains("need a query generation"));
    host.set_picker_generation("testplugin3", id, 1);
    assert_eq!(
        host.plugins.last().unwrap().store.data().picker_items[&id],
        0
    );
    let query = |generation| Callback::PickerQuery {
        id,
        generation: Some(generation),
        query: "a".into(),
    };
    assert!(host
        .callback("testplugin3", query(0), &Snapshot::default())
        .unwrap()
        .is_empty());
    let actions = host
        .callback("testplugin3", query(1), &Snapshot::default())
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::SetItems { generation: Some(1), items, .. }] if items[0].data == "a:1")
    );
    // A full current generation must not reject a stale push at the cap,
    // and a stale replacement must not reset the current count.
    host.plugins
        .last_mut()
        .unwrap()
        .store
        .data_mut()
        .picker_items
        .insert(id, MAX_PICKER_ITEMS);
    for update in [args(&["0", "set"]), args(&["0", "push", "old"])] {
        assert!(host
            .run_command("testplugin3", "items", update, Snapshot::default())
            .unwrap()
            .is_empty());
        assert_eq!(
            host.plugins.last().unwrap().store.data().picker_items[&id],
            MAX_PICKER_ITEMS
        );
    }
    assert!(host
        .run_command(
            "testplugin3",
            "items",
            args(&["1", "push", "fresh"]),
            Snapshot::default()
        )
        .unwrap_err()
        .to_string()
        .contains("at most"));
    host.set_picker_generation("testplugin3", id, 2);
    assert!(host
        .callback("testplugin3", query(1), &Snapshot::default())
        .unwrap()
        .is_empty());
    assert!(!host
        .callback("testplugin3", query(2), &Snapshot::default())
        .unwrap()
        .is_empty());
    assert_eq!(
        host.plugins.last().unwrap().store.data().picker_items[&id],
        1
    );
    host.callback(
        "testplugin3",
        Callback::PickerDone {
            id,
            result: PickerResult::Cancelled,
        },
        &Snapshot::default(),
    )
    .unwrap();
    let state = host.plugins.last().unwrap().store.data();
    assert!(!state.picker_items.contains_key(&id));
    assert!(!state.picker_generations.contains_key(&id));
    assert!(host
        .callback("testplugin3", query(2), &Snapshot::default())
        .unwrap()
        .is_empty());
}

#[test]
fn api_03_static_pickers_use_unversioned_batches() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), ALL);
    let actions = host
        .run_command("testplugin3", "static", vec![], Snapshot::default())
        .unwrap();
    assert!(matches!(
        actions[0],
        Action::ShowPicker {
            generation: None,
            ..
        }
    ));
    let actions = host
        .run_command(
            "testplugin3",
            "items",
            args(&["-", "push", "second"]),
            Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::PushItems { generation: None, items, .. }] if items[0].data == "second")
    );
    assert!(host
        .run_command(
            "testplugin3",
            "items",
            args(&["1", "set"]),
            Snapshot::default()
        )
        .unwrap_err()
        .to_string()
        .contains("require a live API 0.3 picker"));
}

#[test]
fn api_03_streams_keep_the_request_until_completion_and_use_fresh_snapshots() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), &["process", "read-buffers"]);
    assert!(host
        .run_command(
            "testplugin3",
            "stream",
            args(&["echo"]),
            Snapshot::default()
        )
        .unwrap_err()
        .to_string()
        .contains("process not granted"));
    host.set_process_grant("testplugin3", ProcessGrant::Any);
    assert!(host
        .run_command(
            "testplugin3",
            "stream-fail",
            args(&["echo"]),
            Snapshot::default()
        )
        .is_err());
    let state = host.plugins.last().unwrap().store.data();
    assert!(state.pending.is_empty());
    assert!(state.streaming.is_empty());
    let actions = host
        .run_command(
            "testplugin3",
            "stream",
            args(&["echo"]),
            Snapshot::default(),
        )
        .unwrap();
    let id = match &actions[0] {
        Action::Spawn {
            id, stream: true, ..
        } => *id,
        other => panic!("expected streaming spawn: {other:?}"),
    };
    assert!(host
        .callback(
            "testplugin3",
            Callback::ProcessOutput {
                id: id + 1,
                chunk: "foreign".into()
            },
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
    for (chunk, text) in [("first\n", "before"), ("second\n", "after")] {
        let actions = host
            .callback(
                "testplugin3",
                Callback::ProcessOutput {
                    id,
                    chunk: chunk.into(),
                },
                &snapshot(text, 0, 0),
            )
            .unwrap();
        assert_eq!(
            statuses(&actions),
            [format!("chunk:{chunk}:snapshot:{text}")]
        );
    }
    let done = Callback::ProcessDone {
        id,
        result: Ok(ProcessOutput {
            exit_code: Some(0),
            stdout: "tail".into(),
            stderr: "err".into(),
        }),
    };
    assert_eq!(
        statuses(
            &host
                .callback("testplugin3", done.clone(), &Snapshot::default())
                .unwrap()
        ),
        ["done:Some(0):tail:err"]
    );
    assert!(host
        .callback("testplugin3", done, &Snapshot::default())
        .unwrap()
        .is_empty());
    assert!(host
        .callback(
            "testplugin3",
            Callback::ProcessOutput {
                id,
                chunk: "late".into()
            },
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
    let state = host.plugins.last().unwrap().store.data();
    assert!(state.pending.is_empty());
    assert!(state.streaming.is_empty());
    let output = statuses(
        &host
            .run_command("testplugin3", "output-events", vec![], Snapshot::default())
            .unwrap(),
    )
    .remove(0);
    assert!(output.find("first").unwrap() < output.find("second").unwrap());
    assert!(output.find("second").unwrap() < output.find("done:").unwrap());
}

#[test]
fn api_03_extended_events_are_opt_in_and_preserve_dispatch_order() {
    let (mut host, root) = testplugin2();
    load_v3(&mut host, root.path(), ALL);
    let kinds = [
        EventKind::PostCommand,
        EventKind::FocusGained,
        EventKind::FocusLost,
        EventKind::EditorIdle,
    ];
    for kind in kinds {
        assert!(!host.wants(kind));
    }
    host.clear();
    host.load(
        &root.path().join("testplugin3"),
        "{\"extended-events\":true}",
    )
    .unwrap();
    for kind in kinds {
        assert_eq!(host.subscribers(kind), ["testplugin3"]);
    }
    for event in [
        Event::PostCommand("move_char_left".into()),
        Event::FocusLost,
        Event::FocusGained,
        Event::EditorIdle,
    ] {
        let results = host.dispatch(event, &Snapshot::default());
        assert_eq!(results.len(), 1);
        for (name, result) in results {
            assert_eq!(name, "testplugin3");
            assert!(result.unwrap().is_empty());
        }
    }
    let actions = host
        .run_command("testplugin3", "events", vec![], Snapshot::default())
        .unwrap();
    assert_eq!(
        statuses(&actions),
        ["command:move_char_left,focus:lost,focus:gained,idle"]
    );
}

#[test]
fn three_api_worlds_run_side_by_side_with_frozen_02_components() {
    let (mut host, root) = testplugin2();
    let legacy = plugin_dir(root.path(), "testplugin", &testplugin_wasm(), ALL);
    host.load(&legacy, "{}").unwrap();
    load_v3(&mut host, root.path(), ALL);
    let actions = host
        .run_command("testplugin3", "inspect", vec![], snapshot("héllo", 0, 0))
        .unwrap();
    assert_eq!(statuses(&actions), ["0.3:héllo"]);
    let mut text = Rope::from("héllo");
    for action in actions {
        if let Action::Edit { transaction, .. } = action {
            assert!(transaction.apply(&mut text));
        }
    }
    assert_eq!(text.to_string(), "v3:héllo");
    let actions = host
        .run_command("testplugin2", "inspect", vec![], snapshot("héllo", 0, 0))
        .unwrap();
    assert_eq!(statuses(&actions), ["héllo:Mode::Normal"]);
    let actions = host
        .run_command("testplugin", "read", vec![], snapshot("héllo", 0, 0))
        .unwrap();
    assert_eq!(statuses(&actions), ["read 5 chars"]);
}

#[test]
fn open_file_at_records_the_position_and_enforces_permission() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), ALL);
    let actions = host
        .run_command(
            "testplugin3",
            "open-at",
            args(&["/tmp/with spaces", "vsplit", "2", "3"]),
            Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::OpenFileAt { path, action: OpenAction::Vsplit, line: Some(2), column: Some(3) }] if path == Path::new("/tmp/with spaces"))
    );
    host.clear();
    load_v3(&mut host, root.path(), &["read-buffers"]);
    let error = host
        .run_command(
            "testplugin3",
            "open-at",
            args(&["/tmp/file"]),
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("permission denied: add \"open-files\""),
        "{error:#}"
    );
}

#[test]
fn owned_popups_close_once_and_ignore_foreign_ids() {
    let (mut host, root) = testplugin2();
    assert!(host
        .register_request("testplugin2", PendingKind::Popup)
        .unwrap_err()
        .to_string()
        .contains("API 0.3"));
    load_v3(&mut host, root.path(), ALL);
    let actions = host
        .run_command(
            "testplugin3",
            "popup",
            args(&["centered", "**hello**"]),
            Snapshot::default(),
        )
        .unwrap();
    let id = match &actions[0] {
        Action::Popup {
            markdown,
            centered: true,
            owner: Some((plugin, id)),
        } => {
            assert_eq!(markdown, "**hello**");
            assert_eq!(plugin, "testplugin3");
            *id
        }
        other => panic!("expected owned popup: {other:?}"),
    };
    let state = host
        .plugins
        .iter_mut()
        .find(|plugin| plugin.name() == "testplugin3")
        .unwrap()
        .store
        .data_mut();
    state.context = CallContext::Event;
    state.close_popup(id);
    assert!(
        state.actions.is_empty(),
        "event handlers cannot change popup UI"
    );
    let foreign = host
        .register_request("testplugin2", PendingKind::Process)
        .unwrap();
    assert!(host
        .run_command(
            "testplugin3",
            "close-popup",
            vec![foreign.to_string()],
            Snapshot::default()
        )
        .unwrap()
        .is_empty());
    let close = host
        .run_command(
            "testplugin3",
            "close-popup",
            vec![id.to_string()],
            Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&close[..], [Action::ClosePopup { plugin, id: close_id }] if plugin == "testplugin3" && *close_id == id)
    );
    let actions = host
        .callback(
            "testplugin3",
            Callback::PopupClosed { id },
            &Snapshot::default(),
        )
        .unwrap();
    assert_eq!(statuses(&actions), [format!("popup-closed:{id}")]);
    assert!(host
        .callback(
            "testplugin3",
            Callback::PopupClosed { id },
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
    assert!(host
        .run_command(
            "testplugin3",
            "close-popup",
            vec![id.to_string()],
            Snapshot::default()
        )
        .unwrap()
        .is_empty());
    host.clear();
    let dir = root.path().join("testplugin3");
    host.load(&dir, r#"{"popup-on-open":true}"#).unwrap();
    let actions = host
        .dispatch_to(
            "testplugin3",
            Event::DocumentOpen(1),
            &snapshot("abc", 0, 0),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        statuses(&actions),
        ["show-popup is only available from plugin commands and callbacks"]
    );
}

fn example_host(name: &str) -> (PluginHost, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(name);
    std::fs::create_dir(&dir).unwrap();
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(fixture(&format!("examples/{name}/{file}")), dir.join(file)).unwrap();
    }
    let mut host = host();
    host.load(&dir, "{}").unwrap();
    (host, root)
}

#[test]
fn sdk_03_popup_events_and_positioned_open_use_the_new_exports() {
    let (mut host, _root) = example_host("api-demo");
    let actions = host
        .run_command(
            "api-demo",
            "popup",
            args(&["centered", "close-now"]),
            Snapshot::default(),
        )
        .unwrap();
    let id = match &actions[0] {
        Action::Popup {
            centered: true,
            owner: Some((owner, id)),
            ..
        } if owner == "api-demo" => *id,
        other => panic!("expected owned popup: {other:?}"),
    };
    assert!(matches!(actions.last(), Some(Action::ClosePopup { id: closed, .. }) if *closed == id));
    let closed = Callback::PopupClosed { id };
    assert_eq!(
        statuses(
            &host
                .callback("api-demo", closed.clone(), &Snapshot::default())
                .unwrap()
        ),
        [format!("closed:{id}")]
    );
    assert!(host
        .callback("api-demo", closed, &Snapshot::default())
        .unwrap()
        .is_empty());
    for event in [
        Event::PostCommand("move_char_left".into()),
        Event::FocusGained,
        Event::FocusLost,
        Event::EditorIdle,
    ] {
        host.dispatch(event, &Snapshot::default());
    }
    let lines = host.log_lines("api-demo").unwrap();
    for expected in [
        "event:post:move_char_left",
        "event:focus:gained",
        "event:focus:lost",
        "event:idle",
    ] {
        assert!(
            lines.iter().any(|line| line.contains(expected)),
            "{lines:?}"
        );
    }
    let actions = host
        .run_command(
            "api-demo",
            "open-at",
            args(&["/tmp/a b", "vsplit", "2", "3"]),
            Snapshot::default(),
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::OpenFileAt { path, action: OpenAction::Vsplit, line: Some(2), column: Some(3) }] if path == Path::new("/tmp/a b"))
    );
}

#[test]
fn sdk_03_live_helpers_rollback_and_discard_stale_data() {
    let (mut host, _root) = example_host("api-demo");
    let actions = host
        .run_command("api-demo", "live", vec![], Snapshot::default())
        .unwrap();
    let id = match actions[0] {
        Action::ShowPicker {
            id,
            generation: Some(0),
            ..
        } => id,
        ref other => panic!("{other:?}"),
    };
    let query = |generation| Callback::PickerQuery {
        id,
        generation: Some(generation),
        query: "new".into(),
    };
    host.set_picker_generation("api-demo", id, 1);
    host.callback("api-demo", query(1), &Snapshot::default())
        .unwrap();
    assert!(host
        .run_command(
            "api-demo",
            "rollback",
            args(&["1", "wrong"]),
            Snapshot::default()
        )
        .is_err());
    host.run_command(
        "api-demo",
        "push",
        args(&["1", "right"]),
        Snapshot::default(),
    )
    .unwrap();
    assert!(host
        .run_command(
            "api-demo",
            "update",
            args(&["0", "stale"]),
            Snapshot::default()
        )
        .unwrap()
        .is_empty());
    // The host invalidates before the debounced SDK callback arrives.
    host.set_picker_generation("api-demo", id, 2);
    assert!(host
        .run_command(
            "api-demo",
            "update",
            args(&["1", "debounce-gap"]),
            Snapshot::default()
        )
        .unwrap()
        .is_empty());
    let actions = host
        .callback("api-demo", query(2), &Snapshot::default())
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::SetItems { generation: Some(2), items, .. }] if items[0].data == "new:2")
    );
    let actions = host
        .callback(
            "api-demo",
            Callback::PickerDone {
                id,
                result: PickerResult::Accepted((vec![0], OpenAction::Replace)),
            },
            &Snapshot::default(),
        )
        .unwrap();
    assert_eq!(statuses(&actions), ["choice:[\"new:2\"]"]);
    assert!(host
        .run_command(
            "api-demo",
            "push",
            args(&["2", "closed"]),
            Snapshot::default()
        )
        .unwrap()
        .is_empty());
}

#[test]
fn sdk_03_stream_handlers_receive_batches_then_only_the_tail() {
    let (mut host, _root) = example_host("api-demo");
    assert!(host
        .run_command("api-demo", "stream", args(&["echo"]), Snapshot::default())
        .is_err());
    host.set_process_grant("api-demo", ProcessGrant::Any);
    let actions = host
        .run_command("api-demo", "stream", args(&["echo"]), Snapshot::default())
        .unwrap();
    let id = match actions[0] {
        Action::Spawn {
            id, stream: true, ..
        } => id,
        ref other => panic!("{other:?}"),
    };
    for chunk in ["one\n", "two\nthree\n"] {
        let actions = host
            .callback(
                "api-demo",
                Callback::ProcessOutput {
                    id,
                    chunk: chunk.into(),
                },
                &Snapshot::default(),
            )
            .unwrap();
        assert_eq!(statuses(&actions), [format!("chunk:{chunk}")]);
    }
    let actions = host
        .callback(
            "api-demo",
            Callback::ProcessDone {
                id,
                result: Ok(ProcessOutput {
                    exit_code: Some(0),
                    stdout: "tail".into(),
                    stderr: "err".into(),
                }),
            },
            &Snapshot::default(),
        )
        .unwrap();
    assert_eq!(statuses(&actions), ["done:Some(0):tail:err"]);
    assert!(host
        .callback(
            "api-demo",
            Callback::ProcessOutput {
                id,
                chunk: "late\n".into()
            },
            &Snapshot::default()
        )
        .unwrap()
        .is_empty());
}

#[test]
fn finder_streams_files_and_opens_native_splits_without_run_commands() {
    let (mut host, _root) = example_host("finder");
    host.set_process_grant("finder", ProcessGrant::Any);
    let snapshot = snapshot("", 0, 0);
    let actions = host
        .run_command("finder", "files", vec![], snapshot.clone())
        .unwrap();
    let picker = match &actions[0] {
        Action::ShowPicker {
            id,
            generation: None,
            picker,
            ..
        } if picker.items.is_empty() => *id,
        other => panic!("{other:?}"),
    };
    let process = match &actions[1] {
        Action::Spawn {
            id,
            argv,
            stream: true,
            ..
        } if argv == &["rg", "--files"] => *id,
        other => panic!("{other:?}"),
    };
    let actions = host
        .callback(
            "finder",
            Callback::ProcessOutput {
                id: process,
                chunk: "src/a b.rs\n".into(),
            },
            &snapshot,
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::PushItems { items, .. }] if items.len() == 1 && items[0].data == "src/a b.rs")
    );
    let actions = host
        .callback(
            "finder",
            Callback::ProcessDone {
                id: process,
                result: Ok(ProcessOutput {
                    exit_code: Some(0),
                    stdout: "tail.rs".into(),
                    stderr: String::new(),
                }),
            },
            &snapshot,
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::PushItems { items, .. }] if items.len() == 1 && items[0].data == "tail.rs")
    );
    let actions = host
        .callback(
            "finder",
            Callback::PickerDone {
                id: picker,
                result: PickerResult::Accepted((vec![0], OpenAction::Hsplit)),
            },
            &snapshot,
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::OpenFileAt { path, action: OpenAction::Hsplit, line: None, column: None }] if path == Path::new("/work/src/a b.rs"))
    );

    let actions = host
        .run_command("finder", "files", vec![], snapshot.clone())
        .unwrap();
    let picker = match actions[0] {
        Action::ShowPicker { id, .. } => id,
        ref other => panic!("{other:?}"),
    };
    let process = match actions[1] {
        Action::Spawn { id, .. } => id,
        ref other => panic!("{other:?}"),
    };
    let actions = host
        .callback(
            "finder",
            Callback::PickerDone {
                id: picker,
                result: PickerResult::Cancelled,
            },
            &snapshot,
        )
        .unwrap();
    assert!(matches!(&actions[..], [Action::Kill { id, .. }] if *id == process));
    assert!(host
        .callback(
            "finder",
            Callback::ProcessOutput {
                id: process,
                chunk: "late.rs\n".into()
            },
            &snapshot
        )
        .unwrap()
        .is_empty());
    assert!(host
        .callback(
            "finder",
            Callback::ProcessDone {
                id: process,
                result: Err("killed".into())
            },
            &snapshot
        )
        .unwrap()
        .is_empty());
}

#[test]
fn finder_grep_cancels_old_queries_and_opens_the_reported_position() {
    let (mut host, _root) = example_host("finder");
    host.set_process_grant("finder", ProcessGrant::Any);
    let snapshot = snapshot("", 0, 0);
    let actions = host
        .run_command("finder", "grep", vec![], snapshot.clone())
        .unwrap();
    let picker = match actions[0] {
        Action::ShowPicker { id, .. } => id,
        ref other => panic!("{other:?}"),
    };
    let mut processes = Vec::new();
    for generation in [1, 2] {
        host.set_picker_generation("finder", picker, generation);
        let actions = host
            .callback(
                "finder",
                Callback::PickerQuery {
                    id: picker,
                    generation: Some(generation),
                    query: format!("query{generation}"),
                },
                &snapshot,
            )
            .unwrap();
        if generation == 2 {
            assert!(
                matches!(actions.first(), Some(Action::Kill { id, .. }) if *id == processes[0])
            );
        }
        processes.push(
            actions
                .iter()
                .find_map(|action| match action {
                    Action::Spawn {
                        id, stream: false, ..
                    } => Some(*id),
                    _ => None,
                })
                .unwrap(),
        );
    }
    for (index, process) in processes.into_iter().enumerate() {
        let actions = host
            .callback(
                "finder",
                Callback::ProcessDone {
                    id: process,
                    result: Ok(ProcessOutput {
                        exit_code: Some(0),
                        stdout: "src/a b.rs:2:4:héllo\n".into(),
                        stderr: String::new(),
                    }),
                },
                &snapshot,
            )
            .unwrap();
        if index == 0 {
            assert!(actions.is_empty());
        } else {
            assert!(
                matches!(&actions[..], [Action::SetItems { generation: Some(2), items, .. }] if items.len() == 1)
            );
        }
    }
    let actions = host
        .callback(
            "finder",
            Callback::PickerDone {
                id: picker,
                result: PickerResult::Accepted((vec![0], OpenAction::Vsplit)),
            },
            &snapshot,
        )
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::OpenFileAt { path, action: OpenAction::Vsplit, line: Some(2), column: Some(3) }] if path == Path::new("/work/src/a b.rs"))
    );
}

#[test]
fn decorations_are_validated() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), &["read-buffers"]);
    let state = host.plugins[0].store.data_mut();
    state.snapshot = snapshot("ab\ncd\n", 0, 0);
    state.context = CallContext::Event;

    // Valid: one action per layer; signs become line-start positions.
    state
        .set_highlights(1, "words".into(), vec![(0, 2, "warning".into())])
        .unwrap();
    state
        .set_annotations(1, "notes".into(), vec![(2, " x".into(), "comment".into())])
        .unwrap();
    state
        .set_signs(
            1,
            "lines".into(),
            vec![(1, "●more".into(), "warning".into())],
        )
        .unwrap();
    state
        .set_statusline("count".into(), "2\ttodos".into())
        .unwrap();
    let actions = std::mem::take(&mut state.actions);
    assert!(matches!(
        &actions[2],
        Action::Decorate { decorations: Decorations::Signs(signs), .. }
            if signs == &[(3, "●".to_string(), "warning".to_string())]
    ));
    assert!(matches!(
        &actions[3],
        Action::StatusLine { segment, text, .. } if segment == "count" && text == "2 todos"
    ));

    // Invalid: out of range, unknown document, bad names, line breaks.
    let err = |r: Result<(), String>| r.unwrap_err();
    assert!(
        err(state.set_highlights(1, "l".into(), vec![(5, 9, "s".into())]))
            .contains("invalid range")
    );
    assert!(
        err(state.set_highlights(1, "l".into(), vec![(2, 1, "s".into())]))
            .contains("invalid range")
    );
    assert!(err(state.set_highlights(9, "l".into(), vec![])).contains("no document"));
    assert!(err(state.set_highlights(1, "".into(), vec![])).contains("invalid layer"));
    assert!(err(state.set_highlights(1, "a/b".into(), vec![])).contains("invalid layer"));
    assert!(
        err(state.set_annotations(1, "l".into(), vec![(99, "x".into(), "s".into())]))
            .contains("invalid position")
    );
    assert!(
        err(state.set_annotations(1, "l".into(), vec![(0, "a\nb".into(), "s".into())]))
            .contains("line breaks")
    );
    assert!(
        err(state.set_signs(1, "l".into(), vec![(7, "x".into(), "s".into())]))
            .contains("invalid line")
    );
    assert!(err(state.set_statusline("".into(), "x".into())).contains("invalid segment"));
    let long = "x".repeat(MAX_STATUSLINE + 50);
    state.set_statusline("s".into(), long).unwrap();
    assert!(matches!(
        state.actions.last(),
        Some(Action::StatusLine { text, .. }) if text.chars().count() == MAX_STATUSLINE
    ));
}

#[test]
fn todos_example_decorates_documents() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("todos");
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(fixture(&format!("examples/todos/{file}")), dir.join(file)).unwrap();
    }
    host.load(&dir, "{}").unwrap();
    let results = host.dispatch(
        Event::DocumentOpen(1),
        &snapshot("x // TODO a\ny\n// FIXME b\n", 0, 0),
    );
    let actions = results[0].1.as_ref().unwrap();
    let decorations: Vec<_> = actions
        .iter()
        .filter_map(|action| match action {
            Action::Decorate {
                layer, decorations, ..
            } => Some((layer.as_str(), decorations)),
            _ => None,
        })
        .collect();
    assert_eq!(
        decorations[0],
        (
            "words",
            &Decorations::Highlights(vec![(5..9, "warning".into()), (17..22, "error".into())])
        )
    );
    assert!(matches!(decorations[1], ("lines", Decorations::Signs(signs)) if signs.len() == 2));
    // End-of-line notes go at the line breaks (offsets 11 and 24).
    assert!(matches!(
        decorations[2],
        ("notes", Decorations::Annotations(notes))
            if notes.iter().map(|n| n.0).collect::<Vec<_>>() == [11, 24]
    ));
    assert!(actions
        .iter()
        .any(|action| matches!(action, Action::StatusLine { text, .. } if text == "2 todos")));
}

/// Start a background task through testplugin3 and wait for its result.
fn background(host: &mut PluginHost, task: &str, input: &str) -> Result<String, String> {
    let actions = host
        .run_command(
            "testplugin3",
            "background",
            vec![task.into(), input.into()],
            snapshot("main text", 0, 0),
        )
        .unwrap();
    let (id, task, input) = match &actions[..] {
        [Action::RunBackground {
            id, task, input, ..
        }] => (*id, task.clone(), input.clone()),
        other => panic!("expected a background task, got {other:?}"),
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    host.run_background(
        "testplugin3",
        id,
        task,
        input,
        Box::new(move |result| {
            sender.send(result).unwrap();
        }),
    );
    let result = receiver.recv_timeout(Duration::from_secs(20)).unwrap();
    // The result goes back to the plugin's main instance.
    let actions = host
        .callback(
            "testplugin3",
            Callback::BackgroundDone {
                id,
                result: result.clone(),
            },
            &Snapshot::default(),
        )
        .unwrap();
    let expected = match &result {
        Ok(value) => format!("background:{value}"),
        Err(error) => format!("background-error:{error}"),
    };
    assert_eq!(statuses(&actions), [expected]);
    result
}

#[test]
fn background_tasks_run_in_a_separate_instance() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), &["read-buffers"]);
    assert_eq!(background(&mut host, "square", "12"), Ok("144".into()));
    // Calls are timed for :plugin-list.
    let timing = host.info()[0].timing;
    assert!(timing.calls >= 2, "{timing:?}");
    assert!(timing.slowest >= timing.total / timing.calls as u32);
    // Its own memory (the main instance counted 2 commands; the background
    // one 2 tasks), and no documents even with read-buffers.
    assert_eq!(
        background(&mut host, "isolation", ""),
        Ok("runs:2;document:None;text:None".into())
    );
    assert_eq!(
        background(&mut host, "nope", ""),
        Err("unknown task nope".into())
    );
    // A plugin may only have so many at once.
    for _ in 0..MAX_BACKGROUND {
        host.run_command(
            "testplugin3",
            "background",
            vec!["square".into(), "1".into()],
            Snapshot::default(),
        )
        .unwrap();
    }
    let err = host
        .run_command(
            "testplugin3",
            "background",
            vec!["square".into(), "1".into()],
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("at most"), "{err}");
}

#[test]
fn runaway_background_tasks_time_out_without_disabling_the_plugin() {
    let mut host = host_with_limits(Limits {
        background: Duration::from_millis(300),
        ..Limits::default()
    });
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), &["read-buffers"]);
    let started = Instant::now();
    let err = background(&mut host, "spin", "").unwrap_err();
    assert!(err.contains("took longer than 300 ms"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
    // The plugin keeps working, and the next task gets a fresh instance.
    assert!(host.plugins[0].disabled().is_none());
    assert_eq!(background(&mut host, "square", "3"), Ok("9".into()));
}

#[test]
fn background_tasks_are_still_interrupted_after_the_host_is_dropped() {
    let mut host = host_with_limits(Limits {
        background: Duration::from_millis(300),
        ..Limits::default()
    });
    let root = tempfile::tempdir().unwrap();
    load_v3(&mut host, root.path(), &["read-buffers"]);
    let (sender, receiver) = std::sync::mpsc::channel();
    // One spinning task and one queued behind it.
    for _ in 0..2 {
        let actions = host
            .run_command(
                "testplugin3",
                "background",
                vec!["spin".into(), String::new()],
                snapshot("main text", 0, 0),
            )
            .unwrap();
        let [Action::RunBackground {
            id, task, input, ..
        }] = &actions[..]
        else {
            panic!("expected a background task, got {actions:?}");
        };
        let sender = sender.clone();
        host.run_background(
            "testplugin3",
            *id,
            task.clone(),
            input.clone(),
            Box::new(move |result| {
                let _ = sender.send(result);
            }),
        );
    }
    std::thread::sleep(Duration::from_millis(100));
    drop(host);
    let first = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(first.unwrap_err().contains("took longer"));
    let second = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(second, Err("the plugins were reloaded".into()));
}

/// What one Markdown re-render costs (see `examples/markdown`). Run with
/// `cargo test -p helix-plugin --release -- --ignored --nocapture markdown_cost`.
#[test]
#[ignore]
fn markdown_cost() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("markdown");
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            fixture(&format!("examples/markdown/{file}")),
            dir.join(file),
        )
        .unwrap();
    }
    host.load(&dir, "{}").unwrap();
    let book: String = std::fs::read_dir(fixture("../book/src"))
        .unwrap()
        .filter_map(|entry| std::fs::read_to_string(entry.unwrap().path()).ok())
        .collect();
    let mut snapshot = snapshot(&book.repeat(2), 0, 0);
    snapshot.documents.get_mut(&1).unwrap().path = Some("/tmp/notes.md".into());
    let lines = book.repeat(2).lines().count();
    let rounds = 20;
    let started = Instant::now();
    let mut actions = 0;
    for _ in 0..rounds {
        let results = host.dispatch(Event::DocumentChange(1), &snapshot);
        actions = match &results[0].1.as_ref().unwrap()[1] {
            Action::Decorate {
                decorations: Decorations::Overlays(o),
                ..
            } => o.len(),
            _ => 0,
        };
    }
    eprintln!(
        "markdown re-render of {lines} lines ({actions} overlays): {:.2} ms per event",
        started.elapsed().as_secs_f64() * 1000.0 / rounds as f64
    );
}

/// Load `examples/<name>` with `config`.
fn load_example(host: &mut PluginHost, root: &Path, name: &str, config: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(fixture(&format!("examples/{name}/{file}")), dir.join(file)).unwrap();
    }
    host.load(&dir, config).unwrap();
}

#[test]
fn lint_example_reports_diagnostics() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_example(&mut host, root.path(), "lint", "{\"max-line\":5}");
    let results = host.dispatch(
        Event::DocumentOpen(1),
        &snapshot("ok \nlonger line\n", 0, 0),
    );
    let actions = results[0].1.as_ref().unwrap();
    let [Action::Diagnostics { diagnostics, .. }] = &actions[..] else {
        panic!("{actions:?}");
    };
    let found: Vec<_> = diagnostics
        .iter()
        .map(|d| (d.code.as_deref().unwrap(), d.range.clone()))
        .collect();
    assert_eq!(found, [("trailing-space", 2..3), ("line-length", 9..15)]);
}

#[test]
fn snippets_example_offers_snippets_for_the_language() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_example(
        &mut host,
        root.path(),
        "snippets",
        r#"{"snippets": {"rust": {"dbg": "dbg!(${1:value});"}}}"#,
    );
    // `snapshot` is a Rust document.
    let (_, actions) = host
        .request_completion("snippets", 1, 7, &snapshot("let x d", 7, 7))
        .unwrap();
    match &actions.unwrap()[..] {
        [Action::Completions { items, .. }] => {
            assert_eq!(items.len(), 1, "{items:?}");
            assert_eq!(items[0].label, "dbg");
            assert!(items[0].snippet);
            assert_eq!(items[0].insert.as_deref(), Some("dbg!(${1:value});"));
        }
        other => panic!("{other:?}"),
    }
    let (_, actions) = host
        .request_completion("snippets", 1, 2, &snapshot("fn", 2, 2))
        .unwrap();
    assert!(
        matches!(&actions.unwrap()[..], [Action::Completions { items, .. }] if items[0].label == "fn")
    );
}

#[test]
fn emoji_example_completes_after_a_colon() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_example(&mut host, root.path(), "emoji", "{}");
    let (id, actions) = host
        .request_completion("emoji", 1, 8, &snapshot("say :roc", 8, 8))
        .unwrap();
    let actions = actions.unwrap();
    let [Action::Completions {
        id: answered,
        items,
        ..
    }] = &actions[..]
    else {
        panic!("{actions:?}");
    };
    assert_eq!(*answered, id);
    assert_eq!(items[0].label, ":rocket");
    assert_eq!(items[0].insert.as_deref(), Some("🚀"));
    assert_eq!(items[0].replace_from, Some(4));
    // Without a `:name`, an empty answer.
    let (_, actions) = host
        .request_completion("emoji", 1, 3, &snapshot("say", 3, 3))
        .unwrap();
    assert!(
        matches!(&actions.unwrap()[..], [Action::Completions { items, .. }] if items.is_empty())
    );
}

#[test]
fn pomodoro_example_ticks_with_timers() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_example(
        &mut host,
        root.path(),
        "pomodoro",
        "{\"work\":2,\"rest\":1}",
    );
    let actions = host
        .run_command("pomodoro", "start", vec![], Snapshot::default())
        .unwrap();
    let timer = actions
        .iter()
        .find_map(|action| match action {
            Action::SetTimer { id, delay, .. } => Some((*id, *delay)),
            _ => None,
        })
        .unwrap();
    assert_eq!(timer.1, Duration::from_secs(60));
    assert!(actions
        .iter()
        .any(|action| matches!(action, Action::StatusLine { text, .. } if text == "🍅 2m")));
    let actions = host
        .callback(
            "pomodoro",
            Callback::Timer { id: timer.0 },
            &Snapshot::default(),
        )
        .unwrap();
    assert!(actions
        .iter()
        .any(|action| matches!(action, Action::StatusLine { text, .. } if text == "🍅 1m")));
    let actions = host
        .run_command("pomodoro", "stop", vec![], Snapshot::default())
        .unwrap();
    assert!(actions
        .iter()
        .any(|action| matches!(action, Action::StatusLine { text, .. } if text.is_empty())));
}

#[test]
fn yank_history_example_remembers_yanks() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    load_example(&mut host, root.path(), "yank-history", "{}");
    let mut snap = Snapshot::default();
    snap.registers.insert('"', vec!["first".into()]);
    host.dispatch(Event::PostCommand("yank".into()), &snap);
    snap.registers.insert('"', vec!["second".into()]);
    host.dispatch(Event::PostCommand("move_char_left".into()), &snap);
    let actions = host
        .run_command("yank-history", "list", vec![], snap)
        .unwrap();
    let Some(Action::ShowPicker { picker, .. }) = actions.first() else {
        panic!("{actions:?}");
    };
    // Only the yank was remembered.
    assert_eq!(picker.items.len(), 1);
    assert_eq!(picker.items[0].columns, ["first"]);
}

/// API 0.3 components built before 0.4 keep loading and working.
#[test]
fn frozen_api_03_components_still_work() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin3",
        &fixture("tests/fixtures/testplugin3-v0.3.wasm"),
        ALL,
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.3\"");
    std::fs::write(path, manifest).unwrap();
    host.load(&dir, "{}").unwrap();
    let actions = host
        .run_command("testplugin3", "diagnose", vec![], snapshot("hello", 0, 0))
        .unwrap();
    assert!(matches!(&actions[..], [Action::Diagnostics { .. }]));
    let actions = host
        .run_command(
            "testplugin3",
            "timer",
            vec!["5".into()],
            Snapshot::default(),
        )
        .unwrap();
    let [Action::SetTimer { id, .. }] = &actions[..] else {
        panic!("{actions:?}");
    };
    let fired = host
        .callback(
            "testplugin3",
            Callback::Timer { id: *id },
            &Snapshot::default(),
        )
        .unwrap();
    assert!(!fired.is_empty());
    // 0.3 has no file results.
    assert!(host
        .register_request("testplugin3", PendingKind::Files)
        .is_ok());

    // The 0.3 build of oil, with 0.3's file functions.
    let oil = root.path().join("oil");
    std::fs::create_dir_all(&oil).unwrap();
    std::fs::copy(
        fixture("tests/fixtures/oil-v0.3.wasm"),
        oil.join("plugin.wasm"),
    )
    .unwrap();
    let manifest = std::fs::read_to_string(fixture("examples/oil/plugin.toml"))
        .unwrap()
        .replace("api = \"0.5\"", "api = \"0.3\"");
    std::fs::write(oil.join("plugin.toml"), manifest).unwrap();
    host.load(&oil, "{}").unwrap();
    assert!(host
        .commands()
        .iter()
        .any(|(plugin, c)| plugin == "oil" && c.name == "open"));
}

#[test]
fn api_04_file_operations_are_checked_and_answered() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(root.path()).unwrap().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin3",
        &fixture("tests/fixtures/testplugin3.wasm"),
        &["read-buffers", "filesystem"],
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.5\"");
    std::fs::write(path, manifest).unwrap();
    host.load_with(
        &dir,
        PluginSettings {
            config: "{}".into(),
            files: FileGrant::Write,
            dirs: vec![work.clone()],
            ..Default::default()
        },
    )
    .unwrap();
    let a = work.join("a").to_string_lossy().into_owned();
    let b = work.join("b").to_string_lossy().into_owned();
    let actions = host
        .run_command(
            "testplugin3",
            "files",
            vec![
                format!("create:{a}"),
                format!("rename:{a}:{b}"),
                format!("trash:{b}"),
            ],
            Snapshot::default(),
        )
        .unwrap();
    let [Action::FileOps { id, ops, .. }] = &actions[..] else {
        panic!("{actions:?}");
    };
    assert_eq!(
        ops,
        &[
            FileOp::Create {
                path: a.clone(),
                directory: false
            },
            FileOp::Rename {
                from: a.clone(),
                to: b.clone()
            },
            FileOp::Trash(b.clone()),
        ]
    );
    // The editor answers with one result per operation.
    let answered = host
        .callback(
            "testplugin3",
            Callback::FilesDone {
                id: *id,
                results: vec![Ok(()), Err("no".into()), Err("skipped".into())],
            },
            &Snapshot::default(),
        )
        .unwrap();
    assert!(matches!(
        &answered[..],
        [Action::Status(status)] if status == "files-done:ok;err(no);err(skipped)"
    ));
    // Paths outside the grant are refused before anything happens.
    let err = host
        .run_command(
            "testplugin3",
            "files",
            vec!["delete:/etc/passwd".into()],
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("outside"), "{err}");

    // Checked again when they run: a path that left the grant is refused.
    assert_eq!(host.check_writable("testplugin3", &a), Ok(()));
    assert!(host
        .check_writable("testplugin3", "/etc/passwd")
        .unwrap_err()
        .contains("outside"));
    assert!(host
        .check_writable("other", &a)
        .unwrap_err()
        .contains("unloaded"));

    // Only so many batches at once.
    for _ in 0..MAX_FILE_BATCHES {
        host.run_command(
            "testplugin3",
            "files",
            vec![format!("create:{a}")],
            Snapshot::default(),
        )
        .unwrap();
    }
    let err = host
        .run_command(
            "testplugin3",
            "files",
            vec![format!("create:{a}")],
            Snapshot::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("batches"), "{err}");
}

#[test]
fn conceals_are_checked_against_the_text() {
    let mut state = State::background("md", 1 << 20);
    let doc = DocumentId::from(1u32);
    state.snapshot.documents.insert(
        doc,
        DocumentSnapshot {
            text: Rope::from("ae\u{301}b\nnext\n"),
            path: None,
            language: None,
            selection: None,
            modified: false,
            owner: None,
            revision: 0,
            language_servers: Vec::new(),
        },
    );
    state
        .set_conceals(
            doc,
            "c".into(),
            vec![
                // inside `é` (e + combining accent): widened to whole graphemes
                (2, 3, "…x".into(), "s".into()),
                // crosses the line break: cut at the line's end
                (3, 7, "".into(), "s".into()),
                // overlaps the first: dropped
                (1, 2, "".into(), "s".into()),
                // empty: dropped
                (6, 6, "".into(), "s".into()),
            ],
        )
        .unwrap();
    let Some(Action::Decorate { decorations, .. }) = state.actions.pop() else {
        panic!("no decorations");
    };
    assert_eq!(
        decorations,
        Decorations::Conceals(vec![
            (1..3, "…".into(), "s".into()),
            (3..4, "".into(), "s".into()),
        ])
    );
    assert!(state
        .set_conceals(doc, "c".into(), vec![(5, 99, "".into(), "s".into())])
        .is_err());
    state
        .set_render_modes(doc, "c".into(), vec![Mode::Normal])
        .unwrap();
    assert!(matches!(
        state.actions.pop(),
        Some(Action::Decorate {
            decorations: Decorations::RenderModes(modes),
            ..
        }) if modes == [Mode::Normal]
    ));
}

#[test]
fn listeners_need_the_permission_and_the_port() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("competitest");
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            fixture(&format!("examples/competitest/{file}")),
            dir.join(file),
        )
        .unwrap();
    }
    let always = r#"{"always-listen": true}"#;
    // Not granted: init fails and says what to add.
    let err = host
        .load_with(&dir, PluginSettings::new(always))
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("port 27121 is not in allow-listen for [competitest]"),
        "{err:#}"
    );
    // Granted: listening may start from init.
    let settings = PluginSettings {
        listen: vec![27121],
        ..PluginSettings::new(always)
    };
    let actions = host.load_with(&dir, settings).unwrap();
    let [Action::Listen {
        id, port: 27121, ..
    }] = actions[..]
    else {
        panic!("{actions:?}");
    };
    assert!(CAPABILITIES.contains(&"listen"));
    // A request reaches the plugin and keeps the listener open.
    let snapshot = snapshot("", 0, 0);
    let body = Callback::ListenRequest {
        id,
        body: "not json".into(),
    };
    host.callback("competitest", body.clone(), &snapshot)
        .unwrap();
    let log = host.log_lines("competitest").unwrap();
    assert!(
        log.last()
            .unwrap()
            .contains("not a Competitive Companion problem"),
        "{log:?}"
    );
    host.callback("competitest", body, &snapshot).unwrap();
    assert_eq!(host.log_lines("competitest").unwrap().len(), 2);
    // `receive stop` cancels it; later requests are dropped.
    let actions = host
        .run_command(
            "competitest",
            "receive",
            vec!["stop".into()],
            snapshot.clone(),
        )
        .unwrap();
    assert!(
        actions.iter().any(
            |action| matches!(action, Action::StopListen { id: stopped, .. } if *stopped == id)
        ),
        "{actions:?}"
    );
    let late = Callback::ListenRequest {
        id,
        body: "late".into(),
    };
    assert!(host
        .callback("competitest", late, &snapshot)
        .unwrap()
        .is_empty());
    assert_eq!(host.log_lines("competitest").unwrap().len(), 2);
}

#[test]
fn a_failed_listener_reports_its_error() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("competitest");
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["plugin.toml", "plugin.wasm"] {
        std::fs::copy(
            fixture(&format!("examples/competitest/{file}")),
            dir.join(file),
        )
        .unwrap();
    }
    let settings = PluginSettings {
        listen: vec![27121],
        ..PluginSettings::new(r#"{"always-listen": true}"#)
    };
    let actions = host.load_with(&dir, settings).unwrap();
    let [Action::Listen { id, .. }] = actions[..] else {
        panic!("{actions:?}");
    };
    let failed = Callback::ListenFailed {
        id,
        error: "can't listen on 127.0.0.1:27121: Address in use".into(),
    };
    let actions = host
        .callback("competitest", failed, &snapshot("", 0, 0))
        .unwrap();
    assert!(
        matches!(&actions[..], [Action::Error(message)] if message == "CompetiTest: can't listen on 127.0.0.1:27121: Address in use"),
        "{actions:?}"
    );
}

/// testplugin3 (API 0.5) with `settings`, and an editor key check that
/// refuses "bad".
fn v5_with(settings: PluginSettings) -> (PluginHost, tempfile::TempDir) {
    let mut host = host();
    host.set_editor_fns(v05::EditorFns {
        check_keys: Some(|keys| {
            if keys.contains("bad") {
                Err(format!("invalid key in '{keys}'"))
            } else {
                Ok(())
            }
        }),
        commands: None,
    });
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin3",
        &fixture("tests/fixtures/testplugin3.wasm"),
        ALL,
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.5\"");
    std::fs::write(path, manifest).unwrap();
    host.load_with(&dir, settings).unwrap();
    (host, root)
}

fn v5_run(host: &mut PluginHost, command: &str, args: &[&str]) -> anyhow::Result<Vec<Action>> {
    host.run_command(
        "testplugin3",
        command,
        args.iter().map(|a| a.to_string()).collect(),
        snapshot("text\n", 0, 0),
    )
}

/// The bindings the last `KeyBindings` action of `actions` sets.
fn key_bindings(actions: &[Action]) -> Option<Vec<v05::KeyBinding>> {
    actions.iter().rev().find_map(|action| match action {
        Action::KeyBindings { bindings, .. } => Some(bindings.clone()),
        _ => None,
    })
}

#[test]
fn api_05_key_bindings_are_validated_listed_and_removed() {
    let (mut host, _root) = v5_with(PluginSettings::new("{}"));
    let actions = v5_run(
        &mut host,
        "bind",
        &["plugin", "go", "normal", "space", "g", "x"],
    )
    .unwrap();
    let bindings = key_bindings(&actions).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].keys, "space g x");
    assert_eq!(bindings[0].layer, v05::KeyLayer::Plugin);
    assert_eq!(bindings[0].description, "test binding go");
    // The same keys again replace the binding.
    let actions = v5_run(
        &mut host,
        "bind",
        &["plugin", "go2", "normal", "space", "g", "x"],
    )
    .unwrap();
    let bindings = key_bindings(&actions).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].name, "go2");
    // Checked by the editor; the override layer needs the user's grant.
    let err = v5_run(&mut host, "bind", &["plugin", "b", "normal", "bad"]).unwrap_err();
    assert!(
        format!("{err:#}").contains("invalid key in 'bad'"),
        "{err:#}"
    );
    let err = v5_run(&mut host, "bind", &["override", "o", "normal", "w"]).unwrap_err();
    assert!(
        format!("{err:#}").contains("allow-key-override = true"),
        "{err:#}"
    );
    // A command that fails takes its bindings with it.
    assert!(v5_run(&mut host, "fail-after-bind", &[]).is_err());
    let actions = v5_run(&mut host, "list-keys", &[]).unwrap();
    assert_eq!(statuses(&actions), ["keys:space g x=go2"]);
    let actions = v5_run(&mut host, "unbind", &["normal", "space", "g", "x"]).unwrap();
    assert_eq!(key_bindings(&actions).unwrap(), Vec::new());
    assert!(v5_run(&mut host, "unbind", &["normal", "space", "g", "x"]).is_err());
    // At most 256.
    for n in 0..v05::MAX_KEY_BINDINGS {
        v5_run(
            &mut host,
            "bind",
            &["fallback", "k", "normal", &format!("k{n}")],
        )
        .unwrap();
    }
    let err = v5_run(&mut host, "bind", &["fallback", "k", "normal", "one-more"]).unwrap_err();
    assert!(format!("{err:#}").contains("at most 256"), "{err:#}");
}

#[test]
fn api_05_override_layer_with_the_grant_and_bindings_from_init() {
    let settings = PluginSettings {
        key_override: true,
        ..PluginSettings::new("{\"bind-in-init\": true}")
    };
    let (mut host, _root) = v5_with(settings);
    let actions = v5_run(&mut host, "bind", &["override", "o", "normal", "w"]).unwrap();
    let bindings = key_bindings(&actions).unwrap();
    // The one from init is still there.
    let names: Vec<_> = bindings
        .iter()
        .map(|b| (b.name.as_str(), b.layer))
        .collect();
    assert_eq!(
        names,
        [
            ("init-key", v05::KeyLayer::Plugin),
            ("o", v05::KeyLayer::Override)
        ]
    );
}

#[test]
fn api_05_key_callbacks_run_like_commands_within_a_budget() {
    let (mut host, _root) = v5_with(PluginSettings::new("{}"));
    let (outcome, actions) = host
        .key(
            "testplugin3",
            "edit-1",
            "C-k",
            Some(3),
            snapshot("ab", 0, 0),
        )
        .unwrap();
    assert_eq!(outcome, v05::KeyOutcome::Handled);
    assert_eq!(statuses(&actions), ["key:edit-1:C-k:Some(3)"]);
    assert!(actions.iter().any(|a| matches!(a, Action::Edit { .. })));
    let (outcome, _) = host
        .key("testplugin3", "pass-1", "w", None, snapshot("ab", 0, 0))
        .unwrap();
    assert_eq!(outcome, v05::KeyOutcome::PassThrough);
    // UI is allowed, as in a command.
    let (_, actions) = host
        .key("testplugin3", "pick-1", "C-p", None, snapshot("ab", 0, 0))
        .unwrap();
    assert!(actions
        .iter()
        .any(|a| matches!(a, Action::ShowPicker { .. })));
    // Past the budget: an error (the editor passes the keys through) and
    // the plugin is disabled.
    let started = Instant::now();
    assert!(host
        .key("testplugin3", "spin", "C-s", None, snapshot("ab", 0, 0))
        .is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    let err = host
        .key("testplugin3", "edit-2", "C-k", None, snapshot("ab", 0, 0))
        .unwrap_err();
    assert!(err.to_string().contains("disabled"), "{err}");
}

#[test]
fn api_04_plugins_have_no_key_callbacks() {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin3",
        &fixture("tests/fixtures/testplugin3-v0.4.1.wasm"),
        ALL,
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.4\"");
    std::fs::write(path, manifest).unwrap();
    host.load(&dir, "{}").unwrap();
    let err = host
        .key("testplugin3", "x", "w", None, Snapshot::default())
        .unwrap_err();
    assert!(err.to_string().contains("require API 0.5"), "{err}");
}

#[test]
fn api_05_subscriptions_are_validated_and_filter_events() {
    let (mut host, _root) = v5_with(PluginSettings::new("{}"));
    let err = v5_run(&mut host, "subscribe", &["pre-command"]).unwrap_err();
    assert!(format!("{err:#}").contains("filter.commands"), "{err:#}");
    let err = v5_run(&mut host, "subscribe", &["document-change", "@glob", "a{"]).unwrap_err();
    assert!(format!("{err:#}").contains("invalid path glob"), "{err:#}");
    // Language and path filters, checked against the document.
    let actions = v5_run(
        &mut host,
        "subscribe",
        &["document-change", "@lang", "python"],
    )
    .unwrap();
    assert!(actions
        .iter()
        .any(|a| matches!(a, Action::SubscriptionsChanged)));
    assert!(host.wants(EventKind::DocumentChange));
    let rust = snapshot("fn main() {}", 0, 0);
    assert!(host.dispatch(Event::DocumentChange(1), &rust).is_empty());
    let actions = v5_run(
        &mut host,
        "subscribe",
        &["document-change", "@glob", "*.rs"],
    )
    .unwrap();
    let id: u32 = statuses(&actions)[0]
        .strip_prefix("subscribed:")
        .unwrap()
        .parse()
        .unwrap();
    let results = host.dispatch(Event::DocumentChange(1), &rust);
    assert_eq!(results.len(), 1);
    assert!(host
        .log_lines("testplugin3")
        .unwrap()
        .iter()
        .any(|line| line == "event:change:1"));
    // Unsubscribed: no more.
    v5_run(&mut host, "unsubscribe", &[&id.to_string()]).unwrap();
    assert!(host.dispatch(Event::DocumentChange(1), &rust).is_empty());
    // At most 64.
    for _ in 1..v05::MAX_SUBSCRIPTIONS {
        v5_run(&mut host, "subscribe", &["view-enter"]).unwrap();
    }
    let err = v5_run(&mut host, "subscribe", &["view-enter"]).unwrap_err();
    assert!(format!("{err:#}").contains("at most 64"), "{err:#}");
    assert!(host.wants2(v05::SubscriptionEvent::ViewEnter));
    assert!(!host.wants2(v05::SubscriptionEvent::Shutdown));
}

#[test]
fn api_05_hooks_run_in_order_until_one_cancels() {
    let (mut host, _root) = v5_with(PluginSettings::new("{}"));
    v5_run(&mut host, "subscribe", &["pre-command", "@cmd", "write"]).unwrap();
    let result = host.pre_command("write", &snapshot("x", 0, 0));
    assert!(result.cancelled.is_none());
    assert_eq!(result.results.len(), 1);
    // Other commands don't ask the plugin.
    let result = host.pre_command("quit", &snapshot("x", 0, 0));
    assert!(result.results.is_empty());
    v5_run(&mut host, "subscribe", &["pre-save", "@cancel-saves"]).unwrap();
    let result = host.pre_save(1, &snapshot("x", 0, 0));
    assert_eq!(
        result.cancelled,
        Some(("testplugin3".to_string(), "not now".to_string()))
    );
}

#[test]
fn api_05_commands_are_listed_checked_and_reported() {
    let (mut host, _root) = v5_with(PluginSettings::new("{}"));
    host.set_editor_fns(v05::EditorFns {
        check_keys: None,
        commands: Some(|| {
            vec![
                v05::CommandEntry {
                    name: "move_char_left".into(),
                    doc: "Move left".into(),
                    kind: v05::CommandKind::Native,
                },
                v05::CommandEntry {
                    name: "write".into(),
                    doc: "Write".into(),
                    kind: v05::CommandKind::Typable,
                },
            ]
        }),
    });
    let actions = v5_run(&mut host, "count-commands", &[]).unwrap();
    assert_eq!(
        statuses(&actions),
        ["commands:native=true:typable=true:plugin=true:true:true:true"]
    );
    // Unknown commands fail at the call.
    let err = v5_run(&mut host, "exec", &["no_such_command"]).unwrap_err();
    assert!(
        format!("{err:#}").contains("no command 'no_such_command'"),
        "{err:#}"
    );
    let err = v5_run(&mut host, "exec", &["other/thing"]).unwrap_err();
    assert!(format!("{err:#}").contains("no plugin command"), "{err:#}");
    // A known one becomes an action; its result comes back by id.
    let actions = v5_run(&mut host, "exec", &["#3", "move_char_left"]).unwrap();
    let execute = actions.iter().find_map(|action| match action {
        Action::Execute {
            id, name, count, ..
        } => Some((*id, name.clone(), *count)),
        _ => None,
    });
    let (id, name, count) = execute.unwrap();
    assert_eq!((name.as_str(), count), ("move_char_left", Some(3)));
    let snap = snapshot("text\n", 0, 0);
    let actions = host
        .callback2(
            "testplugin3",
            v05::Callback2::CommandDone {
                id,
                result: Err("boom".into()),
            },
            &snap,
        )
        .unwrap();
    assert_eq!(
        statuses(&actions),
        [format!("command-done:{id}:Err(\"boom\")")]
    );
    // Only once.
    let actions = host
        .callback2(
            "testplugin3",
            v05::Callback2::CommandDone { id, result: Ok(()) },
            &snap,
        )
        .unwrap();
    assert!(actions.is_empty());
    // Batches check every step first.
    let err = v5_run(&mut host, "batch", &["write", ";", "nope"]).unwrap_err();
    assert!(format!("{err:#}").contains("no command 'nope'"), "{err:#}");
    let actions = v5_run(&mut host, "batch", &["write", "a.txt", ";", "mode:insert"]).unwrap();
    let steps = actions.iter().find_map(|action| match action {
        Action::Batch { steps, .. } => Some(steps.clone()),
        _ => None,
    });
    assert_eq!(
        steps.unwrap(),
        [
            v05::Step::Command {
                name: "write".into(),
                args: vec!["a.txt".into()],
                count: None
            },
            v05::Step::SetMode(Mode::Insert)
        ]
    );
    let actions = v5_run(&mut host, "set-mode", &["select"]).unwrap();
    assert!(actions
        .iter()
        .any(|a| matches!(a, Action::SetMode(Mode::Select))));
    // State comes from the snapshot.
    let actions = v5_run(&mut host, "views", &[]).unwrap();
    assert_eq!(statuses(&actions), ["views:1:1:80x24:0-2*"]);
    let actions = v5_run(&mut host, "ui-state", &[]).unwrap();
    assert_eq!(
        statuses(&actions),
        ["ui:Mode::Normal:menu=false:prompt=false:picker=false:popup=false:panel=None:keys="]
    );
}

/// Load testplugin3 under each name, with its `config` and extra
/// `plugin.toml` lines, through `load_all`.
fn services_host(
    plugins: &[(&str, &str, &str)],
) -> (PluginHost, Vec<anyhow::Error>, tempfile::TempDir) {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    for (name, _, extra) in plugins {
        let dir = plugin_dir(
            root.path(),
            name,
            &fixture("tests/fixtures/testplugin3.wasm"),
            ALL,
        );
        let path = dir.join("plugin.toml");
        let manifest = std::fs::read_to_string(&path)
            .unwrap()
            .replace("api = \"0.1\"", "api = \"0.5\"");
        std::fs::write(path, format!("{manifest}{extra}\n")).unwrap();
    }
    let configs: Vec<(String, String)> = plugins
        .iter()
        .map(|(name, config, _)| (name.to_string(), format!("{{\"flags\": \"{config}\"}}")))
        .collect();
    let (_, errors) = host.load_all(root.path(), |name| {
        configs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, config)| PluginSettings::new(config))
    });
    (host, errors, root)
}

fn call_as(
    host: &mut PluginHost,
    plugin: &str,
    command: &str,
    args: &[&str],
) -> anyhow::Result<Vec<Action>> {
    host.run_command(
        plugin,
        command,
        args.iter().map(|a| a.to_string()).collect(),
        snapshot("text\n", 0, 0),
    )
}

fn service_call(actions: &[Action]) -> Option<v05::ServiceCall> {
    actions.iter().find_map(|action| match action {
        Action::ServiceCall(call) => Some(call.clone()),
        _ => None,
    })
}

#[test]
fn api_05_services_are_registered_discovered_and_called() {
    let (mut host, errors, _root) = services_host(&[
        ("testplugin3", "provide-echo", ""),
        ("svc-b", "provide-echo provide-private", ""),
        ("svc-c", "provide-echo", ""),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    let discovered = |host: &mut PluginHost, plugin: &str, name: &str| {
        statuses(&call_as(host, plugin, "discover", &[name]).unwrap())[0].clone()
    };
    assert_eq!(
        discovered(&mut host, "testplugin3", "echo"),
        "discovered:svc-b.echo@1.2.3,svc-c.echo@1.2.3,testplugin3.echo@1.2.3"
    );
    // Only the callers a service accepts see it.
    assert_eq!(
        discovered(&mut host, "testplugin3", "svc-b.private"),
        "discovered:svc-b.private@1.0.0"
    );
    assert_eq!(discovered(&mut host, "svc-c", "private"), "discovered:");
    let err = call_as(
        &mut host,
        "svc-c",
        "call",
        &["svc-b.private", "1.0.0", "echo", "1"],
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("doesn't accept calls from 'svc-c'"),
        "{err:#}"
    );
    // A call is served after the caller's call; the reply comes back by id.
    let actions = call_as(
        &mut host,
        "testplugin3",
        "call",
        &["svc-b.echo", "1.2.0", "echo", r#"{"x": 1}"#],
    )
    .unwrap();
    let call = service_call(&actions).unwrap();
    assert_eq!(
        (call.provider.as_str(), call.service.as_str()),
        ("svc-b", "echo")
    );
    let snap = snapshot("text\n", 0, 0);
    let (reply, _) = host.serve(&call, &snap);
    assert_eq!(reply, Ok(r#"{"x": 1}"#.to_string()));
    assert!(host
        .log_lines("svc-b")
        .unwrap()
        .iter()
        .any(|line| line == r#"event:service:testplugin3:svc-b.echo:echo:{"x": 1}"#));
    let actions = host
        .callback2(
            "testplugin3",
            v05::Callback2::ServiceReply {
                id: call.id,
                result: reply,
            },
            &snap,
        )
        .unwrap();
    assert_eq!(
        statuses(&actions),
        [format!(r#"service-reply:{}:Ok("{{\"x\": 1}}")"#, call.id)]
    );
    // Versions: same major, at least the minor.patch asked for.
    for (version, ok) in [
        ("1.0.0", true),
        ("1.2.3", true),
        ("1.3.0", false),
        ("2.0.0", false),
        ("0.9.0", false),
    ] {
        let result = call_as(
            &mut host,
            "testplugin3",
            "call",
            &["svc-b.echo", version, "echo", "1"],
        );
        assert_eq!(result.is_ok(), ok, "{version}: {result:?}");
    }
    let err = call_as(
        &mut host,
        "testplugin3",
        "call",
        &["svc-b.echo", "1", "echo", "1"],
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("major.minor.patch"), "{err:#}");
    // Payloads are JSON.
    let err = call_as(
        &mut host,
        "testplugin3",
        "call",
        &["svc-b.echo", "1.0.0", "echo", "{nope"],
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("the payload is not JSON"),
        "{err:#}"
    );
    let big = format!("\"{}\"", "x".repeat(1 << 20));
    let err = call_as(
        &mut host,
        "testplugin3",
        "call",
        &["svc-b.echo", "1.0.0", "echo", &big],
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("larger than 1 MiB"), "{err:#}");
    let err = call_as(
        &mut host,
        "testplugin3",
        "call",
        &["svc-x.echo", "1.0.0", "echo", "1"],
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("no service 'svc-x.echo'"),
        "{err:#}"
    );
    // The provider's errors and replies that aren't JSON reach the caller.
    for (method, expected) in [
        ("fail", "failed on purpose"),
        ("bad-json", "the reply is not JSON"),
    ] {
        let actions = call_as(
            &mut host,
            "testplugin3",
            "call",
            &["svc-c.echo", "1.0.0", method, "1"],
        )
        .unwrap();
        let (reply, _) = host.serve(&service_call(&actions).unwrap(), &snap);
        assert!(reply.as_ref().unwrap_err().contains(expected), "{reply:?}");
    }
    // Services are registered from init only.
    let err = call_as(&mut host, "testplugin3", "register-late", &[]).unwrap_err();
    assert!(
        format!("{err:#}").contains("only available from init"),
        "{err:#}"
    );
}

#[test]
fn api_05_service_names_are_checked() {
    let (host, errors, _root) = services_host(&[("svc-a", "provide-bad", "")]);
    assert!(host.plugin("svc-a").is_none());
    assert!(
        format!("{:#}", errors[0]).contains("invalid service name 'Bad Name'"),
        "{errors:?}"
    );
}

#[test]
fn api_05_service_chains_refuse_cycles_and_depth() {
    let names = ["testplugin3", "svc-b", "svc-c", "svc-d", "svc-e", "svc-f"];
    let plugins: Vec<_> = names
        .iter()
        .map(|name| (*name, "provide-echo", ""))
        .collect();
    let (mut host, errors, _root) = services_host(&plugins);
    assert!(errors.is_empty(), "{errors:?}");
    let snap = snapshot("text\n", 0, 0);
    // Follow the chain of calls until one fails.
    let follow = |host: &mut PluginHost, payload: &str| {
        let actions = call_as(
            host,
            "testplugin3",
            "call",
            &["svc-b.echo", "1.0.0", "relay", payload],
        )
        .unwrap();
        let mut call = service_call(&actions).unwrap();
        let mut hops = 0;
        loop {
            hops += 1;
            let (reply, actions) = host.serve(&call, &snap);
            match service_call(&actions) {
                Some(next) => call = next,
                None => return (hops, reply),
            }
        }
    };
    // svc-b calls back into testplugin3: a cycle.
    let (hops, reply) = follow(&mut host, r#""testplugin3.echo""#);
    assert_eq!(hops, 1);
    assert_eq!(
        reply,
        Err("service call cycle: testplugin3 -> svc-b -> testplugin3".into())
    );
    // Four plugins deep is fine; the fifth fails.
    let (hops, reply) = follow(&mut host, r#""svc-c.echo svc-d.echo""#);
    assert_eq!((hops, reply), (3, Ok(r#""end""#.into())));
    let (hops, reply) = follow(
        &mut host,
        r#""svc-c.echo svc-d.echo svc-e.echo svc-f.echo""#,
    );
    assert_eq!(hops, 4);
    assert!(reply.unwrap_err().contains("nest at most 4"));
    // Outside a call, the chain starts again.
    assert!(call_as(
        &mut host,
        "svc-b",
        "call",
        &["testplugin3.echo", "1.0.0", "echo", "1"]
    )
    .is_ok());
}

#[test]
fn api_05_unavailable_providers_fail_calls() {
    let (mut host, errors, _root) =
        services_host(&[("testplugin3", "", ""), ("svc-b", "provide-echo", "")]);
    assert!(errors.is_empty(), "{errors:?}");
    let snap = snapshot("text\n", 0, 0);
    let actions = call_as(
        &mut host,
        "testplugin3",
        "call",
        &["svc-b.echo", "1.0.0", "slow", "1"],
    )
    .unwrap();
    let queued = call_as(
        &mut host,
        "testplugin3",
        "call",
        &["svc-b.echo", "1.0.0", "echo", "1"],
    )
    .unwrap();
    let (reply, _) = host.serve(&service_call(&actions).unwrap(), &snap);
    assert!(reply.unwrap_err().contains("disabled"));
    // The call queued before it went fails; new ones fail at once.
    let (reply, _) = host.serve(&service_call(&queued).unwrap(), &snap);
    assert_eq!(reply, Err("plugin 'svc-b' is unavailable".into()));
    let err = call_as(
        &mut host,
        "testplugin3",
        "call",
        &["svc-b.echo", "1.0.0", "echo", "1"],
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("plugin 'svc-b' is unavailable"),
        "{err:#}"
    );
    assert_eq!(
        statuses(&call_as(&mut host, "testplugin3", "discover", &["echo"]).unwrap()),
        ["discovered:"]
    );
}

#[test]
fn api_05_dependencies_load_first() {
    let (host, errors, _root) = services_host(&[
        (
            "aa-user",
            "",
            "dependencies = [\"zz-provider\"]\noptional-dependencies = [\"not-installed\"]",
        ),
        ("zz-provider", "provide-echo", ""),
        ("needs-missing", "", "dependencies = [\"not-installed\"]"),
        ("cycle-a", "", "dependencies = [\"cycle-b\"]"),
        ("cycle-b", "", "dependencies = [\"cycle-a\"]"),
        ("needs-cycle", "", "dependencies = [\"cycle-a\"]"),
    ]);
    let loaded: Vec<&str> = host
        .plugins()
        .iter()
        .map(|p| p.manifest().name.as_str())
        .collect();
    assert_eq!(loaded, ["zz-provider", "aa-user"]);
    let errors: Vec<String> = errors.iter().map(|err| format!("{err:#}")).collect();
    assert!(
        errors.iter().any(|e| e.contains(
            "'needs-missing' needs plugin 'not-installed', which isn't installed or enabled"
        )),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|e| e.contains("dependency cycle")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("'needs-cycle' needs plugin 'cycle-a', which can't load")),
        "{errors:?}"
    );
    assert!(!loaded
        .iter()
        .any(|name| name.starts_with("cycle") || name.starts_with("needs")));
}

#[test]
fn api_05_marks_move_with_edits_and_go_with_their_text() {
    use helix_core::{Rope, Transaction};
    let mut marks = v05::Marks::default();
    let doc = 1;
    let a = marks.set("p", doc, 2, v05::Gravity::Before).unwrap();
    let b = marks.set("p", doc, 2, v05::Gravity::After).unwrap();
    let c = marks.set("p", doc, 5, v05::Gravity::Before).unwrap();
    let other = marks.set("q", doc, 5, v05::Gravity::Before).unwrap();
    let text = Rope::from("abcdefgh");
    // Insert at 2, delete 4..6 (around c).
    let tx = Transaction::change(&text, [(2, 2, Some("XY".into())), (4, 6, None)].into_iter());
    marks.map(doc, tx.changes());
    assert_eq!(marks.get("p", a), Some((doc, 2)));
    assert_eq!(marks.get("p", b), Some((doc, 4)));
    assert_eq!(marks.get("p", c), None);
    assert_eq!(marks.get("q", other), None);
    // Only a plugin's own marks.
    assert_eq!(marks.get("q", a), None);
    assert_eq!(marks.list("p", doc), [(a, 2), (b, 4)]);
    marks.remove("q", a);
    assert_eq!(marks.list("p", doc).len(), 2);
    marks.close(doc);
    assert!(marks.list("p", doc).is_empty());
    // Limits are per plugin, and freed marks count no more.
    for _ in 0..v05::MAX_MARKS {
        marks.set("p", doc, 0, v05::Gravity::Before).unwrap();
    }
    assert!(marks.set("p", doc, 0, v05::Gravity::Before).is_err());
    assert!(marks.set("q", doc, 0, v05::Gravity::Before).is_ok());
    marks.remove_plugin("p");
    assert!(marks.set("p", doc, 0, v05::Gravity::Before).is_ok());
}

const SCHEMA: &str = r#"
[settings.flags]
type = "string"
default = ""
[settings.width]
type = "integer"
default = 10
min = 1
max = 100
description = "Columns"
[settings.mode]
type = "enum"
values = ["fast", "slow"]
default = "fast"
[settings.tags]
type = "string-list"
"#;

/// testplugin3 with `SCHEMA`, loaded with `config` (JSON).
fn schema_host(config: &str) -> (PluginHost, anyhow::Result<Vec<Action>>, tempfile::TempDir) {
    let mut host = host();
    let root = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        root.path(),
        "testplugin3",
        &fixture("tests/fixtures/testplugin3.wasm"),
        ALL,
    );
    let path = dir.join("plugin.toml");
    let manifest = std::fs::read_to_string(&path)
        .unwrap()
        .replace("api = \"0.1\"", "api = \"0.5\"");
    std::fs::write(&path, format!("{manifest}{SCHEMA}")).unwrap();
    let loaded = host.load_with(&dir, PluginSettings::new(config));
    (host, loaded, root)
}

#[test]
fn api_05_settings_are_checked_against_the_schema() {
    // Defaults fill in what the user didn't set.
    let (mut host, loaded, _root) = schema_host(r#"{"width": 20}"#);
    loaded.unwrap();
    let actions = v5_run(&mut host, "config", &[]).unwrap();
    let config: serde_json::Value =
        serde_json::from_str(statuses(&actions)[0].strip_prefix("config:").unwrap()).unwrap();
    assert_eq!(
        config,
        serde_json::json!({"flags": "", "width": 20, "mode": "fast"})
    );
    for (config, error) in [
        (
            r#"{"widht": 20}"#,
            "unknown setting 'widht' (the plugin has: flags, mode, tags, width)",
        ),
        (r#"{"width": "wide"}"#, "width: expected integer"),
        (r#"{"width": 2.5}"#, "width: expected integer"),
        (r#"{"width": 500}"#, "width: 500 is outside 1..=100"),
        (
            r#"{"mode": "medium"}"#,
            "mode: 'medium' is not one of fast, slow",
        ),
        (r#"{"tags": ["a", 1]}"#, "tags: expected string-list"),
    ] {
        let (_, loaded, _root) = schema_host(config);
        let err = format!("{:#}", loaded.unwrap_err());
        assert!(err.contains(error), "{config}: {err}");
    }
    // A schema whose default doesn't fit is the plugin's error.
    let err = Manifest::parse(
        "name = \"p\"\nversion = \"1\"\napi = \"0.5\"\n[settings.n]\ntype = \"integer\"\ndefault = \"x\"\n",
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("setting 'n': default: expected integer"),
        "{err:#}"
    );
    let err = Manifest::parse(
        "name = \"p\"\nversion = \"1\"\napi = \"0.5\"\n[settings.e]\ntype = \"enum\"\n",
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("an enum needs values"),
        "{err:#}"
    );
}

#[test]
fn api_05_plugins_hear_about_settings_changes() {
    let (mut host, loaded, _root) = schema_host(r#"{"flags": "subscribe-config"}"#);
    loaded.unwrap();
    let snap = snapshot("text\n", 0, 0);
    // Unchanged (after defaults): nothing.
    let same = r#"{"flags": "subscribe-config", "width": 10}"#;
    assert!(host
        .set_config("testplugin3", same, &snap)
        .unwrap()
        .is_none());
    let changed = r#"{"flags": "subscribe-config", "width": 30}"#;
    assert!(host
        .set_config("testplugin3", changed, &snap)
        .unwrap()
        .is_some());
    let actions = v5_run(&mut host, "config", &[]).unwrap();
    assert!(
        statuses(&actions)[0].contains(r#""width":30"#),
        "{actions:?}"
    );
    // Invalid settings are refused and keep the old ones.
    let err = host
        .set_config("testplugin3", r#"{"width": 0}"#, &snap)
        .unwrap_err();
    assert!(format!("{err:#}").contains("outside 1..=100"));
    let actions = v5_run(&mut host, "config", &[]).unwrap();
    assert!(statuses(&actions)[0].contains(r#""width":30"#));
    // A plugin that doesn't listen isn't called.
    let (mut host, loaded, _root) = schema_host("{}");
    loaded.unwrap();
    assert!(host
        .set_config("testplugin3", r#"{"width": 30}"#, &snap)
        .unwrap()
        .is_none());
}

#[test]
fn api_05_tracing_and_the_doctor_report() {
    let (mut host, _root) = v5_with(PluginSettings::new("{}"));
    assert_eq!(host.set_trace(Some("nope"), true), 0);
    assert_eq!(host.set_trace(Some("testplugin3"), true), 1);
    v5_run(&mut host, "bind", &["plugin", "go", "normal", "space", "g"]).unwrap();
    let log = host.log_lines("testplugin3").unwrap();
    let trace = log
        .iter()
        .find(|line| line.starts_with("trace: command call, "))
        .unwrap();
    assert!(trace.ends_with(" actions"), "{trace}");
    host.set_trace(None, false);
    let before = host.log_lines("testplugin3").unwrap().len();
    v5_run(&mut host, "caps", &[]).unwrap();
    assert_eq!(host.log_lines("testplugin3").unwrap().len(), before);
    let report = host.doctor();
    for part in [
        "## testplugin3 1.0.0 (API 0.5)",
        "Running.",
        "- Permissions: read-buffers, edit-buffers, open-files, run-commands",
        "- Granted in plugins.toml: none",
        "`space g` → go (Plugin)",
        "- Calls: ",
    ] {
        assert!(report.contains(part), "{part} not in:\n{report}");
    }
}
