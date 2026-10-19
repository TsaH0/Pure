# Plugin roadmap progress

Task list and design: [`plugin-roadmap.md`](plugin-roadmap.md),
[`plugin-processes-and-pickers.md`](plugin-processes-and-pickers.md).

## Status

- Done: M0 (API 0.2 groundwork), M1 (processes and the terminal), M2
  (pickers, including live pickers), M3.1–M3.2 (popups, prompts), M4 (file
  access, workspace info), M6.1 (save, close and selection events).
- Also done: M3.3 (`:plugin-log`), M9.1 (property tests), M9.2
  (benchmarks).
- Next: M11 hardening, then M10 (API 0.3), including M11.3 query generations.
- Needs the owner's design review first: M5, M6.2, M6.3, M7, M8 and the APIs
  for file-manager style plugins.
- Owner's rules: nothing built into the editor (the built-in yazi command was
  removed); plugins written to try the API are kept as examples in
  `helix-plugin/examples/` (`text-tools`, `yazi`, `finder`).

## Open questions for the owner

- ~~Per-program prompt~~: decided no; `allow-process` is the only gate.
- Should a plugin's `run-command` (API 0.1's `run-commands` permission, which
  can run `:sh`, `:write`, ...) also need the `process` grant? Unchanged for
  now.

- **M5 decorations proposal:** plugin-owned ids and per-document ranges for
  highlights and virtual text; opt-in gutter/statusline entries. Reuse the
  existing annotation renderer and invalidate only when a plugin changes its
  data. Owner review should settle lifetimes, priority and theme scopes first.
- **M6 bindings/commands proposal:** optional manifest defaults merged below
  user bindings, with an explicit per-plugin disable switch; command aliases
  resolve through the existing command dispatcher. Review naming and conflict
  handling before implementing either feature; preserve existing bindings.
- **M7 background work proposal:** explicit opt-in jobs receive immutable input
  and return bounded results through callbacks, with cancellation on reload.
  Research WASI 0.3 separately against the pinned Rust/Wasmtime versions;
  host async execution for time limits does not require a new guest ABI.
- **M8 distribution proposal:** stage downloads, display requested grants,
  verify a pinned commit and component digest, then enable explicitly. Review
  update trust and rollback rules before adding install/update/remove commands.
- **File-manager API proposal:** compare an editable scratch buffer with
  plugin-owned views. A scratch buffer can reuse editing and save hooks;
  persistent views require a larger rendering and focus API. Agree on the
  interaction model and upstream diff budget before implementation.

## Review-fix regression coverage

Added real-component and in-editor tests for the `b1519e5` fixes: closing an
edited document, successive event subscribers seeing earlier edits, bounded
`:plugin-run` recursion with recovery, ordered picker actions, replacement
cancellation, Alt-Enter, literal `--`, and `name = false`. File-grant tests use
explicit paths without changing the process environment. The stale-load test
exercises the actual installation callback with both stale and current
loads, checking that stale actions, errors and hosts cannot replace a reload.
All test components and examples were rebuilt with `build-plugins.sh`.
Validation: 57 host tests (one benchmark ignored), 29 editor library tests,
34 in-editor plugin tests, and three private integration-backend tests passed.
Workspace formatting and `cargo clippy --workspace --all-targets` passed
without warnings. Removed two redundant event clones and unused test-case
storage reported by the checks; line-ending conversion still happens when
constructing a test case. The documented full suite completed with 224 passes,
zero failures and eight `commands::movement` tests filtered out.

## M11.1 Bounded WASI calls

The host uses async Wasmtime exports and WASI imports, driven by a parked
current-thread Tokio runtime on the existing scoped call thread. A wall-clock
timeout covers suspended imports and instantiation; epoch interruption still
bounds CPU loops. The epoch ticker runs only while a call is active. Neither
the runtime nor ticker adds periodic idle work or changes the guest ABI.
WASI filesystem operations run off the call thread. Host shutdown uses
`shutdown_background`, so an uncancellable OS FIFO open cannot hang reload or
exit; the remaining worker lifetime is recorded as a roadmap TODO.

Validation: 60 host tests (one benchmark ignored), 29 editor library tests,
and 35 in-editor plugin tests passed. New host tests bound their own runtime
and exercise sleep in both ABIs, init/events/callbacks, a blocked FIFO open,
and host destruction. A tmux check with scratch configuration showed the
2-second timeout after 2.12 seconds, followed by successful editing and saving.
Formatting, workspace Clippy (no warnings), and the normal editor build passed.
Release benchmark: cold loads for 1 / 5 / 20 plugins took
57.0 / 233.4 / 932.6 ms; warm loads took 1.6 / 4.2 / 14.9 ms (within the
5 ms per cached plugin budget). One event dispatch took 0.078 ms.

## M11.2 Cumulative picker limits

