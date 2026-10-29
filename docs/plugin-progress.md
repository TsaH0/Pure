# Plugin roadmap progress

Task list and design: [`plugin-roadmap.md`](plugin-roadmap.md),
[`plugin-processes-and-pickers.md`](plugin-processes-and-pickers.md).
Future capabilities: [`plugin-future-roadmap.md`](plugin-future-roadmap.md).

## Status (2026-10-20)

- Done: M0–M12. API 0.1 and 0.2 are frozen (compatibility fixtures in
  `helix-plugin/tests/fixtures`); API 0.4 is the current one (0.3 still
  gets new imports).
- Still open: the Future TODOs in the roadmap (one item is blocked
  upstream), plus the 22 correctness/performance findings in
  [`suggested-improvements.md`](suggested-improvements.md). Proposed API
  extensions are tracked separately in
  [`plugin-future-roadmap.md`](plugin-future-roadmap.md). Conceal is done
  (render-markdown.nvim's behaviour, the owner's choice); the review identifies
  additional invalidation and scaling work.
- Owner's rules: nothing built into the editor (the built-in yazi command was
  removed); plugins written to try the API are kept as examples in
  `helix-plugin/examples/` (`text-tools`, `finder`, `api-demo`, `todos`,
  `markdown`, `js-upper`, `oil`, `lint`, `emoji`, `pomodoro`,
  `yank-history`). The `yazi` example was removed at the owner's request
  (2026-10-20) and replaced by `oil`.
- Sections below are a log, oldest first after this one; numbers in them
  are from the day they were measured.

## Open questions for the owner

