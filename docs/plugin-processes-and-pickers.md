# Plugin API 0.2: processes, the terminal and pickers

Status: **implemented** (M0–M2 of [`plugin-roadmap.md`](plugin-roadmap.md),
except live pickers and text previews). Pure ships no example plugins built
on this (no yazi, file finder or git plugin): users write their own, see
`helix-plugin/README.md`. The "Examples to ship" section below is superseded.

This is the next step after phases 1–3
of [`plugin-system.md`](plugin-system.md). It lets plugins do what yazi, fzf,
lazygit and Telescope-style integrations need:

1. **Processes and the terminal**: run a program in the background and get its
   output (`spawn`), or hand it the whole terminal and get back what it chose
   (`run-interactive`).
2. **Pickers**: show items in Pure's own picker (fuzzy matching, preview,
   splits) and get back what the user chose.

With both, a yazi plugin is ~20 lines and an "fzf for anything" plugin is
`spawn` + `show-picker`.

## The problem: everything here is asynchronous

Today a plugin call is synchronous: the editor calls the plugin, the plugin
reads state and queues actions, the call returns (within 2 s) and the editor
applies the actions. A program the user is looking at, or a picker waiting for
a choice, can take minutes, and the editor must keep running meanwhile.

So these functions **start** an operation and return a **request id** at once.
The editor runs the operation and, when it finishes, calls the plugin again
through a new export with the id and the result. That is a second, ordinary
synchronous plugin call: it can read buffers, edit, open files, run commands,
or start the next operation (pick a file → spawn a program on it).

```
:plugin-run fzf/files
  └─ run-command("files")
       spawn(["rg", "--files"]) ──► id 1                 (returns at once)
                                     … rg runs on a tokio task …
  └─ on-process-done(1, output)
       show-picker({items: lines}) ──► id 2
                                     … user picks …
  └─ on-picker-done(2, accepted([7]), vsplit)
       open-file(item 7)
```

This matches how the host already works (queued actions applied after the
call) and needs no WASI 0.3 async: plugins stay simple synchronous components.
The Rust SDK hides the ids behind closures:

```rust
editor::spawn(&["rg", "--files"], Default::default(), |out| {
    let items = out?.stdout.lines().map(Item::text).collect();
    editor::pick(Picker::new("Files", items), |choice| {
        for item in choice.items { editor::open_file(&item.data)?; }
        Ok(())
    })
})
```

## WIT changes (`helix:plugin@0.2.0`)

Additive: everything in 0.1 stays as it is.

```wit
interface types {
  // ... 0.1 types ...
  type request-id = u32;

  /// How to open something the user picked, like the picker's keys:
  /// Enter, C-s, C-v.
  enum open-action { replace, hsplit, vsplit }
}

/// Needs the `process` permission (see "Permissions").
interface process {
  use types.{request-id};

  record spawn-options {
    /// Working directory; defaults to the editor's.
    cwd: option<string>,
    /// Written to the program's stdin, which is then closed.
    stdin: option<string>,
    /// Kill the program after this long. Default 30 s, at most 10 min.
    timeout-ms: option<u32>,
  }

  record interactive-options {
    cwd: option<string>,
    /// Replace the argument `{chooser}` with the path of a fresh temporary
    /// file and return that file's contents (yazi's `--chooser-file`,
    /// lazygit's, fzf's `> file`).
    chooser-file: bool,
    /// Run on the normal screen and wait for a key afterwards, like
    /// `space z` / Vim's `:!`.
    wait-for-key: bool,
  }

  record output {
    /// `none` if the program was killed by a signal or the timeout.
    exit-code: option<s32>,
    /// Captured output (spawn) or the chooser file (run-interactive).
    /// Lossy UTF-8, at most 16 MiB each.
    stdout: string,
    stderr: string,
  }

  /// Run `argv` (no shell: `argv[0]` is the program) in the background.
  /// The result arrives in `on-process-done`.
  spawn: func(argv: list<string>, options: spawn-options) -> result<request-id, string>;

  /// Hand the terminal to `argv` (like the built-in yazi). The result
  /// arrives in `on-process-done` after the program exits and the editor
  /// is redrawn. Only from commands and callbacks, not event handlers.
  run-interactive: func(argv: list<string>, options: interactive-options) -> result<request-id, string>;

  /// Kill a running `spawn`. Its `on-process-done` still arrives.
  kill: func(id: request-id);
}

/// Needs no permission (it only shows what the plugin gives it), but only
/// works from commands and callbacks, not event handlers.
interface ui {
  use types.{request-id};

  variant preview {
    /// A file, optionally with lines (0-based, inclusive) highlighted.
    file(tuple<string, option<tuple<u32, u32>>>),
    text(string),
  }

  record item {
    /// One string per column.
    columns: list<string>,
    /// Handed back when chosen; e.g. a path or an id.
    data: string,
    preview: option<preview>,
  }

  record picker {
    title: string,
    /// Column headers; one column is fine.
    columns: list<string>,
    items: list<item>,
    /// Index of the initially selected item.
    initial: option<u32>,
  }

  variant picker-result {
    /// Indices into `items` (more than one when the user selected several).
    accepted(tuple<list<u32>, open-action>),
    cancelled,
  }

  show-picker: func(picker: picker) -> result<request-id, string>;
  /// Add items to an open picker (for output that arrives in parts).
  push-items: func(id: request-id, items: list<item>) -> result<_, string>;
}

/// New exports. A plugin built for 0.2 must export these (the SDK provides
/// them).
interface callbacks {
  use types.{request-id};
  use process.{output};
  use ui.{picker-result};

  on-process-done: func(id: request-id, result: result<output, string>);
  on-picker-done: func(id: request-id, result: picker-result);
}

world helix-plugin {
  import host;
  import process;
  import ui;
  export plugin;
  export callbacks;
}
```