The host counts initial and pushed items for each picker, refuses a push
beyond one million without recording an action, resets the count when items
are replaced, and drops it on completion or a trap. A failed command restores
the previous counts alongside its pending requests. Tests exercise the real
million-item boundary, replacement, completion, and guest-command rollback.
Validation: 62 host tests, 29 editor library tests and 35 in-editor plugin
tests passed; fixtures rebuilt, formatting and workspace Clippy passed with
no warnings.

## M11.4 Process-grant trust boundary

The guide explains that `allow-process` checks only the requested program,
not its children or options (`rg --pre`, git shell aliases), and grants the
program the user's filesystem/network rights. Workspace writes can change
git hooks; legacy `run-commands` also requires trust. No prompts or permission
semantics changed. Documentation reviewed; formatting and workspace Clippy
passed with no warnings. No runtime code changed.

## M11.5 Chooser and compiled-cache cleanup

Chooser files now have an owning guard captured by the terminal request;
dropping a queued request (including on editor shutdown) removes its file,
as does normal completion. Successful compiled-cache reads/writes remove
older hash-named versions of the same plugin, preserving other plugins and
unrecognized files. Failed writes remove their temporary file, including
when the final rename fails.

Validation: 64 host tests, 30 editor library tests and 35 in-editor plugin
tests passed; a targeted test forces a real cache rename failure and checks
cleanup. A tmux chooser command returned its result, removed the chooser and
restored the editor. Formatting, workspace Clippy (no warnings) and a normal
editor build passed. Separate chooser-read and queued-request issues are
recorded in the roadmap for follow-up.

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

## M2.4, M3, M4, M6.1 (done)

API 0.2 was extended in place (it is not released yet):

- Prompts (`show-prompt` → `on-prompt-done`) and popups (`show-popup`,
  markdown at the cursor). `helix-term/src/plugins/prompt.rs`.
- Live pickers: `live` pickers send what the user types to the plugin
  (`on-picker-query`, debounced, through a hidden primary column) and the
  plugin answers with `set-items`. The query callback doesn't complete the
  request. Pickers now make an injector when they need one (a kept one
  showed "(running)" forever).
- File access: `filesystem` permission plus `allow-files = "read" |
  "write"`; the workspace is preopened at its real path. Every plugin gets a
  read-write `data-dir` (`<data dir>/plugins/<name>`). Settings and grants
  are passed at load time (`PluginSettings`, `PluginHost::load_with`,
  `set_dirs`).
- `workspace-root`, `cwd`, `config-dir`, `data-dir`, `document-modified`.
- Events `document-save` (from `Application::handle_document_write`),
  `document-close`, `selection-change` (debounced), exported by 0.2 plugins
  in a separate `events` interface.
- SDK: `prompt`, `popup`, `pick_live` (handler gets the picker id and may
  answer later), `set_items`, `Item::new`, `Item::with_file_preview`,
  handlers for the new events.
- Examples: `examples/yazi` (chooser through `run-interactive`) and
  `examples/finder` (files, live grep, git branches), checked by hand in
  tmux.
- Tests: 52 host tests, 26 in-editor plugin tests (prompts, live picker,
  the three events), full suite as in the roadmap's "Commands".

## M3.3, M9.1, M9.2 (done)

- `:plugin-log <plugin>` opens the plugin's last 1000 `log` lines in a new
  buffer (`PluginHost::log_lines`).
- Property tests (quickcheck, `helix-plugin/src/test.rs`, `mod properties`):
  plugin edits apply like a simple model or are rejected exactly when they
  overlap or are out of range; selections and `text-range` never panic.
- Benchmarks: `cargo test -p helix-plugin --release -- --ignored --nocapture
  benchmarks`. On this machine (4 cores), loading `text-tools`:

  | Plugins | Cold cache | Warm cache |
  | --- | --- | --- |
  | 1 | 48 ms | 1.5 ms |
  | 5 | 218 ms | 5.3 ms |
  | 20 | 936 ms | 19.7 ms |

  Dispatching one event to one plugin (with a 10 000-line document): 0.05 ms.
  Warm-cache startup is within the 5 ms per plugin budget; cold-cache
  compilation is sequential (a TODO).

## Performance (done)

Goal from the owner: plugins must never make Pure slower than vanilla Helix.

- **Startup:** plugins load in the background (`load_in_background`):
  compiling and `init` run on a blocking thread; the editor draws right away
  and plugins get `document-open` for files opened meanwhile. Without a
  plugin directory (or with an empty one) no plugin host or engine is
  created at all. Components compile in parallel on a cold cache.
- **Idle:** the epoch ticker (time limits) parks unless a plugin call runs:
  0 wake-ups while idle (it used to wake every 5 ms).
- **Typing and moving:** editor hooks check a lock-free bitmask of the
  events plugins subscribed to and return at once if none did.
- **Plugin calls:** the workspace root is cached per working directory.
- Benchmark (release, 4 cores, `text-tools`): warm cache 1.4 / 2.9 / 12.6 ms
  for 1 / 5 / 20 plugins (all off the startup path now); cold cache 56 /
  231 / 731 ms; one event to one plugin 0.05 ms.