- ~~Per-program prompt~~: decided no; `allow-process` is the only gate.
- ~~Should a plugin's `run-command` that starts a program also need the
  `allow-process` grant?~~ Done: commands that start programs (`:sh`,
  `:pipe`, `:pipe-to`, `:insert-output`, `:append-output`,
  `:run-interactive`, `:run-and-wait`/`!`, `:compile`, `:grep` and the
  matching static commands; aliases resolve) need `allow-process = true`
  (a list of programs isn't enough). The editor tells the host which
  commands those are (`PluginHost::set_program_check`,
  `plugins::starts_program`).
- The M5–M8 and file-manager proposals that were here were reviewed and
  implemented (M5 decorations, M6 bindings and commands, M7 background
  tasks, M8 distribution, M12 plugin buffers and file operations).

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

## M10 SDK 0.3 and finder migration

The SDK now generates the 0.3 world by default, with mutually exclusive
`api-v0-1` and `api-v0-2` legacy features. Version-specific callback exports
cover popup closure, output batches, query generations and the four new events.
The SDK routes streaming batches without completing their request, removes
stream handlers on final completion, and journals live picker changes on failed
commands. Generation helpers drop obsolete batches and closed picker updates;
the host independently covers input before the debounced guest notification.

Finder's files command installs an empty picker before starting `rg --files`,
appends batches and the final unfinished tail once, and kills the producer on
choice/cancel. Grep uses the supplied generation on every delayed replacement
and cancels obsolete processes. Both open through `open-file-at`, preserve split
choices, and no longer need `run-commands`. Grep translates ripgrep byte columns
into character columns from the returned line, with no additional file read.
The optional kept `examples/api-demo` exercises the new SDK callbacks, events,
live updates and positioned opens. All example manifests now select 0.3;
existing 0.1/0.2 tests still use frozen components, with an additional fresh 0.2
SDK component for rollback tests. The guide documents version selection and
migration, and the historical host diff is labeled as a design reference.

Validation: 79 host tests, 37 editor library tests and 50 in-editor plugin
tests passed. The full documented integration run passed all 241 tests with
zero failures and eight movement tests excluded. Workspace formatting and
workspace/default-SDK/0.2-SDK Clippy passed without warnings. SDK native unit
tests passed in both configurations; 0.1 and 0.2 target checks passed. All guests
were rebuilt and frozen 0.2 digests stayed unchanged. After a normal build,
tmux checks passed for SDK popup closure, live generations, streamed output
before exit, finder file streaming/splits and Unicode grep positions/splits.
The first manual harness needed to wait for background registration and avoid
assuming a header on a single-column static picker; no product change was
needed for those harness corrections.

Quiet release benchmark (API 0.3 text-tools): cold loads for 1 / 5 / 20
plugins took 84.1 / 253.6 / 1018.8 ms; warm loads took 2.2 / 4.0 / 18.7 ms,
within the 5 ms per cached plugin budget. The unchanged frozen API 0.2
selection-event workload took 0.102 ms per dispatch. An initial run overlapping
the integration suite measured 104.6 / 593.4 / 1866.8 ms cold and
2.1 / 7.1 / 26.7 ms warm, with a 0.072 ms event; retain this as a noisy sample,
not a speed comparison. With SDK events subscribed and both example pickers
closed, three successive 1.2-second terminal samples showed zero scheduled
thread timeslices. These observations do not establish whole-editor parity
with vanilla Helix.

Performance design: no new timer, startup hook or per-keystroke subscription
was introduced by the SDK migration. Finder allocates per open picker/query;
stream routing and generation checks run only for active requests. The optional
trial plugin explicitly subscribes to its events; it is never installed by Pure.

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

## M11.6 Substitution preview cosmetics

Preview rows map back to source offsets through replacements. Generated
continuation rows leave the number column blank instead of claiming another
source line. CR, CRLF and LF split correctly with character-based highlight
offsets; optional Unicode endings follow the core feature. CR-only text also
splits in the default build, where the core still considers it one source
line. Preview work stops after the existing row cap.

Validation: 32 editor library tests and all 14 substitution integration tests
passed. A tmux check confirmed numbered source rows, unnumbered inserted rows,
and cancellation restoring the view. Formatting, workspace Clippy (no warnings)
and the normal editor build passed. The full documented suite passed: 226 tests, zero failures, eight movement
tests filtered out as documented.
The panel's separate truncation-label issue is recorded in the roadmap.

After cache cleanup, the release benchmark (rerun without concurrent builds)
measured 1 / 5 / 20 plugin cold loads at 95.4 / 257.3 / 1140.8 ms and warm
loads at 2.5 / 4.9 / 19.5 ms, within the per-plugin warm-cache budget. Event
dispatch took 0.069 ms. The benchmark passed.

## M10 Compatibility fixtures (groundwork)

Frozen the API 0.2 test plugin and text-tools example before any SDK changes.
Host and in-editor 0.2 tests now load the version-suffixed files; the build
script leaves them untouched. Their hashes and purpose are recorded beside
the fixtures. The text-tools test helper sets the manifest API explicitly,
so changing the live example later cannot silently change the compatibility
test's world. Validation: all 64 host tests and 35 in-editor plugin tests
passed against the frozen components; formatting and workspace Clippy passed
without warnings. API 0.3 host/editor/SDK functionality remains open.

## M10 Host world and positioned opens (in progress)

Added the 0.3 WIT world and boundary adapters while retaining the existing
host types and frozen older WIT. All three component worlds load side by
side. A direct-bindings 0.3 test guest is built by `build-plugins.sh`, so this
coverage does not depend on changing the SDK default prematurely.

`open-file-at` records a native action, checks `open-files`, opens the requested
split and sets a 1-based character position, clamping coordinates to the
document. It needs no `run-commands` permission. Tests cover host actions,
permission denial, all splits and Unicode columns; tmux confirmed a path with
spaces opening in a split at line 2, column 3 while preserving the first view.
Validation: 66 host tests, 32 editor library tests and 36 in-editor plugin tests
passed; formatting, warning-free workspace Clippy and a normal build passed.

All 0.3 host/editor abilities are implemented. SDK helpers and examples still
need their migration before the SDK default switches.
The SDK and examples remain on 0.2. M10.1/M10.2 stay open until the SDK/finder
migration; then finish guide and full-suite validation. No older WIT changed.

## M10.3 Popup lifecycle and centering

API 0.3 popups return owned request ids and close by id, Esc or the next
ordinary key. Dropping their contents delivers `on-popup-close` exactly once;
unknown, duplicate and foreign ids are ignored. Multiple popups can coexist,
and targeted closing preserves the others. Showing and closing in one call
retains action order. UI operations remain restricted to commands/callbacks.
The 0.2 fire-and-forget popup widget and ABI remain unchanged.

A small generic component-removal helper supports owned lifetimes; the popup
widget has an opt-in centering flag. No periodic work is added. Validation:
67 host tests, 32 editor library tests and 39 in-editor plugin tests passed;
targeted context tests also passed. Tmux confirmed centering before/after a
terminal resize and the Esc close callback. Fixtures rebuilt; formatting,
warning-free workspace Clippy and normal builds passed.

The SDK/finder/guide migration remains. A pre-existing SDK picker-registry rollback issue found
while reviewing callback ownership is recorded in the roadmap for SDK work.

## M10.4 Command, focus and idle events

API 0.3 subscribers receive post-command names, terminal focus gained/lost
and editor-idle notifications. Focus is captured before compositor dispatch,
so overlays cannot hide it. Idle uses the editor's existing one-shot timer
and dispatches directly; it creates no debounce timer or polling task.
Legacy guests cannot subscribe to these events.

Performance: command hooks check the atomic subscription mask before
allocating a name or locking the queue. Faulting plugins now refresh that
mask immediately, stopping unnecessary hook work for disabled subscribers.
The in-editor regression test verifies all four notifications, unsubscribing
on reload, and no second idle notification without input over 700 ms.
Tmux confirmed command names, real terminal focus notifications and idle
delivery. After input settled, sampling all nine editor threads' scheduler
counters for 1.2 seconds found zero additional scheduled timeslices. This is
a quiet-session observation, alongside the one-shot timer regression test.
Validation: 68 host tests, 32 editor library tests and 40 in-editor plugin
tests passed. Fixtures rebuilt; formatting, warning-free workspace Clippy,
the build without plugins, and a normal build passed.
Release benchmark after the event hooks (1 / 5 / 20 plugins): cold cache
56.5 / 230.9 / 955.6 ms; warm cache 1.8 / 3.9 / 15.9 ms; one event to one
plugin 0.075 ms. These are a quiet rerun of the same executable: the first
run immediately after compilation had a 633.9 ms single-plugin cold outlier
(warm 2.4 / 5.1 / 22.3 ms). Warm loads remain within 5 ms per plugin;
microbenchmarks do not establish a whole-editor comparison with vanilla Helix.

## M10.5 Streaming process output (host/editor)

API 0.3 `spawn` can stream complete stdout lines, including their newlines.
Batches keep their order before the final completion, whose stdout contains
only the unfinished line; stderr is returned in full up to its existing cap.
The stdout 16 MiB cap counts delivered batches and the tail together.
Callbacks keep the request open and each receives a fresh editor snapshot;
foreign, non-streaming, duplicate and late callbacks are ignored. Command
failure rolls back the new streaming request state. Frozen 0.2 guests retain
the original collected-output behavior.

Performance: reads make batches without a flush timer or one task per line.
A one-batch channel feeds the bounded editor queue, applying backpressure.
Post-exit grace bounds pipe reads, while already-read batches can finish
delivery to a slow consumer. Only new bytes are searched for line endings,
avoiding quadratic scans of a long unfinished line. Disabled plugins now
stop their running background processes as M1.2 intended; queued terminal
requests still have the separate revalidation TODO in the roadmap.

Real-component and editor tests cover callback order, editing between chunks,
completion tails, kill, spawn failure, frozen 0.2 behavior, and request cleanup.
Executor tests cover split UTF-8 bytes, cumulative output caps and delivery
under backpressure beyond the pipe grace period. SDK/finder use remains open
and the SDK still defaults to 0.2.
Validation: 69 host tests, 36 editor library tests and 42 in-editor plugin
tests passed; fixtures rebuilt, formatting, warning-free workspace Clippy
and a normal build passed. Tmux confirmed the first batch before process
exit, editing while it ran, updated snapshots, ordered batches and an
unduplicated final tail. An initial quiet-window sample showed five scheduled
timeslices; a per-thread rerun saw four across Tokio workers in the first
1.2-second window, then zero in each of the next two windows. This records
post-process settling activity separately from sustained idle polling;
the latest release microbenchmarks remain the M10.4 measurements above.

## M11.3 Live query generations (API 0.3 host/editor)

Each live picker starts at generation zero. Input advances it and clears
both items and the selectable matcher snapshot immediately, before debounce.
The host resets item accounting without calling the guest; only the debounced
query callback invokes wasm. Live `set-items`/`push-items` require the supplied
generation, and stale updates are dropped before accounting. Static pickers
use no generation. The UI checks again when applying queued batches.

Debounce tracks generation as well as text, so changing away from a query
and back still requests fresh results. Queued query jobs validate the original
picker instance and generation before restarting or invoking its callback.
Replacing items also clears the old selectable snapshot immediately. Request
failure, completion and traps clean up generation state; the older WIT and
frozen components remain unchanged.

Host tests cover stale queries/sets/pushes, a full current-generation item
cap, reset and cleanup, failed commands and static pickers. An editor library
test covers stale UI batches already queued. In-editor tests cover accepting
during the debounce gap, stale result batches and reverted query text.
Performance: only opted-in live pickers do generation bookkeeping, using
the existing debounce task; no polling or extra idle timer is added. SDK
generation tokens and finder migration remain part of the M10 SDK switch.
Validation: 71 host tests, 37 editor library tests and 45 in-editor plugin
tests passed; formatting, warning-free workspace Clippy and a normal build
passed. Fixtures rebuilt. Tmux confirmed initial/results replacement,
generation 3 after reverting text, cancellation on Enter before debounce,
and dropping a closed picker's query. With the live picker left open and
settled, all editor threads showed zero additional scheduled timeslices
over 1.2 seconds. Two adjacent findings (ignored picker titles and ordering
process start with picker installation) are recorded in Future TODOs.
Release measurements (1 / 5 / 20 plugins): cold 66.3 / 251.3 / 952.5 ms,
warm 2.7 / 6.6 / 28.9 ms, event dispatch 0.083 ms. A quiet rerun measured
cold 77.9 / 273.0 / 994.8 ms, warm 2.8 / 6.9 / 21.9 ms and dispatch
0.158 ms. Both runs meet the 5 ms-per-cached-plugin budget; dispatch timings
vary materially, so these do not establish a performance improvement.

## Queued process ordering and revalidation

Process starts and kills now share the call's ordered compositor effects.
This installs a picker before starting a program that feeds it, while
preserving same-call spawn/kill order. Deferred starts check that their owner
is enabled and the request is outstanding. Interactive requests check at
installation and again before the input reader or terminal is released;
discarding a stale request drops its chooser guard.

Host checks cover request identity, kind and completion. Editor tests exercise
spawned and interactive requests followed by reload or disable in the same
call, verify that their scripts never run, and confirm editing still works.
A spawn/kill test covers both effects queued in one call. Performance: checks
run only for requested program work; no startup, typing or idle task is added.
Validation: 71 host tests, 37 editor library tests and 47 in-editor plugin
tests passed; fixtures rebuilt. Formatting, warning-free workspace Clippy,
the build without plugins and a normal build passed. Tmux confirmed that
cancelled spawned/interactive scripts never ran and editing/saving continued.
The existing chooser check still ran its program, received its selection,
removed the chooser and restored the editor.

## SDK picker rollback

Failed commands now restore pre-existing picker data as well as dropping new
request, query and item registries. Pushes record the original length;
replacements move the original list into one undo entry per picker. A mixed
push/replace sequence preserves the original prefix. Successful commands
release the journal, and callbacks retain their existing semantics because
the host applies their actions even when a Rust closure reports an error.
This avoids cloning large item lists or accumulating one journal entry for
every batch. No startup or idle work is added.

Fresh 0.2 components test failed push, replacement and mixed updates, and
discarding a failed live picker's handler. The editor regression selects a
committed pushed row after a failed push and checks its data. Frozen 0.2
fixtures stay immutable. This test exposed an integration-only picker bug:
Rayon redraw callbacks lacked the owning Tokio runtime context and could
abort the harness. Integration callbacks now capture that context; normal
builds retain the original function callback. An adjacent queued-replacement
selection race to investigate is recorded in Future TODOs.
Validation after the harness fix: 73 host tests, 37 editor library tests and
48 in-editor plugin tests passed. Formatting, warning-free workspace Clippy
and SDK Clippy for wasm32-wasip2, and a normal build passed. Rebuilt all test
and example components; frozen 0.2 digests are unchanged. Tmux with the fresh
SDK confirmed ordered replacement/push, selection data matching the displayed
last row, and editing/saving after closing the picker. The SDK remains on 0.2
until the next migration task.

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
- Examples: `examples/yazi` (chooser through `run-interactive`; later
  removed at the owner's request, replaced by `examples/oil`) and
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

## Follow-up: background load metadata

Background builds now return their directory/settings with their host result;
the editor installs both only after accepting the generation. A stale task
cannot replace a reload's grants or directory. The private stale-load test
checks both rejected and accepted metadata, alongside host/actions/errors.
This moves existing load-time work under the guard, removes two metadata clones
from the background path, and adds no startup wait, event hook or idle work.
Validation: all three private integration-backend tests passed, including the
extended stale-load test. Formatting and workspace Clippy passed without warnings.

## Follow-up: bounded chooser output

Chooser completion opens nonblocking on Unix and validates the opened file
is regular, so replacing the path with a FIFO cannot hang terminal restoration.
Reads keep at most 16 MiB plus one byte to detect actual truncation; completion
returns the prefix and a stderr notice only when bytes were discarded. Missing
files remain an empty cancellation result; read failures now reach the callback
instead of silently becoming empty output. The private chooser is still removed
by its existing guard on every path, and failed program launches do not read it.
Tests check exact-cap versus oversized files, lossy UTF-8, deleted files and a
replaced FIFO without a writer. The FIFO test can release a regressed blocking
open rather than leaking a test thread.

Performance: this work runs only after a requested interactive chooser exits.
It bounds memory and read work and introduces no timer, hook or idle wake-up.
Validation: 39 editor library tests passed; formatting, warning-free workspace
Clippy and a normal build passed. Tmux checks verified a valid chooser callback
and cleanup, then a program replacing its chooser with a FIFO: error delivery,
cleanup, terminal restoration and subsequent editing/saving all passed.

## Pure without plugins vs vanilla Helix (2026-10-20)

Owner's rule: with no plugins, Pure must be as fast as vanilla Helix.
Measured with `docs/bench/vs-helix.py`: both release builds driven through
a pty with identical input, alternating runs. Vanilla is `upstream/master`
at `ba40e54`, Pure's merge base. Same config dir (no plugins, Rust grammar
built, language server off), 120x40 terminal, 4 cores. File: a
7,500-line Rust file (`helix-term/src/commands.rs`).

| Median (min–max) | Vanilla Helix | Pure, no plugins |
| --- | --- | --- |
| Startup to first draw | 450 ms (432–482) | 447 ms (422–496) |
| CPU, start then quit (15 runs) | 376 ms (359–411) | 378 ms (350–424) |
| CPU, editing session (5 runs) | 22.21 s (22.10–22.40) | 22.16 s (22.09–22.98) |
| Idle over 5 s: wake-ups, CPU | 0, 0 ms | 0, 0 ms |
| Peak memory | 37.3 MiB | 38.5 MiB |
| Binary | 33 MB | 57 MB |

The editing session sends ~1,000 keys 3 ms apart: 400 `j`, a search with
60 `n`, typing a line in insert mode, 200 `w`, `x` x40, `s` to split, undo,
page up/down. All differences are within noise except memory (+1.2 MiB,
Wasmtime's code and statics) and binary size. Build with
`--no-default-features --features git` to drop both.

Rerun: build both with `cargo build --release`, then
`python3 docs/bench/vs-helix.py '{"helix": ["<hx>", "<runtime>"], "pure":
["<pure>", "<runtime>"]}' start|work|idle <rounds>`. It expects `work/big.rs`
and a `bench-cfg` config dir next to the script; see the paths at the top.

## render-markdown.nvim-style plugin (examples/markdown, 2026-10-20)

The owner asked whether a render-markdown.nvim-like plugin is possible.
Mostly yes, after adding one decoration kind, **overlays** (`set-overlays`:
characters drawn over the text, Helix's jump-label mechanism; mapped through
edits and dropped with their char).

| render-markdown feature | In Pure |
| --- | --- |
| Heading icons and colours per level | Yes (overlay `#` -> `◉◎○◆◇▸`, highlight) |
| Bullets by depth, checkboxes | Yes (`•◦▪▫`, `☐` / `✔`, checked dimmed) |
| Block quotes | Yes (`▋`) |
| Full-width rules | Yes (overlay + end-of-line virtual text) |
| Tables with box borders | Yes (`│`, `├──┼──┤`) |
| Code blocks with the language | Yes (fence rule, label, block highlight) |
| Raw text on the cursor line (anti-conceal) | Yes (`set-raw-on-cursor-line`: the editor follows the cursor, no events) |
| Toggle | Yes (`markdown/toggle`, `space M`) |
| Hiding `**`, backticks, link URLs (conceal) | **No** at the time; done later (see "Conceal" below) |
| Heading backgrounds across the whole window | Partly: highlight covers the text only |

Gaps that would need editor work: conceal (a new kind of text annotation in
`helix-core`'s document formatter) and full-line backgrounds. Both are
possible but larger upstream diffs; logged as TODOs.

## Overhead with plugins loaded (2026-10-20)

Same harness and builds as above; the third column loads five plugins: yazi
(file manager), finder, todos, markdown and text-tools.

| Median (min–max) | Vanilla | Pure, none | Pure, 5 plugins |
| --- | --- | --- | --- |
| big.rs: startup | 461 ms | 456 ms | 443 ms |
| big.rs: CPU, start then quit | 384 ms | 385 ms | 408 ms |
| big.rs: CPU, editing session | 22.15 s | 22.05 s | 22.49 s |
| big.md (6,732 lines): startup | 131 ms | 131 ms | 131 ms |
| big.md: CPU, start then quit | 57 ms | 58 ms | 84 ms |
| big.md: CPU, editing session | 726 ms | 722 ms | 1115 ms |
| Idle over 5 s (both files) | 0 wake-ups | 0 wake-ups | 0 wake-ups |
| Peak memory, big.md | 25.2 MiB | 29.3 MiB | 44.1 MiB |

- Startup to first draw is unchanged: plugins load after the first frame.
- What the plugins cost is what they asked for. On big.rs that is the todos
  scan per edit (+2%). On big.md it is markdown re-rendering 6,732 lines on
  each of ~260 edits (~1.5 ms per edit, under one frame).
- Markdown first cost 1,846 ms there: it re-rendered on every cursor move
  (15.7 ms each). Fixed by a faster renderer (no per-char allocations) and
  by `set-raw-on-cursor-line`, which moves "show the cursor's line raw" into
  the editor, so cursor moves cost the plugin nothing.
- Memory: ~3 MiB per loaded plugin (its Wasm instance and compiled code).
- TODO: incremental rendering for markdown (re-render only changed lines)
  would cut the per-edit cost on very large files.

## M12: what kinds of plugins can be built (2026-10-20)

The owner asked which kinds of plugins Pure couldn't support yet. Compared
with Neovim's ecosystem, these needed new API, now in API 0.3:

| Kind (Neovim example) | Needed | Pure | Example |
| --- | --- | --- | --- |
| Linters (nvim-lint) | diagnostics | `diagnostics.set-diagnostics` | `lint` |
| Completion sources (nvim-cmp sources) | completion provider | `on-completion` + `complete` | `emoji` |
| File managers (oil.nvim) | editable plugin buffers, file operations | `open-buffer`, `on-buffer-write`, `[keys.buffer]`, `files.*`, `allow-dirs` | `oil` |
| Timers, clocks, auto-save (pomodoro) | timers | `timers.set-timer` | `pomodoro` |
| Yank history (yanky.nvim) | registers | `register`, `set-register` | `yank-history` |
| Output/REPL buffers | scratch buffers | `open-scratch` | – |

Already possible before: pickers and finders (telescope), git and other
tools through programs (`spawn`, `run-interactive`), popups and prompts,
decorations (todo-comments, render-markdown), statusline segments, key
bindings and commands, background work. Later work added the sidebar and
conceal; see the current capability audit in
[`plugin-future-roadmap.md`](plugin-future-roadmap.md) for the remaining limits.

Design notes:

- Plugin diagnostics reuse the LSP diagnostic path with
  `DiagnosticProvider::Plugin` and UTF-32 positions, so every place that
  shows diagnostics works without per-feature code.
- Completion requests are answered asynchronously: the editor registers a
  oneshot per request, the plugin's `complete` action fills it, and the
  menu task waits up to 1 s. A new request replaces an unanswered one.
- File operations go through `Editor::move_path` and friends, which tell
  language servers (`willRename`, ...) and retarget open buffers; for a
  renamed directory, buffers of files inside it follow too. The host
  checks each path's real parent against the real roots, so a symlink in
  the workspace can't lead the editor outside it.
- Plugin buffers are excluded from Helix's "drop the empty scratch buffer
  when switching away" rule, so `ctrl-o` can return to them.
- Cost: none unless a plugin uses them. Completion checks one atomic bit
  per request; registers are copied into snapshots only while a plugin
  with the `registers` permission is loaded; timers are sleeping tasks.
- Tests: host tests per interface (`api_03_*`) and per example
  (`*_example_*`), editor tests for diagnostics, timers, plugin buffers,
  buffer keys, completion and the oil workflow
  (`oil_example_edits_directories_as_buffers`).


## Overhead with 10 plugins vs vanilla (2026-10-20, after M12, API 0.4, sidebar)

Same harness (`docs/bench/vs-helix.py`), release builds, alternating runs.
"10 plugins": oil, tree, markdown, todos, lint, emoji, finder, text-tools,
yank-history and pomodoro, with their grants.

| Median | Vanilla | Pure, none | Pure, 10 plugins |
| --- | --- | --- | --- |
| big.rs: startup | 435 ms | 444 ms | 451 ms |
| big.rs: CPU, start then quit | 362 ms | 377 ms | 421 ms |
| big.rs: CPU, editing session | 24.0 s | 26.7 s* | 23.9 s |
| big.md: startup | 133 ms | 132 ms | 133 ms |
| big.md: CPU, start then quit | 60 ms | 57 ms | 114 ms |
| big.md: CPU, editing session | 760 ms | 784 ms | 1519 ms |
| Idle 5 s, both files | 0 wake-ups | 0 wake-ups | 0 wake-ups |
| Peak memory, big.rs | 36.2 MiB | 41.4 MiB | 63.8 MiB |

\* The big.rs editing runs varied ±10 % on this machine (vanilla 22.0–24.9 s,
Pure 23.7–28.9 s, Pure with plugins 23.0–25.2 s): within noise.

- Startup to the first frame and idle are unchanged with any number of
  plugins: plugins load after the first frame and nothing runs while idle.
- With 10 plugins, starting costs ~50 ms more CPU (instantiating 10
  components, after the first frame). The first run after installing them
  also compiles them once (up to ~2.5 s of background CPU, then cached).
- Editing the 6,700-line Markdown file costs what the plugins asked for:
  markdown re-renders the changed part, lint re-checks the document, todos
  re-scans, emoji is asked for completions while typing. Together about
  2.8 ms per edit, under one frame.
- Memory without plugins: the heap is the same as before this work (19.6
  MiB anonymous memory for both the previous and the current build); the
  extra RSS is file-backed code pages of the larger binary (+3 MiB since
  the previous build, shared and reclaimable, not allocations). Building
  with `--no-default-features --features git` drops Wasmtime entirely.
  With 10 plugins: +22 MiB (about 2 MiB per plugin instance).

## Conceal (2026-10-20)

The owner's decision on the open questions: "whatever render-markdown.nvim
does". So:

| Question | render-markdown.nvim | Pure |
| --- | --- | --- |
| Raw text: cursor line or cursor only? | the whole cursor line (`anti_conceal`) | the whole primary cursor line (`set-raw-on-cursor-line`) |
| Lines with other selections raw too? | no (one cursor) | no: only the primary's line |
| Insert mode? | not rendered (`render_modes = n, c, t`) | not rendered in insert or select (`set-render-modes [normal]`, per layer) |
| Mouse click on hidden text? | hidden text has no cell | the same; a click on a replacement lands at the range's start |

- `helix-core`: `text_annotations::Conceal` and `add_conceals`; the
  formatter's `advance_grapheme` swallows a concealed range (never a line
  break) into one grapheme drawn as the replacement (or nothing: width 0),
  and moves inline annotations and overlays past it. Position mapping
  (cursor, scrolling, mouse, soft wrap) goes through the formatter, so it
  follows. Tests: rendering, positions, annotations inside hidden text, soft
  wrap, `pos -> screen -> pos` round trips.
- `helix-view`: a `conceals` layer in `PluginDecorations` (mapped through
  edits; hidden text that is edited shows again), per-layer render modes,
  and the mode the document was last drawn in (set by the renderer, so
  motions between frames match the screen; per document, so parallel
  editors in tests don't share it).
- API: `decorations.set-conceals` and `set-render-modes` in 0.3 and 0.4
  (imports only: frozen components still link). The host widens ranges to
  whole graphemes, cuts them at the line's end, keeps one grapheme of the
  replacement and drops overlaps.
- `examples/markdown` hides `**`/`__`, `*`/`_`, `~~`, backticks and link
  URLs (`[text](url)` shows `text`, `![alt](url)` shows `▣alt`), renders
  only in normal mode, and keeps its incremental rendering (the property
  test covers the new layers).
- Checked in a terminal (pty + pyte): the cursor line is raw, the others
  concealed; `i` shows everything raw; `esc` renders again.
- No cost without plugins: an empty `PluginDecorations` is skipped as
  before; the formatter checks an empty conceal list per document grapheme
  (a loop over zero layers).

## System review (2026-10-20, master `1268b872`)

Pulled the latest master and reviewed plugin execution/SDK/ABI compatibility,
startup and reload, file/process jobs, editor I/O, decorations/conceal,
Markdown/oil/tree, substitution preview, compilation/quickfix and CI/performance
instrumentation. Findings and next steps are in
[`suggested-improvements.md`](suggested-improvements.md): **22 prioritized
items**, each distinguishing reproduced behavior, code-supported risks and
proposed capabilities. No runtime fixes or guest artifacts changed in this
review.

Bounded scratch probes reproduced the shell-expansion process-grant bypass,
auto-reload FIFO UI stall, loss of worker interruption after host drop, and
local installer writes outside staging before path rejection. Host/native probes
also confirmed cancelled timer scheduling actions and preview output allocation
before display truncation. The report identifies memory/job limits, filesystem
race boundaries, lifecycle cleanup and large-file/many-plugin scaling work.

Validation: 95 host tests, 47 editor library tests and 13 formatter tests passed;
workspace formatting and Clippy passed without warnings. The default plugin
integration run had 67 passes and one idle-event assertion failure; that test
passed alone, and all 68 plugin tests passed with four test threads. Two
auto-reload integration tests, three Markdown native tests and three oil native
tests passed. A normal editor build was used for the scratch terminal probes.
The full documented integration/workspace suites and release benchmarks were
not rerun.

Performance observations are deliberately limited: an idle normal debug editor
with no plugins recorded 13 thread context switches in each 3.3-second sample
with auto-reload enabled and disabled; source inspection confirms the recurring
timer exists in both cases. This is neither an exact wake-up count nor a release
benchmark against vanilla Helix. The report proposes regression gates and
measurements rather than extending historical parity claims to this revision.

## Future plugin capabilities review (2026-10-20)

Rechecked master `1268b872`, the 0.4 WIT, SDK/host/editor adapters and native
syntax, completion/snippet, view, LSP, DAP, VCS, save and process boundaries.
[`plugin-future-roadmap.md`](plugin-future-roadmap.md) records which plugin
behaviors remain blocked or limited, the candidate APIs, delivery waves and
performance/compatibility gates. It separates existing tool/text-buffer
workarounds from native integration, and core designs from ABI additions.

The proposed first delivery hardens relevant current findings, defines request
and version contracts, adds revision-safe document work and reuses the native
snippet engine. Later steps cover views, syntax/semantic services, file watches,
persistent processes, richer panels, debugger/diff integration, folds and URI
providers. New exports/record shapes would use a proposed next ABI with frozen
0.4 fixtures and legacy support; no new APIs were implemented in this review.
No benchmarks or guest artifacts changed. Source links and roadmap references
were checked; implementation-specific tests and measurements remain explicit
acceptance requirements, not completed validation.
Workspace formatting and Clippy passed without warnings; runtime test suites
were not repeated for this documentation-only follow-up.

## Performance pass after panels and incremental highlighting (2026-10-21)

Same harness (`docs/bench/vs-helix.py`), big.rs (257 KB, Rust), release
builds of Helix 25.07.1, Pure before the merge of the incremental-highlighting
work (`6734495`) and Pure after it (`02034a3`), all built with plain
`cargo build --release`, alternating runs.

| Median | Helix | Pure before | Pure after |
| --- | --- | --- | --- |
| Startup to first frame (15 runs) | 472.9 ms | 471.6 ms | 468.0 ms |
| CPU, start then quit (15 runs) | 401.0 ms | 405.1 ms | 394.4 ms |
| CPU, editing session (3 runs) | 25.2 s | 24.2 s | **14.3 s** |
| Peak memory | 37.9 MiB | 42.8 MiB | 41.8 MiB |

- **Editing is 43 % cheaper than Helix.** Profiling the editing session
  showed ~47 % of typing time in the injection and locals queries, which
  re-ran over the whole file after each keystroke. The vendored tree-house
  (`vendor/README.md`) re-runs them only over the changed regions.
- **Startup is unchanged.** An earlier measurement showed the merge ~26 ms
  slower; that came from comparing a stripped build with line tables
  against a plain one, not from the code. Timing `Syntax::new` on big.rs
  against upstream and vendored tree-house gives the same result (70–74 ms,
  plus ~350 ms to compile the queries once on first use). Callgrind can't
  compare startups here: parse and query steps have time budgets, so
  valgrind's slowdown changes how much work runs.
- **Where startup goes:** about three quarters of the start-then-quit CPU is
  compiling the Rust highlight/injection/locals queries
  (`ts_query__perform_analysis`), the same in Helix. Tree-sitter can't
  cache a compiled query, so this stays until upstream can.
- **Idle:** with a file open, the `auto-reload` check wakes the main thread
  once a second (one `stat` per open file; 0 ms CPU measured over 5 s).
  With no file open, or with `auto-reload = false`, there are no wake-ups
  (S02). Since 2026-10-29 the directories of open files are watched
  instead (`helix-term/src/auto_reload.rs`): 0 wake-ups per 5 s with a
  file open (was 21), startup unchanged; the timer stays as a fallback
  where watching fails.
- **Panels and the new examples** cost nothing when unused: no panel means
  `layout` returns the editor area unchanged, and plugin key tries only
  cover the plugins that are loaded.

## Speed after plugin API 0.5 (2026-10-26)

Same harness (`docs/bench/vs-helix.py`), big.rs (257 KB, Rust), release
builds, alternating runs, 4 cores. "Before" is `6324afa` (the last commit
before 0.5), "now" the finished 0.5. Each editor gets only its own built
Rust grammar (no injection grammars), so all highlight the same way.

| Median | Helix 25.07.1 | Helix at Pure's merge base (`ba40e54`) | Pure before 0.5 | Pure now |
| --- | --- | --- | --- | --- |
| Startup to first frame (15 runs) | 254 ms | 330 ms | 330 ms | 314–320 ms |
| CPU, start then quit | 268 ms | 357 ms | 353 ms | 331–334 ms |
| CPU, editing session (3–5 runs) | 20.8 s | 20.2 s | 12.2 s | 11.8–12.4 s |
| Idle over 5 s: CPU | 0 ms | – | 0 ms | 0 ms |
| Peak memory | 36 MiB | 39 MiB | 41 MiB | 41–43 MiB |

With plugins (separate cache directories per configuration):

| Median | Before, five API 0.3 plugins | Now, the same plugins | Now, seven API 0.5 plugins |
| --- | --- | --- | --- |
| CPU, start then quit (10 runs) | 369 ms | 368 ms | 377 ms |
| CPU, editing session (4 runs) | 11.78 s | 11.63 s | 11.85 s |
| Idle over 5 s: wake-ups, CPU | 22, 0 ms | 22, 0 ms | 22, 0 ms |
| Peak memory | 51 MiB | 52 MiB | 61 MiB |

- **0.5 made nothing slower**: with no plugins, and with the same plugins,
  before and now are within noise. Seven 0.5 plugins (outline, illuminate,
  git-blame, harpoon, markdown, text-tools, todos) add 9 MiB and no
  measurable editing CPU.
- **Fixed while measuring** (they cost nothing in this benchmark, but grow
  with use): the completion menu built up to 1,000 item labels for plugins
  on every keystroke and copied them to read the count, even with no
  plugins; the per-frame UI bookkeeping for `state.ui` ran without 0.5
  plugins; and a `pre-command` hook on one command made a snapshot for every
  command.
- **Startup vs Helix 25.07.1** (+60–75 ms): the same as vanilla Helix at
  Pure's merge base, so it comes from upstream changes since 25.07.1 (larger
  Rust queries to compile), not from Pure.
- **Three measurement traps**, now handled: vanilla Helix asks the terminal
  for its attributes and waited 2 s in the pty (the harness answers now);
  Pure's newer Rust grammar breaks 25.07.1's queries (each editor gets its
  own config dir with its own grammar); and two plugin sets with plugins of
  the same name sharing one cache directory recompile each other's plugins
  on alternate runs (the cache keeps one build per plugin name), which first
  looked like a 2.5x startup regression.
