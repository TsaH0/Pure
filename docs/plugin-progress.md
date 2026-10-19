# Plugin roadmap progress

Task list and design: [`plugin-roadmap.md`](plugin-roadmap.md),
[`plugin-processes-and-pickers.md`](plugin-processes-and-pickers.md).

## Status

- Done: M0.1–M0.5 (API 0.2 groundwork), M1.1–M1.4 (processes and the
  terminal), M2.1–M2.3 and M2.5 (pickers).
- Next: M2.4 (live pickers), M3 (popups and prompts), M4 (file access),
  M6.1 (more events), M9.1–M9.2 (fuzzing, benchmarks).
- Needs the owner's design review first: M5, M6.2, M6.3, M7, M8 and the APIs
  for file-manager style plugins.
- Scope rule from the owner: Pure ships the plugin API only, no bundled
  end-user plugins (no file manager, no yazi integration). The built-in yazi
  command was removed.

## Open questions for the owner

- Should Pure also ask once per program the first time a plugin runs it, on
  top of `allow-process` in `plugins.toml`? Only the config grant exists now.
- Should a plugin's `run-command` (API 0.1's `run-commands` permission, which
  can run `:sh`, `:write`, ...) also need the `process` grant? Unchanged for
  now.

## M0 API 0.2 groundwork (done)

- `wit/v0.1/` (byte-identical to the original API) and `wit/v0.2/` (adds
  request ids, `process`, `ui` and the `callbacks` export).
- `helix-plugin/src/api.rs`: bindings for both worlds; both call the same
  `State` methods. `api` in `plugin.toml` picks the world.
- Request ids from one process-wide counter (never reused, even across
  reloads), one-shot callbacks with the command time limit and a fresh
  snapshot, and call contexts: UI only from commands and callbacks.
- SDK 0.2 (`helix-plugin-api`): closures for results, exported callbacks;
  the `api-v0-1` feature builds the old ABI. `text-tools` builds unchanged.
- Test plugin `tests/plugins/testplugin2`. The frozen
  `tests/fixtures/text-tools-v0.1.wasm` checks 0.1 compatibility.
- Review fixes: `register_request` is test-only (`#[doc(hidden)]`), the SDK
  lockfile is not committed, and `build-plugins.sh` strips local paths from
  the components.

## M1 Processes and the terminal (done)

- `process` permission plus the user's grant: `allow-process = true` or a
  list of programs under the plugin's table in `plugins.toml`
  (`ProcessGrant`, `PluginHost::set_process_grant`). Shown by `:plugin-list`.
  Programs can't be started from `init`.
- `spawn`: `helix-term/src/plugins/process.rs` runs the program without a
  shell, with stdin, cwd, a timeout (default 30 s, max 10 min), 16 MiB
  output caps and lossy UTF-8. If a child keeps the pipes open after the
  program exits, output collection stops after 500 ms. `kill` and reloading
  plugins kill programs. At most 16 at once per plugin.
- `run-interactive`: hands the terminal over like `:run-interactive`;
  `{chooser}` becomes a fresh private (0600) temp file whose contents are
  returned and which is then deleted; `wait-for-key` works like `space z`.
- Results reach the plugin through a new job callback that also gets
  `Jobs` (`job::Callback::EditorCompositorJobs`), so callbacks can run
  editor commands.
- Tests: host tests for permission, grant, allow-list, the process limit,
  kill and delivery; executor unit tests (exit code, both streams, stdin,
  cwd, lossy UTF-8, timeout with a child holding the pipes, kill, missing
  program, chooser files); an in-editor test running `sh` through a plugin
  with an allow-list.

## M2 Pickers (done except M2.4)

- M2.1: `Picker::with_on_close` (cancel hook, once, never after an accept,
  including Alt-Enter) and `Picker::not_reopenable` (answered plugin
  pickers aren't kept for `space '`).
- M2.2: `helix-term/src/plugins/picker.rs` shows a plugin's picker with up
  to 8 columns, file previews and the initial item; Enter, `C-s`, `C-v`
  and Esc report the original index and the split, or a cancel. Text
  previews are not shown yet.
- M2.3: `push-items` (SDK `editor::push_items`) adds items to an open
  picker; the SDK keeps the items so the callback gets pushed ones too.
- Tests: host tests for pickers and `push-items`; in-editor tests for every
  key, filtering, and that `space '` doesn't reopen an answered picker.
