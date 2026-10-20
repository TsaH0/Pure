# Future plugin API roadmap

Reviewed **2026-10-20**, against master **`1268b872`**, fetched again after the
system review. API **0.4** is current. This document proposes future work;
none of the APIs named below exist unless explicitly listed as current.

The question is which plugins Pure cannot support **with native integration**,
and which APIs would unlock them. A command that runs an external tool, or an
editable text buffer, can already approximate many plugins. That does not give
the plugin access to the editor's existing syntax tree, debugger, layout,
snippet engine, or asynchronous save pipeline.

Use this file for feature/API planning. Use
[`suggested-improvements.md`](suggested-improvements.md) for the 22 current
correctness/performance findings, and
[`plugin-roadmap.md`](plugin-roadmap.md) for the completed milestones and agent
rules. The work here must preserve those rules and resolve the relevant review
findings before expanding the affected subsystem.

## Status (2026-10-20)

See [`plugin-fix-verification.md`](plugin-fix-verification.md) for independent
checks of `4bd8f2e9`. The imports and native snippet completion path work, but
F00/F01 remain partial, including queued background cancellation and resource
budgets. Host-side load-generation checks can be implemented internally; they
do not all require new callback exports or API 0.5.

The first delivery the roadmap recommends ("F00/F01, then F02 plus the
narrow F06 snippet path") has an initial implementation. It went into API 0.4 as
**imports only** (new `requests`, `revisions` and `snippets`
interfaces), which the compatibility rule allows: no new exports, no
changed records, so no 0.5 was needed yet. `testplugin3-v0.4.wasm`
freezes a 0.4 component built before the additions and is tested to
keep loading.

| Item | Done | Still open |
| --- | --- | --- |
| F00 | The 22 review findings: see the status table in [`suggested-improvements.md`](suggested-improvements.md) | What that table lists as left |
| F01 | `requests.capabilities`; `requests.cancel` for every request kind (callback dropped; timers aborted, programs killed, file batches skip what hasn't started, completions answered empty, popups closed); clear errors for "needs a newer Pure" and "built for another API version" | Request status/progress, typed errors, load-generation ids: they need new exports, so 0.5 |
| F02 | `revisions.revision` (Helix's document version, counting the plugin's own edits within a call) and `revisions.apply-edits-at` (refused when the text changed); trial: `api-demo`'s background `sort` | `changes-since` deltas, origin-tagged change subscriptions, multi-document conditional edits |
| F06 | `snippets.complete`: plugin completions inserted through the native snippet engine (tab stops, placeholders, choices); trial: `examples/snippets` | Snippet variables (the editor doesn't resolve them yet), inline suggestions (ghost text, partial acceptance) |

Next in the roadmap's order: F03 (views/viewport), F05 (syntax trees),
F07 (LSP services), F09 (watches). F04, F10 and later touch UI and
process architecture and need the owner's review first.

## What already works

The [0.4 WIT](../helix-plugin/wit/v0.4/helix-plugin.wit) is the contract, with
implementations in the [host](../helix-plugin/src/lib.rs),
[version adapters](../helix-plugin/src/api/world.rs),
[editor bridge](../helix-term/src/plugins.rs), and
[SDK](../helix-plugin/sdk/src/lib.rs). Checking only the old M12 limitations
would miss features added since that list was written.

| Plugin family | Current support; do not treat this as missing |
| --- | --- |
| Text transforms, selection tools, simple refactors | Read text/ranges and selections, apply edits, set selections; one call's edits to a document form one undo step |
| File/grep/branch finders | Static/live pickers, query generations, streaming stdout, previews, positioned opens and splits; `examples/finder` |
| File managers and file trees | Owned editable buffers and buffer keys, ordered create/rename/copy/delete/trash results, directory handler and one sidebar; `examples/oil` and `examples/tree` |
| Linters, TODO markers, Markdown rendering | Diagnostics, highlights, inline text, overlays, signs, full-line backgrounds, conceal and render modes; `lint`, `todos`, `markdown` |
| Simple completion sources | Plain-text insertions in the native menu, descriptions and a replacement start; `examples/emoji` |
| Clocks, yank history and command automation | Opt-in timers, registers, events, statusline segments, commands and default/buffer mappings |
| External Git, HTTP/AI, test and build tools | Granted processes, one-shot stdin, captured/streamed output, popups, pickers and text buffers; subprocesses run with the user's rights |
| Languages, formatters and debuggers configured by the user | Existing `languages.toml`, grammar/query files, native LSP/DAP and formatter support; a new plugin API is not required just to configure these |

## Plugin families that remain blocked or limited

**Limited** means a useful version is possible now, but the listed integration
is absent. **Blocked** refers to that specific behavior, not every plugin in
the family. **Core design** means adding a WIT function alone will not implement
the feature. Neovim names below describe user expectations, not a promise of
source compatibility or a Lua runtime.

| Plugin or behavior | Status today | Missing boundary | Roadmap |
| --- | --- | --- | --- |
| LuaSnip-style snippets with linked placeholders and choices | Blocked for native snippet sessions; inserting template text works | Explicit snippet format/session and native tab-stop integration | F06 |
| Copilot-style inline suggestions with partial acceptance | Limited: external inference and annotations work; no dedicated suggestion/acceptance lifecycle | Revisioned, view-scoped ghost text and acceptance transactions | F02, F03, F06 |
| AST-aware text objects, structural editing and symbol outlines | Limited: plugins can parse text themselves or call tools | Bounded access to the already-parsed syntax tree and native action context | F05, F08 |
| Trouble-style aggregation of existing LSP diagnostics | Limited: plugins can publish their own diagnostics, but cannot read the editor's diagnostic collection | Diagnostic snapshots and change subscriptions | F07 |
| Aerial/navic-style outlines/breadcrumbs using the active language server | Limited: own parser/tool works; sidebar/text decorations exist | Document/workspace symbols, active-server capabilities and result events | F05, F07 |
| Refactoring/code-action frontends using the editor's LSP clients | Limited: commands/tools can perform some actions, but there is no structured result/apply bridge | Typed asynchronous LSP requests, workspace edits and completion receipts | F02, F07 |
| A formatter plugin that participates in ordinary file saves | Blocked for an awaited plugin pre-save provider; configured external formatters already work | Bounded save/provider pipeline with cancellation and failure policy | F08 |
| Custom repeatable operators, motions and count/register-aware commands | Limited: plugin commands can edit/select, but receive string args rather than native action context | Count/register/view context, repeat/undo contract, safe native command execution | F08 |
| Reactive plugins that observe edits from other plugins | Limited: plugin-applied edits suppress ordinary change hooks | Origin-tagged revision/delta subscriptions with feedback-loop protection | F02 |
| Session/layout managers and viewport-aware minimaps | Limited: open/split commands work; no structured view/layout/viewport query | Stable view identity, layout snapshots, viewport events and restore primitives | F03 |
| Simultaneous tree + test explorer + debugger panels | Limited to one sidebar plus normal text splits | Independently owned panel handles, docking/focus and host-rendered widgets | F04, F11 |
| Interactive floating dashboards/forms | Limited: one-shot popups, prompts, pickers and text buffers work | Persistent components, focus/input/resize/close lifecycle and widget actions | F11 |
| Native test/task adapters writing the quickfix list | Limited: run tests and show own diagnostics/pickers; no typed quickfix import/export | Structured location lists, task progress/cancel and producer identity | F08, F10 |
| nvim-dap-ui-style frontends for the existing debugger | Blocked for reading the current debugger session | Session/stop events, threads/frames/scopes/variables/breakpoints and typed controls | F11, F12 |
| Gitsigns/Diffview-style integration with the editor's existing diff state | Limited: `git` subprocesses, text diff buffers, signs and native hunk commands work | Revisioned native diff/hunk data, change notifications and targeted actions | F03, F13 |
| Persistent REPLs, language kernels and bidirectional tool protocols | Limited: restart a process per request or show a transcript | Persistent stdin writes, separate stdout/stderr chunks and session cancellation | F10 |
| Toggleterm-style terminal inside a split/panel | Core design: whole-terminal handover works now | PTY lifecycle plus terminal emulator, rendering, resize and focused input routing | F10, F11 |
| Live project indexers/watch-based trees without polling | Limited: explicit rescans, WASI reads and external watchers work | Grant-scoped file watches, bounded async directory/file reads and resync events | F09 |
| Fast large-document renderers | Possible, but still transfer/replace whole text/layers in common paths | Revision deltas, visible ranges and partial layer updates | F02, F03 |
| UFO-style folds and code/Markdown section collapsing | Core design: conceal is single-line and is not folding | Fold layout/motion/search/selection semantics, then fold provider/control API | F14 |
| Notebook results and multiline inline review/suggestion blocks | Limited to text buffers or single-line annotations | Virtual block layout, anchors, hit testing and bounded retained content | F03, F11 |
| Remote/URI-backed files integrated with normal `:open`/`:write` | Limited: an owned buffer can load/save remote text explicitly | URI provider resolution, async load/save/stat, conflicts and normal-file lifecycle | F15 |
| Shared live editing/collaboration | Research: network helpers and edits alone are insufficient for robust collaboration | Revision/origin foundation plus a separate convergence, undo and identity design | F02; later research |

## Implementation order and scope

All entries below are open proposals. Names such as `documents.changes-since`
are **illustrative**, not functions plugin authors can call. Size estimates
describe scope, not delivery dates: **M** is a bounded cross-layer change,
**L** spans several editor subsystems, **XL** needs a core architecture design.

| Wave | Items | Result | Review boundary |
| --- | --- | --- | --- |
| 0 | F00–F01 | Reliable lifecycle, budgets and a compatibility plan | Required foundation; agree the next ABI before changing WIT |
| 1 | F02–F06 | Revision-safe work, view/buffer control, syntax access and real snippets | Design the contracts; preserve existing editing and user bindings |
| 2 | F07–F10 | Semantic tooling, native workflows, watch/index jobs and persistent tools | Review provider ordering, data permissions and resource policy |
| 3 | F11–F13 | Richer panels, debugger and native diff integration | Panel/input architecture needs the owner's design review |
| 4 | F14–F15 | Folds and URI-backed documents | Separate owner-reviewed core designs; not just ABI additions |

A wave is not a single large patch. Deliver one capability with a kept trial
plugin, tests and measurements, then commit/push it before beginning the next.
F05/F06 and F07/F09 can be separate changes once their foundations are ready.
F10's pipe sessions do not require first building an embedded terminal.

## Candidate API surface

These names describe the proposed contracts for design review; they are not
valid 0.4 imports. Preserve existing APIs and add these alongside them.

| Item | Candidate surface | Key result/constraint |
| --- | --- | --- |
| F01 | `capabilities.describe`, `requests.cancel/status` | Supported features, generation-owned requests and typed terminal results |
| F02 | `documents.revision/read/changes-since/apply-if-revision` | Bounded text/deltas and conditional edits with receipts |
| F03 | `views.list/current/layout/viewport/focus/open/close`, `layers.patch` | Opaque view handles, explicit focus and revisioned partial updates |
| F04 | `buffers.create/update/append/options/close`, `commands.execute` | Owned buffers and proper native command completion |
| F05 | `syntax.node-at/ancestors/captures` | Revision-scoped immutable descriptors and bounded query results |
| F06 | `snippets.insert`, `completion.complete-vnext`, `suggestions.show/accept/cancel` | Explicit insertion format and host-owned snippet/suggestion sessions |
| F07 | `semantic.capabilities/request/cancel`, `diagnostics.read/subscribe` | Typed LSP services, normalized locations and result events |
| F08 | `formatting.register`, `actions.register`, `locations.read/replace/append` | Awaited save edits, native action context and structured quickfix integration |
| F09 | `fs.read/list/watch/unwatch`, `file-jobs.progress/cancel` | Grant-scoped asynchronous bounded I/O and watch overflow/resync |
| F10 | `sessions.start/write/close-input/cancel` plus output/exit events | Bounded bidirectional pipes; PTYs are a separate transport/design |
| F11 | `panels.create/patch/focus/close`, `blocks.set/clear` | Owned host widgets and multiline virtual content after core design |
| F12 | `debug.sessions/threads/frames/variables/control` plus lifecycle events | Capability-checked access to the existing DAP session |
| F13 | `vcs.snapshot/hunks/subscribe`, `vcs.apply-hunk` | Base/buffer/index validation and distinct read/write policies |
| F14 | `folds.provide/query/open/close` | View-local controls after core folding exists |
| F15 | `providers.register-scheme`, load/save/stat callbacks | URI identity, conflict tokens and normal document lifecycle |

### F00 — Harden the current host before increasing its surface (L)

Track implementation in the existing review rather than copying its checklist:
process grants **S01**, native I/O **S02/S09**, worker/timer teardown
**S03/S06/S11**, installation and filesystem boundaries **S04/S05/S10/S17**,
aggregate budgets **S07/S08/S14/S15/S18**, and regression gates **S21**.

New APIs must share the corrected owner/request lifecycle and budgets. Adding a
second implementation of file, timer or process ownership would reproduce the
same problems. Keep the upstream WASI blocked-worker limitation visible; it
does not excuse locally fixable UI or ticker lifetime bugs.

**Done when:** the relevant reproduced cases are permanent tests and teardown
leaves no active plugin work, focused dead UI or recurring tasks. Do not claim
this is already satisfied by historical benchmarks.

### F01 — Version negotiation and common asynchronous request contracts (M)

Expose a bounded capability/minimum-host description and standardize request
ownership, cancellation, terminal completion, progress, typed errors and stale
result handling. Every request belongs to a plugin load generation; document
and view requests also carry the appropriate identity/revision. Cancelling
removes queued work and callbacks; in-progress OS effects have an explicit
best-effort policy. Side effects need receipts, not merely "queued successfully."

The SDK should release closures/handles when work is cancelled, a view/document
closes, or its plugin unloads. Publish which contexts may use each API; preserve
the rule that normal event handlers cannot interrupt typing by opening UI.

**Version plan:** propose **0.5** for new callback/event exports and changed
record shapes; do not add them to a released world's exports. Freeze 0.4 guest
fixtures before migrating the SDK default, keep 0.1–0.4 adapters, and add an
`api-v0-4` SDK feature. Current policy still permits additive host imports in a
released world; that does not make new callbacks or changed existing types
compatible. A capability check cannot make an absent required import link: use
manifest/version validation and a clear pre-instantiation error too. Future
event extensibility must have its wire-compatibility rules tested, not assumed.

**Targets:** host `PendingKind`/`Action`/`Callback`, version adapters, manifest
loader, SDK request registries and frozen fixtures.
**Verify:** old/new guest-host combinations, unsupported capabilities,
cancel-before-launch, reload/disable/shutdown, duplicate/late completions, and
callback registry memory returning to baseline. No periodic request sweeper.

### F02 — Revisioned documents, deltas and conditional transactions (L)

Expose document revision/metadata, bounded range reads, `changes-since` with a
full-snapshot fallback, and opt-in change batches containing old/new revisions,
edits and origin. Coalesced events must preserve a valid transformation or say
history was lost. Deliver bounded history only while subscribed; do not retain
every edit forever. Add close/path/language identity information where needed.

Add `apply-if-revision` and an application receipt for async tools. A background
result for an old revision must not silently edit new text. Define rejection or
explicit rebase; keep char offsets and the primary-selection convention. For
multi-document refactors, validate every expected revision before mutating any
document and define coordinated undo/failure behavior. This is not a promise
that disk operations, language servers and buffers form one atomic transaction.

Ordinary change events currently suppress plugin-applied edits. Add a separate
origin-aware subscription for renderers/indexers, with bounded causal chains
and no automatic self-triggering. Do not remove the existing loop protection.

**Targets:** `DocumentSnapshot`, `DocumentDidChange`/`ChangeSet`, event queue,
editor apply path and SDK delta helpers.
**Verify:** async result after typing/close/reopen, plugin-to-plugin updates,
coalescing/history overflow, multicursor/Unicode/CRLF, cross-document rejection,
undo/redo and memory bounded by configured history rather than session length.

### F03 — Views, layout, viewport and partial decorations (L)

Expose opaque view ids, focused view, document-to-view mapping, per-view
selections/scroll, viewport bounds, layout snapshots and targeted open/focus/
split/close operations. Return handles or completion receipts from opening
instead of relying only on a later document-open callback. Scope deferred
results to their initiating view; opening a background result must not steal
focus after the user moves elsewhere.

Add coalesced opt-in view/viewport/resize events and document- versus view-scoped
decoration ownership. Provide revisioned partial replacement/patches for layers
and visible-range requests. Keep the current whole-layer setters for simple
plugins and old guests. Viewport coordinates must specify document versus
visual lines, wrapping, conceal and clipping. Layout serialization may initially
cover existing splits; editor tabs/workspaces are not already provided by it.

**Targets:** `Snapshot`, `View`, `Tree`, `PluginDecorations` and renderer.
**Verify:** one document in two differently scrolled views, resize/soft wrap,
closing the initiating view, session restore with missing files, partial/full
layer equivalence, and no guest calls during rendering or unsubscribed scrolling.

### F04 — Owned buffers and a safe native command bridge (M/L)

Add owned-buffer handles, update/append/close, read-only/output mode, local
display options and explicit visibility/focus/close lifecycle. Buffer options
must be owner-scoped, reversible and applied only where requested. Preserve
ordinary editable buffers, `on-buffer-write`, unsaved-change protection and
the owner's key mappings. Use bounded appends for task logs/transcripts.

Fix a specific command-bridge limitation: `run_editor_command` builds a command
context, executes it, then only warns if it produced compositor callbacks or
`on_next_key_callback`. Those callbacks are not installed, so `run-command` is
not a universal way to reuse native pickers/prompts/actions. Dispatch supported
commands through the proper UI path, carry plugin grant provenance through
deferred execution, and return structured unsupported/completion results.
Do not emulate this with arbitrary synthetic keystroke injection.

**Targets:** owned-buffer actions, `run_editor_command`, compositor/jobs and
buffer key resolution.
**Verify:** native UI command completion/cancellation, nested command grants,
hidden buffers, reload with dirty owned buffers, read-only output, user/default
binding precedence and commands requiring a count or next-key continuation.

### F05 — Reuse the editor's syntax trees (M/L)

Provide bounded queries for a node/range, ancestors, named children, captures
and language injection boundaries. Prefer copied immutable descriptors tied to
a document revision over Rust pointers or a tree retained across edits. Reuse
the editor's parsed tree; a plugin should not have to ship another parser just
to ask "which function am I in?" Query caches and result counts need limits.

**Targets:** `Syntax`, query/tag/textobject helpers, read permission checks and
the snapshot boundary. Grammar/query files remain a supported configuration
route; this proposal is not a native shared-library loader inside WASM.
**Verify:** missing grammar, injected languages, parse lag after an edit,
cancelled/expensive queries, char/byte conversion, large trees, and no extra
parse/query work without a requesting plugin.

### F06 — Native snippets and inline suggestion providers (L)

Add explicit plain-text versus snippet completion formats and a native snippet
insertion API, reusing the existing snippet parser/session/tab-stop machinery.
Plugin completion currently becomes `CompletionItem::Other` with a plain
transaction; setting `kind = "snippet"` changes the label, not insertion
semantics. Specify placeholder choices, linked edits, variable resolution,
multicursor behavior and undo/repeat. Do not add a parallel WASM snippet engine.

Separately add a revisioned, view-scoped suggestion handle with replacement
range, candidate/cancel, and accept-all/word/line actions. Keep ghost text out of
the document and undo history until acceptance. A plugin may already call an
AI tool and annotate text; the missing part is this integration. Bind acceptance
only through explicit user configuration or non-conflicting optional mappings.

**Targets:** plugin completion conversion, native completion UI, `snippets`,
active snippet state and view decorations.
**Verify:** literal `$`/braces versus snippets, tab stops/choices, async stale
answers, several selections, insert-mode transitions, completion-menu conflicts,
Unicode/wrapping, partial acceptance and no extra requests on unsubscribed keys.

### F07 — Typed semantic services and diagnostic reads (L)

Start with capability queries and typed asynchronous requests to existing LSP
clients: document/workspace symbols, hover, definitions/references and code
actions. Expose bounded diagnostic snapshots plus change subscriptions for
aggregation. Return document/revision and location encoding; normalize positions
at the host boundary. Reuse the native workspace-edit application path rather
than asking plugins to replay partially understood edits.

Use the configured server lifecycle. Do not launch a second LSP just because a
plugin wants an outline, or initially expose unrestricted JSON-RPC access to
every server. If arbitrary extension methods are later needed, define the trust
boundary and payload limits separately. Native debugger state is F12, not LSP.

**Targets:** LSP command handlers/registry, diagnostics providers and request
broker. Read access must match the data exposed; editor-mutating actions and
server commands retain appropriate authorization.
**Verify:** multiple/no servers, unsupported methods, server restart, cancellation,
stale results, UTF-8/16/32 positions, document close and bounded diagnostic floods.

### F08 — Save providers, command context and location lists (L; split patches)

Three independent extensions reuse native workflows:

- **Formatting/save:** register a bounded async formatter/pre-save edit
  provider with explicit order relative to configured formatters/code actions,
  revision validation, timeout/cancel and failure policy. Await it in the native
  save pipeline. Existing post-save callbacks cannot safely substitute for this
  by recursively saving again. Ordinary auto-save can already be built with
  timers and commands; a pre-save provider is not required for that basic case.
- **Actions:** supply native count/register/view/selection context and a
  documented repeat/undo policy for plugin motions/operators/actions. Reuse
  selection/edit transactions; do not add a synchronous WASM call on every key
  or an unrestricted keystroke interception hook.
- **Quickfix/tasks:** expose bounded structured location-list read/replace/
  append/select, with producer identity, generation, coordinate encoding,
  progress and cancel. Keep diagnostics distinct from arbitrary task locations;
  preserve `]q`/`[q` and other owner bindings.

**Targets:** save pipeline, native command context/history and quickfix store.
**Verify:** manual save/`:wa`/format-on-save with conflicts/timeouts, repeat and
undo across multicursors, overlapping test runs and quickfix lists containing
unsaved documents or Unicode locations. No provider call unless registered.

### F09 — Watch-based files and bounded background filesystem services (L)

Expose async bounded/paged directory listing and file reads, shared grant-scoped
file/directory watches, and operation progress/cancel. Watch events need rename,
delete, overflow/resync and root-lifetime semantics. Share native watches across
plugins; unregister when the last subscriber leaves. Unsupported filesystems
need an explicit manual/opt-in fallback, not silent periodic polling.

The CPU worker currently has no files/editor access. Indexers therefore need a
safe asynchronous file service or an explicitly scoped worker capability, not
permission to make arbitrary blocking WASI reads on the editor call thread.
Keep handle-relative authorization, file-type checks, job/byte/depth budgets and
cancellation from F00. Changes to grants/workspaces invalidate pending work.

**Targets:** file executor, watch broker/editor file invalidation and worker
request paths; reuse this infrastructure for safe auto-reload where feasible.
**Verify:** root/symlink changes, network mounts, overflow, large/deep trees,
FIFO/device rejection, two plugins watching the same path, and zero recurring
wake-ups with no events/subscribers. Cancellation must not overpromise stopping
an OS call already blocked in a library.

### F10 — Persistent process sessions before embedded terminals (L/XL)

Add a separate bounded session API: start, write/flush/close stdin, subscribe to
tagged stdout/stderr byte chunks, cancel and await exit. The current API supplies
stdin once and closes it, streams complete stdout lines only, and delivers full
stderr at completion. It is unsuitable as a general bidirectional kernel/tool
protocol or prompt-without-newline stream. Preserve the existing one-shot API.

Specify active/idle lifetime limits for sessions rather than merely removing
today's timeout/output limits. Bound queued input/output and retained transcript
bytes, handle framing/partial UTF-8 in SDK helpers, and release processes on
owner unload. Pipes waiting for data need no recurring timer or guest call.

Continue to use `allow-process` as the sole program gate, including argv and
cwd handling. No per-program prompts or separate gate for starting a PTY.
Subprocesses retain user-account rights; new APIs do not make them sandboxed.

Embedded terminals are a later **XL** subproject: PTY support, an optional
terminal emulator, screen/damage updates, resize, scrollback and focused input
routing in a view/panel. Running `bash` in an ordinary text buffer does not
implement these. Construct and schedule them only for an active terminal.

**Targets:** process executor, request/session broker, SDK stream/framing
helpers; terminal rendering requires F11 and a separate core design.
**Verify:** no-newline prompts, separate stderr, binary chunks/backpressure,
inherited pipes, rapid cancel/reload, process groups, long idle sessions with no
periodic wake-ups, and platform-specific teardown. Terminal validation also
needs resize, escape sequences, mouse/paste and clean terminal restoration.

### F11 — Owned panels, host widgets and virtual blocks (XL)

Design panel handles and a bounded set of host-rendered components: lists,
trees, text/log views, actions and forms. Add focus/resize/visibility/close
events, stable item identity and partial updates. Start with multiple owned
panels/docking if approved; retain current sidebar/popup APIs as simple clients.
Keep input navigation, hit testing and painting in the host so a plugin is not
called for each render cell or navigation key.

Separately define anchored multiline virtual blocks for notebook/test/review
results. Current annotations explicitly reject line breaks; conceal only hides
part of one line. Blocks affect document-to-screen mapping and must integrate
with wrapping, selection, scrolling and mouse behavior. They are not just longer
annotation strings. Text output is the first scope, not an HTML/webview runtime.

**Targets:** compositor/components, view tree/sidebar, annotations/formatter and
position mapping. Review placement, user key ownership and focus arbitration
with the owner before implementation; this roadmap is the proposal.
**Verify:** several plugins/panels, close/trap/reload, small terminals, Unicode,
wrapping/mouse, hidden-panel suspension, bounded rows/bytes and unchanged empty
render path. Publish a core UI design before committing to a general widget API.

### F12 — Observe/control the existing debugger (L)

Expose active-session capabilities and lifecycle/stop/continue/output events,
then bounded queries for threads, frames, scopes, variables and breakpoints.
Add typed controls for stepping, breakpoint changes and frame selection. Scope
handles to a debug session and stop generation; expanding an old variable tree
after continuing must fail cleanly. A console/evaluate facility needs a clear
trust policy because evaluating debuggee code can have side effects.

Reuse native DAP configuration and sessions; panels use F11 or existing buffers
initially. A plugin may run its own adapter externally today, but it cannot
inspect the editor's active debugger through the current ABI.

**Targets:** DAP registry/client, native DAP handlers and request bridge.
**Verify:** restart/disconnect while queries are pending, large variable trees,
adapter errors, several threads, unsupported operations and no DAP work without
an active subscription/request.

### F13 — Native diff/hunk and VCS notifications (M/L)

Expose revisioned diff-base/hunk snapshots, current branch/status where already
available, and change notifications from the existing VCS provider. Hunk actions
must validate base and buffer/index revisions; keep buffer revert, working-tree
edits and index staging separate. Staging needs its own explicit filesystem/
trust design; an existing diff handle is not permission to write the Git index.

This is optional native integration, not a prerequisite for Git plugins:
granted `git` subprocesses plus pickers/diff buffers already support many
workflows. Implement only where reusing native state avoids duplicate work or
makes targeted actions safer.

**Targets:** `helix-vcs`, document diff handles, native hunk commands and F01.
**Verify:** edits while viewing a hunk, index/branch changes, rename/deletion,
binary/untracked files, cancelled actions and no extra background repository
scan when unsubscribed.

### F14 — Core folding, then a fold provider API (XL)

First design folds in the formatter/view/motion/search/selection model, including
wrapped and concealed text, hidden cursors, nested folds and multiple views.
Then expose versioned fold ranges, open/close/toggle, state queries and optional
syntax/LSP/plugin providers. Fold state is view-local; content/revision identity
is document-local. Keep old navigation behavior when no folds exist.

**Targets:** formatter and position mapping, view state, motion/search/render
paths, then syntax/LSP/fold providers. Needs the owner's design review.
**Verify:** selection across a fold, search/jump reveal, edits/undo inside folded
content, multi-view behavior, nested malformed ranges, mouse/soft wrap, and no
new per-grapheme plugin work. Conceal is not a substitute for this design.

### F15 — URI-backed document providers (XL)

Generalize the useful owned-buffer model only after F01/F02/F04: scheme/provider
registration, async read/write/stat, stable URI identity, conflict tokens,
encoding, modified/saved state and cancellation. Integrate with normal open,
write/`:wa`, close and recovery paths. Directory enumeration/watch is an
additional optional provider capability, not proof all remote paths behave as
local files. Define URI handling for LSP, quickfix and workspace edits.

Remote text editors/database viewers can already use explicit owned buffers
and granted tools. This proposal supplies normal-document behavior; it does
not automatically provide a virtual mount or a network transport. Use existing
granted helpers first, and decide any new native network capability separately.

**Targets:** document identity/open/save, provider registry and location paths.
**Verify:** disconnect/offline recovery, conflicting remote updates, duplicate
opens, long saves while editing, encoding errors, provider unload with dirty
buffers and no synchronous remote I/O in UI/guest calls. Needs core design review.

## Optional extensions and deliberate limits

These are not prerequisites for the waves above:

| Request | Recommendation |
| --- | --- |
| Direct HTTP inside WASM | Granted `curl`/helpers already enable HTTP/AI plugins. Consider bounded async HTTP only for a demonstrated need; it needs a separate explicit network/data policy, streaming/cancel and TLS/proxy design. Do not silently add WASI sockets/environment inheritance. |
| Clipboard-reading managers | Clipboard registers are deliberately write-only in the current host. Ordinary register/yank history works. Native clipboard reads/change events require an explicit privacy/grant decision and event-driven platform support; external tools already require a process grant. |
| Plugin-to-plugin services | Begin with documented versioned data/contracts or existing commands. A later opt-in service bus needs capability discovery, ownership, deadlines, cycle prevention and bounded messages, rather than shared mutable WASM memory. |
| Hot settings/reload/unload callbacks | Add typed settings schemas and lifecycle notifications if needed for resource-owning plugins; do not rely on a guest unload callback for safety. Host teardown must work even when the guest traps or never responds. |
| Collaboration | F02 provides useful revisions and origins, not a CRDT/OT engine. Shared undo, convergence, peers, authentication, reconnect and permissions need a separate design and tests. |
| Images/HTML dashboards | Terminal graphics protocols and webviews are separate platform/rendering projects. Keep F11 text/widgets scoped first; plugin API exposure alone cannot add a browser or reliable image rendering. |
| Existing Neovim/VS Code plugins unchanged | Not supported: Pure has a WIT component contract, not Lua/editor-object or VS Code extension compatibility. Ports use generic capabilities; do not promise arbitrary native libraries or dynamic editor internals. |

## Performance and compatibility gates for every capability

1. **No unrequested cost:** preserve the atomic subscription fast path. No new
   plugin enumeration, snapshot copy, filesystem walk, parse, WASM call or timer
   in startup/input/render paths without an active request/subscription.
2. **No periodic idle work:** shared watches and pipes are event-driven; timers
   exist only for requested deadlines/clocks. Cancelled/closed/unloaded owners
   leave no live sleepers, streams or recurring work. Track the current
   auto-reload regression in S02 rather than treating zero idle cost as proven.
3. **Bound totals:** enforce per-plugin and global counts/bytes/history budgets,
   backpressure, fair admission and aggregate UI dispatch limits. Per-call
   deadlines and per-linear-memory limits are not total resource budgets.
4. **Revision/lifetime checks:** every delayed effect checks owner generation,
   document/view identity and relevant revision. Validate at application time;
   return errors for stale work. A path string alone is not a capability.
5. **Reuse host services:** share syntax/LSP/DAP/VCS/watch state and native
   snippet/undo/format pipelines. Measure before adding another worker/runtime
   or cloning whole documents. No guest execution from paint/hit-test loops.
6. **Measure release behavior:** compare no-plugin, installed-but-disabled,
   loaded-unsubscribed and active-feature configurations against the same
   baseline. Record revision, profile, machine/input, cold/warm state, p50/p95/
   p99 latency, CPU, idle wake-ups and retained memory. Include many plugins,
   many views/documents and large inputs. Historical measurements are not new
   acceptance results; this roadmap did not rerun release benchmarks.
7. **Retain compatibility:** test frozen 0.1–0.4 components, add the new world
   beside them, and validate error messages for unsupported imports/features.
   Do not change existing export signatures/types or the owner's bindings.
8. **Keep a trial plugin:** each delivered API has an optional example under
   `helix-plugin/examples/<name>/` with source, manifest and built component.
   Retain it. Rebuild with `build-plugins.sh` when WIT/SDK/guests change. Ship
   generic APIs in core, never automatically installed file managers or tools.

The first useful delivery is **F00/F01, then F02 plus the narrow F06 snippet
path**. That establishes safety/revisions and unlocks a concrete missing plugin
without beginning a UI rewrite. Next prioritize F03/F05/F07/F09 for semantic
tools and large-file efficiency. Treat panels, terminals, folds and URI providers
as separately reviewed architecture work.

## Source map and verification of this roadmap

The limitations above were checked against the current contract and adapters,
not inferred solely from missing example plugins:

| Boundary | Code checked |
| --- | --- |
| Actual public imports/exports, event records and completion fields | [0.4 WIT](../helix-plugin/wit/v0.4/helix-plugin.wit), [version adapters](../helix-plugin/src/api/world.rs), [SDK bindings/exports](../helix-plugin/sdk/src/lib.rs) |
| No structured revision/view/syntax/LSP/DAP/quickfix API | [host `DocumentSnapshot`/`Snapshot`/`Action`](../helix-plugin/src/lib.rs), [editor snapshots and hooks](../helix-term/src/plugins.rs) |
| Plain plugin completion versus native snippet insertion | [plugin completion conversion](../helix-term/src/handlers/completion/plugin.rs), [completion UI `lsp_item_to_transaction`](../helix-term/src/ui/completion.rs), [native snippets](../helix-core/src/snippets.rs) |
| Deferred UI command callbacks discarded | [editor bridge `run_editor_command`](../helix-term/src/plugins.rs) |
| One sidebar; normal split views already exist | [sidebar](../helix-view/src/sidebar.rs), [view tree](../helix-view/src/tree.rs), [view state](../helix-view/src/view.rs) |
| No multiline plugin annotations; conceal is single-line | [host `set_annotations`/`set_conceals`](../helix-plugin/src/lib.rs), [decoration mapping](../helix-view/src/plugin_decorations.rs), [document formatter](../helix-core/src/doc_formatter.rs) |
| One-shot stdin, line-based stdout and final stderr | [process executor](../helix-term/src/plugins/process.rs), [SDK request handling](../helix-plugin/sdk/src/requests.rs) |
| Native semantic/save/debug/diff services available to reuse | [LSP commands](../helix-term/src/commands/lsp.rs), [document save/state](../helix-view/src/document.rs), [DAP handlers](../helix-view/src/handlers/dap.rs), [VCS](../helix-vcs/src/lib.rs), [quickfix store](../helix-term/src/commands/quickfix.rs) |

This follow-up changes documentation only: no API implementation, fixture
rebuild or new performance result. Static checks validate source links and
roadmap references; implementation must add the regression/ABI/terminal tests
and measurements specified per capability.

Documentation validation checked all 16 item ids, the 23-behavior matrix, source
and cross-links, table shapes and whitespace. `cargo fmt --all` and
`cargo clippy --workspace --all-targets` passed without warnings. Runtime test
suites were not repeated for this documentation-only follow-up.
