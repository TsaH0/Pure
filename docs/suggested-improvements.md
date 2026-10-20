# Suggested improvements: Pure system review

Reviewed on **2026-10-20**, against **master `1268b872`** (after pulling PR #29).
This was a review and implementation backlog; see "Status" for what has
since been fixed.

## Status (2026-10-20, after the fixes)

Follow-up verification of working-branch commit `4bd8f2e9` is in
[`plugin-fix-verification.md`](plugin-fix-verification.md). The fixes are not
all complete: oversized single preview expansions, cancelled background work
and combining-text grep coordinates still have reproduced gaps. That report
also distinguishes branch fixes from master and records the tests run.

Each finding was checked against the code first; all were valid. What was
done, and what is left on purpose:

| ID | Done | Left |
| --- | --- | --- |
| S01 | Commands containing `%sh` need `allow-process` (test) | — |
| S02 | No wake-ups without files or with auto-reload off (measured 0 context switches / 5 s); FIFOs and devices skipped (test) | Reading a changed file is still synchronous (as `:reload`); file watches belong to F09 |
| S03 | Ticker runs until running calls end; queued tasks refused after host drop (test fails without the fix) | — |
| S04 | `Manifest::parse` rejects non-local `wasm` before any copy (test) | — |
| S05 | `create_new`, `RENAME_NOREPLACE`, `O_NOFOLLOW`; grants re-checked when each operation runs (tests) | Full handle-relative (`openat`) operations; the re-check narrows, not closes, the parent-swap window |
| S06 | Cancel removes same-call timers or aborts the task; reload aborts all (tests) | — |
| S07 | At most 2 linear memories per store; log lines capped at 4 KiB | Aggregate byte budgets for retained layers, picker/sidebar text |
| S08 | Preview builds at most 1 MiB of replacements (test) | — |
| S09 | Install/update commit and reload run in the background (checked in a terminal) | LSP `will*` waits and `%sh` expansion are upstream behaviour, unchanged |
| S10 | ≤ 4 batches per plugin, batches serialized, FIFOs refused (tests) | Cancelling a batch mid-way |
| S11 | Disabled plugins lose processes, timers, completions, decorations, diagnostics, statusline, sidebar (test) | — |
| S12 | Markdown forgets closed documents, sets render settings once | Delta reads need F02 |
| S13 | One `update_positions` pass per layer; randomized equivalence test | — |
| S14 | Per-plugin call timing in `:plugin-list`; 50 ms budget per event, the rest after pending input | p95/p99 telemetry |
| S15 | Compiles ≤ available parallelism (max 8); no host when all plugins are off | Cache pruning on warm hits kept: it removes other versions' entries and costs microseconds |
| S16 | A reload during the startup load opens the waiting directory (test fails without the fix) | Focus checks for deferred opens (F03) |
| S17 | Old copy restored if the update or the lock write fails (test fails without the fix); unreadable lock loads nothing; git has no prompts and a 2-minute limit | Inter-process lock serialization |
| S18 | `:grep` capped at 32 MiB / 100,000 entries, newest wins, byte columns (test) | `:compile` buffer is unbounded, like Emacs' |
| S19 | Oil orders operations by dependency, swaps through a temporary name, makes new directories once (tests) | — |
| S20 | Conceals drop on any edit inside them (test) | — |
| S21 | CI job: plugin-less build, native tests of examples, committed components | Release benchmark gates |
| S22 | Moved to the future roadmap's F01 | — |

## What was reviewed

The review followed the fork's execution paths through the plugin host and SDK,
API 0.1–0.4 adapters, startup/reload, events and completions, timers and workers,
processes, file operations and installation, decorations and the document
formatter, Markdown/oil/tree examples, command-line UI, automatic file reload,
substitution previews, compilation, quickfix, and CI/benchmark configuration.
LSP and editor internals were inspected where these paths call them; this is
not a line-by-line audit of every upstream Helix/DAP/LSP module.

The previous feature list is largely complete: directory handlers, trash,
full-line backgrounds, incremental Markdown rendering, conceal, and the sidebar
exist. They should not be listed again as missing features. The important next
work is lifecycle correctness, resource bounds, and scaling those implementations.
Preserve optional plugins, existing user bindings, old-component compatibility,
and the existing empty/unsubscribed fast paths while making these changes.

Evidence labels:

- **Reproduced:** exercised against this revision with an existing test or a
  bounded scratch-file probe.
- **Code-supported:** the relevant call path is present in the source, but a
  dedicated end-to-end regression test was not run for that finding.
- **Proposal:** a useful extension or measurement, not a demonstrated defect.

Priority: **P1** should be addressed before relying on the affected guarantee;
**P2** is important correctness/scaling work; **P3** is follow-on API/product work.

## Prioritized findings

| ID | Priority | Finding | Evidence |
| --- | --- | --- | --- |
| S01 | P1 | Shell expansions bypass the plugin process grant | Reproduced |
| S02 | P1 | Auto-reload wakes while disabled and can block the editor on special files | Reproduced |
| S03 | P1 | Dropping a host can remove the deadline mechanism from a running CPU worker | Reproduced |
| S04 | P1 | Local installation copies an invalid component path before rejecting it | Reproduced |
| S05 | P1 | File grants/no-overwrite checks are separated from the filesystem operation | Code-supported |
| S06 | P1 | Cancellation leaves timer tasks scheduled | Reproduced host actions; scheduler path inspected |
| S07 | P1 | Resource limits do not bound total plugin/host memory | Code-supported |
| S08 | P1 | Preview row/input limits do not bound substitution output allocations | Reproduced allocation count |
| S09 | P1 | Native filesystem/LSP work still runs synchronously on the UI path | Code-supported |
| S10 | P2 | File-operation jobs need cancellation, admission limits, and serialization | Code-supported |
| S11 | P2 | Trap/disable cleanup is weaker than reload cleanup | Code-supported |
| S12 | P2 | Markdown retains closed documents and still does whole-document work | Code-supported |
| S13 | P2 | Decoration mapping can multiply edit count by decoration count | Code-supported |
| S14 | P2 | Event/completion fan-out has no aggregate UI budget | Code-supported |
| S15 | P2 | Plugin loading has unbounded compilation concurrency and repeated cache scans | Code-supported |
| S16 | P2 | Directory loading state and deferred requests need a complete lifecycle | Code-supported |
| S17 | P2 | Plugin update/lock handling needs transactional recovery and fail-closed verification | Code-supported |
| S18 | P2 | Compilation, grep, and quickfix lack consistent bounds/cancellation/coordinates | Code-supported |
| S19 | P2 | Oil's operation planner needs dependencies, deduplication, and rename-cycle handling | Code-supported |
| S20 | P2 | Conceal invalidation misses edits that preserve range length | Code-supported |
| S21 | P2 | CI and benchmarks need lifecycle/performance regression coverage | Observed test outcomes; proposal |
| S22 | P3 | New API capabilities need clearer negotiation and completion contracts | Proposal |

### S01 — Enforce process grants where expansion/execution actually happens

Sources: [host command gate](../helix-plugin/src/lib.rs) (`State::run_command`),
[command classifier](../helix-term/src/plugins.rs) (`starts_program`),
[typed-command argument expansion](../helix-term/src/commands/typed.rs)
(`execute_command`), and [shell expansion](../helix-view/src/expansion.rs)
(`expand_shell`).

The classifier checks the command name against a list. `:echo` is not on that
list, but its arguments can contain `%sh{...}`, which starts a shell while
arguments are expanded. In a scratch configuration, a plugin with only
`run-commands` and no process grant was refused for a direct `:sh`, but its
`:echo %sh{printf bypass > <scratch-marker>}` created the marker. The outer
`:plugin-run` argument was single-quoted so the expansion happened inside the
plugin-issued command, not while entering the user command.

**Improve:** carry the initiating plugin's execution/grant context through typed
commands, expansions, nested commands, and deferred execution. Check the grant
at actual shell/process entry points. Keep ordinary editor commands usable and
retain the owner's single `allow-process` gate without per-program prompts.
**Verify:** direct shell commands, `%sh`, nested/aliased commands, deferred work,
None/list/Any grants, and ordinary navigation; denied paths must create no process.

### S02 — Replace unconditional polling and make auto-reload nonblocking

Sources: [application timer and select loop](../helix-term/src/application.rs)
(`auto_reload_timer`), [reload scan](../helix-view/src/editor.rs)
(`reload_changed_documents`), and [document reload](../helix-view/src/document.rs)
(`Document::reload`).

A one-second interval is created and polled unconditionally. Setting
`auto-reload = false` only makes the scan return early; it does not stop the
wake-ups. A normal debug binary, with no plugins, recorded **13 total thread
context switches over 3.3 seconds** in each of the enabled and disabled cases.
That is a context-switch observation, not an exact timer-expiration count or a
release CPU benchmark. The interval also exists for scratch-only sessions.

When enabled, the UI synchronously stats every file-backed document, opens
changed files, reads them, and calculates a diff. Replacing a scratch watched
file with a FIFO stopped the editor from processing an `:echo` command until a
writer opened/closed that FIFO. This is distinct from the known upstream WASI
worker issue: this path can block the editor itself.

**Improve:** use shared, event-driven file watches; register them only for enabled
file-backed documents. Perform safe regular-file reads and expensive diff work
outside the UI loop. Recheck document revision/modified state before applying.
Track file identity plus stable read metadata, not only path mtime, so atomic
replacement, coarse timestamps, and changes during a read do not get missed.
**Verify:** disabled/empty sessions have zero recurring wake-ups; test FIFO/device
replacement, slow filesystems, atomic saves, same-mtime replacements, unsaved
edits, deletion/recreation, hidden buffers, and config toggling.

### S03 — Keep worker interruption alive through host drop/reload

Source: [worker and ticker lifetime](../helix-plugin/src/lib.rs)
(`spawn_worker`, `Ticker`, `PluginHost::drop`).

Workers retain the engine/component/ticker and can still be executing when the
host is dropped. Host drop immediately sets `ticker.stop = true`. A CPU-bound
Wasmtime call needs epoch advancement to yield; a Tokio timeout around that
future alone cannot interrupt a future that never yields.

A probe using the existing `testplugin3` background `spin` task and a 300 ms
background limit returned a timeout with the host alive. Dropping the host
100 ms after starting the task produced **no completion within another 1500 ms**.
Each probe ran in a separate bounded process, so it left no spinning worker in
the review session. The code path explains why the guest can continue past its
configured deadline after host drop.

**Improve:** give active workers an interruption/cancellation lifetime independent
of the host's foreground handle. Cancel queued work on reload, interrupt active
work, and stop the epoch thread only once active calls are finished. Avoid
blocking UI shutdown while waiting for workers.
**Verify:** host drop, reload, clear, and disable during CPU spin and suspended
WASI calls; all workers must finish or reach a bounded, explicitly tracked state.

### S04 — Validate installation paths before copying any local file

Source: [installer staging](../helix-term/src/plugins/install.rs) (`stage`).

The local-directory branch reads `manifest.wasm`, copies it into staging, and
only afterwards checks that it is a single relative filename. A scratch plugin
with `wasm = "../../victim.wasm"` was rejected, but an existing scratch file
outside its staging directory had already been overwritten by that copy. The
probe touched only its own temporary source/install trees.

**Improve:** centralize manifest component-path validation and run it before any
join/copy/hash/open, in both installer and loader paths. Confine destination
operations to the staging directory and define a policy for component symlinks.
**Verify:** absolute paths, parent traversal, nested paths, symlinks, and malformed
manifests leave all files outside staging unchanged, including on failure before
installation confirmation.

### S05 — Make grant checks and no-clobber operations one safe operation

Sources: [path authorization](../helix-plugin/src/lib.rs) (`State::writable`,
`apply_files`), [file executor](../helix-term/src/plugins.rs) (`file_ops`), and
[editor filesystem helpers](../helix-view/src/editor.rs).

Authorization canonicalizes an existing ancestor during the guest call, but
execution happens later using ordinary absolute paths. A parent can be replaced
by a symlink between those steps. Similarly, checking `symlink_metadata(to)`
before ordinary `rename`/`copy`/`write` is not an atomic no-overwrite guarantee;
other batches or processes can create the destination between check and use.

Legacy API 0.3 `create_path` also goes directly to `Editor::create_path`, whose
file branch uses `fs::write(path, [])`. Unlike the API 0.4 create path, it does
not first reject an existing final symlink, so the final target can be followed
and truncated even though `writable` checked only the parent.

**Improve:** preserve directory capabilities or use handle-relative operations
with explicit symlink/no-follow policies and atomic create/no-replace behavior.
Keep final symlinks as entries when deleting/trashing/copying links. Route legacy
APIs through the same hardened implementation without changing their ABI.
**Verify:** deterministic parent-swap and concurrent-destination tests; legacy
create through a final symlink must not change a target outside granted roots.

### S06 — Cancel the timer task, not only its pending request

Sources: [timer bookkeeping](../helix-plugin/src/lib.rs) (`set_timer`,
`cancel_timer`) and [timer scheduling](../helix-term/src/plugins.rs)
(`Action::SetTimer`).

Cancellation removes the pending id but does not abort the sleeping Tokio task
or remove an already-recorded scheduling action. The existing guest's
`timer-cancel` command returned a `SetTimer` action with an already-cancelled id
in **32 out of 32 calls**. The executor starts a sleep for every such action.
Cancelled long timers can retain tasks/strings until expiry; removing their
pending ids lets repeated cancellation bypass the nominal 16-timer task bound.
Reload/disable also leaves those sleepers to expire and enqueue ignored results.

**Improve:** own timer handles/cancellation tokens by plugin and request id;
validate before launch, abort on cancellation/reload/disable, and remove completed
handles. Coalesce timers in an on-demand deadline queue if scale warrants it.
**Verify:** cancel before scheduling and after launch, repeated long-timer churn,
reload/disable, bounded live task count, and no wakes from cancelled timers.

### S07 — Budget aggregate guest memory and host-retained data

Sources: [store limits and host requests](../helix-plugin/src/lib.rs),
[SDK registries](../helix-plugin/sdk/src/requests.rs), and
[decoration storage](../helix-view/src/plugin_decorations.rs).

`StoreLimitsBuilder::memory_size` is a **per-linear-memory** limit; Wasmtime's
own documentation explicitly says so. Stores permit up to 16 memories, and a
background instance has another store. The configured 256 MiB value is therefore
not a total resident-memory ceiling for one plugin. This is about the configured
bound, not a claim that current examples allocate that amount.

The store limiter also does not cover host `Vec`/`String` allocations. Limits are
mostly per request/layer/item count: there is no aggregate retained-byte/layer
budget, the log bounds line count rather than bytes, and repeated actions can
accumulate before the call returns. Large strings and many valid layers can
outlive the guest call in editor-owned state.

**Improve:** enforce cumulative memory accounting per instance/plugin and a global
host budget. Bound bytes as well as counts for action batches, retained layers,
logs, picker/sidebar text and outstanding request payloads. Keep ABI conversion
allocations in the threat/resource model. Reject or evict predictably.
**Verify:** many memories/layers, huge labels/log lines, repeated updates in one
call, and multiple background/foreground instances, with measured host RSS.

### S08 — Bound preview computation before materializing substitutions

Sources: [live preview](../helix-term/src/commands/substitute_preview.rs)
(`compute`) and [replacement computation](../helix-core/src/substitute.rs)
(`Substitution::apply`).

The 200-row display cap is applied after `apply` has created every change and
replacement. The 8 Mi-character scope limit bounds input characters, not output
bytes, capture expansion or total matches. A bounded native probe with 100,000
input characters and a 256-character replacement produced 100,000 changes and
**25,600,000 replacement payload bytes**, before vector/string overhead. The
preview calls this same eager routine before truncating its panel.

**Improve:** add a preview-specific streaming/budgeted match path with limits on
matches, generated bytes and elapsed work. Stop computing unseen replacement
payloads once the budget is spent, show that the preview is partial, and preserve
final substitution semantics. Avoid large synchronous recomputation per typed key.
**Verify:** dense/zero-width matches, long capture expansion, large Unicode input,
and repeated replacement typing stay within explicit latency/memory bounds.

### S09 — Finish moving native I/O and waits off the UI thread

Sources: [file executor](../helix-term/src/plugins.rs) (`file_ops::run_one`),
[editor create/move/delete](../helix-view/src/editor.rs),
[shell expansion](../helix-view/src/expansion.rs), and
[installer confirmation](../helix-term/src/plugins/install.rs).

API 0.4 copy/trash is moved to blocking tasks, but create/rename/delete goes back
to the editor thread. Recursive deletion, slow metadata/rename, and blocking LSP
`will_*` requests can still stall input. Installer confirmation synchronously
commits files and calls `load`, which rebuilds/reinitializes the host on the UI
path. Shell expansion waits synchronously with unbounded captured output. Even
some host imports do synchronous path metadata/canonicalization: the guest's
async timeout cannot preempt a native host function blocked inside such a call.

**Improve:** separate editor preflight/notifications from filesystem execution;
use background jobs with completion, cancellation, progress and revision checks.
Give LSP requests bounded waits and shell output explicit bounds. Do not hold
host/editor locks while performing potentially blocking I/O. Include installer
commit/reload in this work, not just fetch.
**Verify:** slow/unresponsive language servers, remote mounts, large deletes,
large components and slow shell expansion leave input/redraw responsive.

### S10 — Make file batches bounded, cancellable and isolated

Sources: [batch creation](../helix-plugin/src/lib.rs) (`apply_files`) and
[launch/execution](../helix-term/src/plugins.rs) (`Action::FileOps`, `file_ops`).

There is a 1000-operation limit per batch, but no corresponding active-batch
limit. Each accepted batch becomes a detached task. Execution does not check
that the original owner/request/grant is still current before each side effect;
reload/disable can invalidate the request while old work continues. Independent
batches can race on the same paths. Copy accepts any non-directory/non-symlink
source as a file, so a special file can also occupy a blocking worker indefinitely.

**Improve:** add per-plugin/global admission limits, immutable request provenance,
operation-time authorization, cancellation between safe boundaries, and conflict
serialization. Reject unsupported special files; bound depth/work and clean up
partial copies. Define which effects may complete after cancellation. Tokens
must not pretend to forcibly stop an already-blocked OS operation.
**Verify:** reload/disable before launch and mid-batch, overlapping batches,
FIFO/device copies, deep trees, queue saturation and partial copy failure.

### S11 — Share cleanup between trap/disable and reload

Sources: [disable handling](../helix-term/src/plugins.rs) (`refresh_wanted`),
[reload installation](../helix-term/src/plugins.rs) (`install`), and
[guest trap handling](../helix-plugin/src/lib.rs) (`Plugin::call`).

A trap clears host pending state and refreshes subscriptions; the editor then
kills processes. Reload additionally clears decorations, diagnostics, statusline,
sidebar and completion waiters. Disable does not perform equivalent UI cleanup.
A disabled renderer can leave old conceals/decorations visible, and a disabled
sidebar can keep dead commands and focus. Timers and background workers also
need lifecycle cleanup rather than just ignored late callbacks.

**Improve:** create one owner-scoped teardown path, used for disable, unload,
reload and shutdown. Clear only that owner's UI/requests and release unused
instance memory while retaining diagnostic information about the failure.
**Verify:** trap a plugin while each UI/request type is active; unrelated plugins
remain intact and no orphaned focused UI or periodic tasks remain.

### S12 — Release closed Markdown caches and reduce whole-document traffic

Source: [Markdown cache/update/events](../helix-plugin/examples/markdown/src/lib.rs)
(`CACHE`, `render_update`, `update`, `Registration`).

The cache retains source text, line offsets, fence state and all rendered layers
per document. The plugin subscribes to open/change but not close, so closing many
Markdown files retains those entries until reload or explicit cache removal.
Incremental parsing still reads the complete text over the ABI, splits and
compares whole old/new line lists, rebuilds line starts, clones retained output,
and replaces complete decoration layers. It improves parsing locality, not the
entire update's asymptotic cost.

**Improve:** subscribe to close and evict caches; bound cached bytes and handle
language/path changes. Add revisioned changed-range/delta reads and partial layer
updates when profiling justifies an API extension. Reuse unchanged storage and
avoid resending mode/raw-line settings on every edit.
**Verify:** open/close churn returns retained memory near baseline; benchmark
edits at start/middle/end, fence edits, multimegabyte files, and other renderers
such as lint/todos. Report ABI/copy/mapping cost separately from parser cost.

### S13 — Map decoration positions in batches

Sources: [decoration mapping](../helix-view/src/plugin_decorations.rs) (`map`)
and [position mapping](../helix-core/src/transaction.rs)
(`ChangeSet::update_positions`, `map_pos`).

Highlights start a new position-mapping traversal for each range. Overlays and
conceals call `map_pos` independently for each endpoint. `ChangeSet` already
documents the distinction: one sorted batch can be O(N+M), while independent
mapping can be O(N*M). Dense layers combined with large multicursor transactions
can therefore become expensive even when the renderer itself is incremental.

**Improve:** batch sorted endpoints per layer, reuse traversal state, and
explicitly preserve association/overlap semantics. Consider visible-range
rendering and dirty-region indexes only after profiling the simpler batch fix.
**Verify:** cross-product benchmarks of 0/1k/100k decorations and 1/100/1000 edit
positions, plus property tests comparing results with the current mapper.

### S14 — Budget event/completion fan-out and snapshot construction

Sources: [snapshots/events/completions](../helix-term/src/plugins.rs)
(`snapshot`, `queue`, `deliver_event`, `request_completions`) and
[guest call execution](../helix-plugin/src/lib.rs) (`Plugin::call`,
`outside_async_runtime`).

One event may synchronously call every subscriber, with a fresh snapshot and a
new scoped OS thread per call. Per-call deadlines do not establish an aggregate
frame budget: 20 handlers near a 250 ms limit can stall the loop for seconds.
Snapshots traverse every document and search views; if any enabled plugin can
read registers, register contents are collected for all snapshots. Change events
are coalesced, but post-command/focus/mode events are appended without a total
queue bound. The completion menu's one-second answer timeout starts after its
synchronous request dispatch, so it does not bound that dispatch itself.

**Improve:** add per-plugin latency telemetry and aggregate dispatch budgets,
coalesce/bound appropriate events, and yield between safe batches while
preserving fresh-snapshot ordering. Build permission/operation-specific snapshots
and cached document-view indexes. Measure whether a persistent call worker helps
before replacing the current scoped-thread design.
**Verify:** many subscribers/documents/registers, slow but non-trapping handlers,
input bursts and multiple completion sources; track p50/p95/p99 key-to-draw
latency and starvation, not only average single-event time.

### S15 — Bound compilation concurrency and cache maintenance

Source: [loading/cache](../helix-plugin/src/lib.rs) (`load_all`, `compile`,
`prune_cache`) and [host creation](../helix-term/src/plugins.rs) (`build_host`).

`load_all` creates an OS thread per enabled plugin and retains all compilation
results before installation. Each successful compile/cache read also scans the
shared cache directory to prune its own old versions. Many plugins can produce
CPU/memory contention and roughly quadratic directory-entry examination. A
plugin directory containing only disabled plugins still causes engine/runtime
creation before the enabled list is known.

**Improve:** use a bounded compiler pool, parse/filter enabled manifests before
creating the engine, and perform indexed/batched cache pruning once per load.
Bound component/cache file sizes and make multi-editor cache updates race-safe.
Prioritize a configured startup directory handler without waiting for unrelated
large components to finish compiling.
**Verify:** cold/warm loads at 0/5/20/100 plugins, all-disabled configurations,
large non-Rust components, concurrent editor instances and low-memory machines.

### S16 — Complete directory-handler loading/request state transitions

Source: [startup and directory handlers](../helix-term/src/plugins.rs)
(`load_in_background`, `install_background`, `load_from`, `open_directory`).

`LOADING` is cleared on successful current background installation, but a
superseding synchronous reload does not itself clear/drain that state. The stale
background result returns before clearing it. Failed task/join paths also return
without resolving pending directories. A deferred directory request has only a
path, not its initiating view/activity generation, so a later completion can
steal focus after the user has started other work.

**Improve:** model loads with a generation-owned completion/cancellation guard;
resolve pending requests on every success/failure/supersession path. Attach
request identity and view/activity state, define mixed directory/file argument
behavior, and cancel stale requests instead of opening late UI.
**Verify:** startup load followed by manual reload, load failure, changing the
handler/config while loading, several opens, and editing before load completes.

### S17 — Make install/update and lock verification recoverable

Sources: [commit and lock writes](../helix-term/src/plugins/install.rs)
(`commit`, `write_atomically`, `stage`) and
[load verification](../helix-term/src/plugins.rs) (`build_host`).

Update removes the old target before renaming the fresh directory, then writes
the lock afterwards. A crash or lock-write failure can leave the old plugin gone
or a new component paired with the old digest. Lock temp names use only the pid,
and lock read/modify/write has no inter-process serialization. If loading cannot
parse/read the lock, it records an error but uses an empty changed-plugin list
and continues loading, bypassing the expected verification decision. Git fetches
also use blocking `Command::output` without an explicit timeout/cancellation.

**Improve:** retain a rollback directory, serialize install/update/remove,
transactionally persist component plus lock state, and recover interrupted
transactions. Fail closed or require an explicit recovery path when integrity
metadata is invalid. Bound/cancel git fetches and captured diagnostics.
**Verify:** injected failure at every update stage, corrupt/unreadable locks,
concurrent editors, interrupted fetch and restart recovery, preserving the
previous usable version.

### S18 — Apply bounded-output and cancellation policies to core tools too

Sources: [compilation](../helix-term/src/commands/compilation.rs),
[grep](../helix-term/src/commands/typed.rs) (`grep`),
[quickfix](../helix-term/src/commands/quickfix.rs), and
[shell expansion](../helix-view/src/expansion.rs).

The plugin process executor has useful caps/backpressure, but core `:grep` uses
`Command::output` without comparable output/entry limits or a latest-request
generation. Slow old searches can replace newer quickfix results. Compilation
retains an unbounded buffer/history, polls supersession every 100 ms while
running, and times out waiting for a blocking pump without actually cancelling
that OS read; inherited pipe handles can keep the worker alive. Large output is
cloned and parsed into quickfix on completion. Quickfix parsing stats every
location, and treats all reported columns alike even though ripgrep reports byte
columns and editor positioning uses character/grapheme coordinates.

**Improve:** share bounded capture/streaming/cancellation primitives without
changing bindings or normal user command semantics. Add configurable output and
entry caps, job generations, notification-based cancellation and bounded pipe
shutdown. Batch/cache path checks and make each producer's coordinate encoding
explicit (including fallback grep output).
**Verify:** noisy commands, superseded searches, inherited pipes, SIGTERM-resistant
children, long sessions, duplicate locations, UTF-8 filenames/text and Windows
paths. Report truncation visibly rather than silently losing output.

### S19 — Plan oil edits as filesystem dependencies

Sources: [oil planning/execution](../helix-plugin/examples/oil/src/lib.rs)
(`plan`, `execute`) and [no-overwrite executor](../helix-term/src/plugins.rs).

The planner allows a destination that another operation will vacate, but does
not topologically order those dependencies or break rename cycles. Swapping two
names can therefore fail against the executor's no-overwrite check. `execute`
also adds a parent-create operation for each move whose parent is currently
missing; several moves into the same new directory can enqueue duplicate creates
because the first create has not executed yet. The second create then fails
partway through the batch. Retained entry/id maps also grow as directories are
visited, so long sessions need an eviction/reference policy.

**Improve:** build a dependency graph, deduplicate parent creation, reserve safe
temporary names for cycles, and reconcile partial successes with the remaining
user intent before retry. Bound metadata retained for no-longer-open listings.
**Verify:** swaps/cycles, rename chains, several moves into a new directory,
parent/child operations, partial failures/retry and navigation churn.

### S20 — Invalidate conceal ranges based on intersections, not length

Source: [conceal mapping](../helix-view/src/plugin_decorations.rs) (`map`).

A conceal is retained when its mapped length equals the old length. An
interior replacement with the same number of characters leaves both endpoints
and length unchanged, so this does not implement the documented rule that
editing hidden text reveals it until refreshed. An edit can also change
multi-scalar grapheme boundaries without changing a scalar count.

**Improve:** invalidate ranges intersecting replaced/inserted content, define
boundary insertion semantics, and revalidate grapheme/newline constraints where
needed. Retain cheap mapping for edits wholly outside the range.
**Verify:** same-length interior replacement, combining characters, emoji/ZWJ,
newline edits, boundary insertions, multiple views/selections and property tests
against a full recomputation.

### S21 — Turn the claimed guarantees into reproducible CI gates

Sources: [CI](../.github/workflows/build.yml),
[benchmark harness](bench/vs-helix.py),
[performance history](plugin-progress.md), and
[idle event regression](../helix-term/tests/test/plugins.rs).

The workspace checks do not automatically run native tests in the separate SDK
and example workspaces, verify rebuilt guest artifacts match source, or build
the plugin-disabled feature combination. Existing historical benchmarks predate
the new unconditional auto-reload interval and cannot establish the current
zero-idle-wake claim. The benchmark harness requires manually prepared files and
config/data directories, uses fixed sleeps, and does not identify the tested
commit/config/artifact state in its output.

During this review the default plugin integration run produced 67 passes and one
failure in `api_03_command_focus_and_idle_events_do_not_rearm_idle`; the isolated
rerun passed, and a four-test-thread rerun passed all 68 plugin tests. Preserve
all observations: this is timing-sensitive evidence,
not proof that the timer caused that particular assertion failure.

**Improve:** add fixture/ABI checks for frozen versions, SDK/guest native tests,
plugin-disabled builds, and deterministic lifecycle tests from S01–S20. Make the
performance harness generate scratch inputs/configs, record revision/profile,
wait for explicit readiness and clean up on failure. Add stable zero-wakeup and
bounded-memory gates; use representative p95/p99 latency runs for large files,
plugins and many open documents. Do not publish a new parity claim from debug
probes or a noisy single run.
**Verify:** clean-checkout execution and a deliberate regression fail the intended
gate; separate cold/warm, loaded/unloaded and auto-reload settings in results.

### S22 — Specify capabilities and asynchronous completion more explicitly

Sources: [versioned WIT](../helix-plugin/wit/),
[SDK](../helix-plugin/sdk/src/lib.rs), [plugin guide](../helix-plugin/README.md),
and [file/background execution](../helix-term/src/plugins.rs).

Adding imports to an existing world preserves old-component/new-host loading,
but a plugin using a newly added import may still carry the same `api` string as
an older host that lacks it. A generic linker failure is a weak compatibility
message. The API also needs consistent contracts for stale-result rejection,
progress, cancellation, partial file effects and ownership during reload.

**Improve:** add minimum-host/capability metadata or a negotiated capability
query, retain frozen guest fixtures, and generate reference documentation from
one source. Prefer typed settings parsing over the examples' ad-hoc string
splitting as configuration grows. Specify operation completion/revision semantics
before adding more global UI state or asynchronous features.
**Verify:** old guests on new hosts and new-capability guests on older hosts have
clear outcomes; examples/templates and API references agree with tested behavior.

## Remaining architectural capabilities

The follow-up capability audit and proposed API delivery waves are in
[`plugin-future-roadmap.md`](plugin-future-roadmap.md).

These are proposals, not regressions or authorization to start large UI changes:

| Capability | Why it matters | Suggested boundary |
| --- | --- | --- |
| Revisioned text deltas and partial decoration updates | Removes the whole-document transfer/replacement work in S12 | Opt-in change history with bounded retention and full-snapshot fallback |
| File-job progress/cancel and directory watch subscriptions | Makes file managers/indexers responsive on large/remote trees | Generic grant-scoped APIs; no periodic work when unsubscribed |
| View-scoped decorations and viewport queries | Supports several views of a document without rendering the whole file | Explicit view identity/lifetime; visible-range invalidation |
| Multiple plugin panels/floating views, folding, embedded terminals | Still beyond a single sidebar/plugin buffer | Separate owner-reviewed editor architecture proposals; keep plugins optional |
| Per-plugin timing/memory/job diagnostics | Makes slow handlers and leaked jobs attributable | Cheap counters while active; opt-in tracing rather than periodic polling |

## Existing upstream blocker

The roadmap's **cancel blocked filesystem workers** item remains valid. Guest
async timeouts protect the editor from suspended WASI calls, but a blocking-pool
OS FIFO open may outlive the guest until the file wakes. Track supported
Wasmtime/WASI/cap-std cancellation/nonblocking-open facilities and keep shutdown
nonblocking. Do not confuse this with S02's direct editor FIFO read or S03's
application-owned ticker lifetime; those have local implementation work.

## Suggested implementation sequence

1. Close the reproduced execution/path-validation gaps: **S01 and S04**.
2. Fix responsiveness and lifetime failures: **S02, S03, S06 and S09**.
3. Harden filesystem boundaries and request ownership: **S05, S10, S11, S16,
   S17**, then oil's dependency planning (**S19**).
4. Add memory/output budgets and the preview cap: **S07, S08, S18**.
5. Address measured scaling: **S12–S15 and S20**, with the tests/metrics in
   **S21** alongside each fix. Keep **S22** and the capabilities above as
   explicit API design work.

Do not raise budgets or add more polling to hide these failures. Every task
should retain old ABI fixtures and user bindings, update the roadmap/progress,
commit rebuilt guests only when their source/SDK/WIT changes, and be committed
and pushed after its validation.

## Validation performed for this review

Tests ran with the pinned toolchain and the repository's grammar-build disable,
non-incremental build and enlarged-stack settings.

| Check | Observed result |
| --- | --- |
| `cargo test -p helix-plugin` | 95 passed; 2 ignored benchmarks; no failures |
| `cargo test -p helix-core --lib doc_formatter` | 13 passed; 179 filtered out |
| `cargo test -p helix-term --lib` | 47 passed; no failures |
| Workspace formatting check | Passed |
| `cargo clippy --workspace --all-targets` | Passed without warnings |
| Plugin integration, default parallelism | 67 passed; 1 failed (idle assertion) |
| Isolated failing idle test | 1 passed |
| Plugin integration, four test threads | 68 passed; no failures |
| Auto-reload integration tests, one test thread | 2 passed; no failures |
| Markdown native tests (explicit manifest) | 3 passed; 1 ignored benchmark |
| Oil native tests (explicit manifest) | 3 passed; no failures |
| Normal editor build | Passed; used for scratch terminal probes |

The entire workspace/full documented integration suite, new release
benchmarks and macOS/Windows behavior were not rerun for this documentation review.

Bounded probes, using only temporary files/configuration:

- Direct plugin `:sh` without a process grant was refused; the same plugin's
  `:echo %sh{...}` wrote a scratch marker (**S01**).
- No-plugin normal-debug sessions recorded 13 thread context switches in each
  3.3-second sample with auto-reload enabled and disabled (**S02**). The timer's
  unconditional scheduling is established by source inspection.
- Replacing a watched scratch file with a FIFO prevented command processing;
  releasing the FIFO restored it (**S02**).
- The existing background `spin` guest timed out at a 300 ms limit with its
  host retained; dropping the host after 100 ms produced no completion over the
  next 1500 ms (**S03**). Probe processes exited after observation.
- Local staging rejected `../../victim.wasm` after changing a scratch file
  outside staging (**S04**).
- 32 immediate timer cancellations still returned 32 scheduling actions, each
  with no pending timer request (**S06**). No long-lived timer tasks were created
  by this host-only probe.
- A 100,000-character substitution input produced 100,000 replacements totaling
  25,600,000 payload bytes (**S08**); no large edit was applied to a real document.

No runtime fixes, key bindings, manifests, WIT or committed WASM artifacts were
changed by this review. Treat code-supported findings as targeted regression-test
work before implementation, and keep the reproduced cases as permanent tests.