### Compatibility

A 0.1 component imports `host@0.1.0` and lacks the `callbacks` export, so it
cannot instantiate against the 0.2 world. The host generates bindings for both
worlds (`bindgen!` twice, in `v0_1` and `v0_2` modules) and picks one from
`api` in `plugin.toml`. The 0.1 bindings stay a thin shim over the same host
state, so existing plugins (and `text-tools`) keep working unchanged.

## Permissions

`process` is different from the existing permissions. A program runs with the
user's full rights, outside the sandbox. So declaring it in `plugin.toml` is
not enough. The user must also grant it in `plugins.toml`, optionally limited to
some programs:

```toml
[yazi]
allow-process = true                 # any program
[fzf]
allow-process = ["rg", "fd", "fzf"]  # only these (argv[0], looked up in PATH)
```

Without the grant the plugin still loads; `spawn` / `run-interactive` return
an error naming the missing setting, and `:plugin-list` shows "process: not
granted". `show-picker` needs no permission.

## Host side

- **Pending requests**: `PluginHost` keeps a per-plugin counter for ids and a
  table `id → kind`. `:plugin-reload` and disabling a plugin drop its table,
  so late results are ignored. At most 16 running processes per plugin.
- **New `Action`s**: `Spawn { plugin, id, argv, options }`,
  `RunInteractive { .. }`, `ShowPicker { plugin, id, picker }`,
  `PushItems { .. }`, `Kill { .. }`. `plugins::apply` (helix-term) turns them
  into:
  - `Spawn`: a `tokio::process::Command` on a task (stdin, timeout,
    output caps). On exit it sends a `job::dispatch` callback that calls
    `PluginHost::callback(plugin, Callback::ProcessDone { id, result })` and
    applies the actions it returns.
  - `RunInteractive`: `interactive::request` with an `Interactive` whose
    `on_exit` reads the chooser file and does the same `callback` call (the
    yazi code generalised; built-in `yazi` can use it too).
  - `ShowPicker`: a `Picker<PluginItem, _>` with the given columns and
    preview, pushed with `overlaid`. Its accept callback (and a close hook for
    `cancelled`) calls `callback(.., Callback::PickerDone { .. })`.
    `push-items` uses the picker's injector, like the global search does.
- **Where they may be called**: `run-interactive` and `show-picker` only
  during `run-command` and callbacks (user-initiated), as `run-command` is
  today; from an event handler they return an error, so nothing pops up
  while typing. `spawn` works everywhere (linters on change).
- **Limits**: callbacks get the command time limit (2 s). Program output is
  capped at 16 MiB per stream; pickers at 1 000 000 items.

## SDK (helix-plugin-api 0.2)

- `editor::spawn(argv, opts, FnOnce(Result<Output>) -> Result<()>)`,
  `editor::run_interactive(..)`, `editor::pick(Picker, FnOnce(Choice) -> ..)`.
- A `thread_local!` table `id → Box<dyn FnOnce>` in the SDK, filled by those
  functions and drained by the generated `on-process-done` /
  `on-picker-done`, so plugin authors never see ids.
- `register_plugin!` exports `callbacks` automatically.

## Examples to ship

- **`yazi`**: `run-interactive(["yazi", <current file>, "--chooser-file={chooser}"])`
  → `open-file` each line. Proves step 1 and replaces nothing built-in.
- **`files`**: `spawn(["rg", "--files"])` → `show-picker` → open with the
  chosen split. Proves `spawn` + pickers.
- **`git-branches`**: `spawn(["git", "branch", "--format=%(refname:short)"])`
  → picker → `spawn(["git", "switch", name])` → status message. Proves
  chaining.

## Tests

- helix-plugin unit tests with a test component calling `spawn` on
  `sh -c "echo hi; echo err >&2; exit 3"`, checking the callback gets
  `exit-code 3`, both streams and that later actions apply; timeout and
  `kill`; output caps; ids dropped after reload; permission and grant
  errors; `run-interactive` rejected from an event handler.
- helix-term integration tests (the existing harness): a plugin command that
  opens a picker; `<ret>`, `<C-v>` and `<esc>` deliver `accepted` with the
  right action / `cancelled`; `push-items` shows up; a 0.1 plugin still
  loads beside 0.2 ones.
- `run-interactive` checked by hand in tmux (the test harness has no real
  terminal), plus a unit test of the chooser-file substitution.

## Order of work (each one mergeable)

1. **0.2 plumbing**: WIT 0.2, the dual-world host, request ids, the
   `callbacks` export, SDK closures. No new abilities yet; 0.1 plugins
   unchanged.
2. **`spawn` + `kill`** with the `process` grant, tests, `git-branches`
   (status-only at first).
3. **`run-interactive`**, sharing code with the built-in yazi, plus the
   `yazi` example.
4. **Pickers**: `show-picker`, `push-items`, previews, the `files` example,
   integration tests.

## Open questions

- Should `allow-process = true` be enough, or should the first use of a
  program ask the user once (like workspace trust) and remember the answer?
- Live pickers that re-query on each keystroke (live grep) need a
  `on-picker-query` callback; left for later.
