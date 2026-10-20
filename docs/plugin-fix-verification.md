# Verification of plugin fixes

Checked working branch **`claude/youthful-cerf-f620zv` at `4bd8f2e9`** against
the original review at **`1268b872`**. At this check, remote **master was still
`1268b872`**: the fixes below were on the working branch, not merged into master.

There is substantial progress, but the issues are **not all resolved**. Passing
regression tests verifies the cases they cover; it does not close the resource
and lifecycle gaps listed below. This verification changes documentation only.

## Current findings

References: [original findings and implementation status](suggested-improvements.md)
and [future API roadmap](plugin-future-roadmap.md).

| ID | Verified change | Remaining scope |
| --- | --- | --- |
| S01 | Literal `%sh` shell expansions in plugin-issued commands now require the process grant; editor tests and scratch terminal check pass | The broader execution-provenance design remains relevant to future command APIs |
| S02 | Auto-reload timer is gated when disabled/no file documents; existing FIFO replacements are skipped; tests pass | Enabled file sessions still poll; regular-file reads/diffs remain synchronous and path checks do not close replacement races |
| S03 | Host drop keeps epoch interruption alive until active calls finish; the real-component drop/queued-worker regression passes | General request cancellation is a separate issue below |
| S04 | Component filename validation occurs in manifest parsing before local staging copies; installer regression passes | Complete installer transaction/recovery is S17 |
| S05 | Exclusive create, Linux no-replace rename, final-link protections and operation-time grant rechecks added | Parent/source stat-to-open races, handle-relative confinement, and non-Linux/unsupported-filesystem rename fallback remain |
| S06 | Same-call timer cancellation removes the action; running timers are aborted on cancel/reload/disable; tests pass | New request types still need the same ownership discipline |
| S07 | Stores allow two memories rather than sixteen; retained log entries are truncated | No aggregate plugin/host retained-byte budget; two foreground plus background memories are still not one 256 MiB total cap |
| S08 | Dense replacements stop expanding after a budget is reached | The budget is checked before one full replacement is expanded, so a single expansion exceeds it; all match locations are still retained. Reproduced below |
| S09 | Install/update commit and reload moved off the editor thread | Plugin create/rename/delete, LSP waits, automatic regular-file reload and shell expansion still include synchronous work |
| S10 | Four pending file batches per plugin, serialization, special-file refusal, per-operation grant checks and explicit cancel-between-operations added | Reload generation/request identity is not checked before each side effect; blocking operations cannot be interrupted; no global queued-byte budget |
| S11 | Disable clears UI decorations/diagnostics/status/sidebar, processes, timers and completion waiters; integration regression passes | Teardown does not cancel the background-worker queue or file batches by their original load generation |
| S12 | Markdown removes closed-document cache entries and configures render settings once | Full-text transfer, line-list comparisons/copies and whole-layer replacements remain |
| S13 | Decoration endpoints use batched mapping; randomized equivalence test passes | Future viewport/delta work remains separate |
| S14 | Per-plugin average/max timing and a yield between subscribers after 50 ms added | Budget is checked after a whole handler and resets per event; completion fan-out, snapshot cost, event queue size and overall UI latency are not bounded by it |
| S15 | Compile concurrency is capped and all-disabled configurations avoid constructing the host | Compilation results still accumulate before installation; each warm cache hit still scans the shared directory; large-component/aggregate limits remain |
| S16 | Reload during startup resolves the waiting directory; regression passes | Deferred opens still lack initiating view/activity identity and can take focus late |
| S17 | Runtime update failure attempts rollback; invalid locks fail closed; git has a deadline | Crash recovery, rollback-failure handling and inter-process transaction/lock serialization remain |
| S18 | Grep stdout/result caps, latest-result generation, cached path checks and ordinary UTF-8 byte-column conversion added; tests pass | Compilation output/pump cancellation remains unbounded; the grep byte-to-char-to-grapheme conversion still mispositions combining text, as below |
| S19 | Oil orders rename dependencies, handles ordinary swaps, deduplicates new directories and evicts closed listings; native/in-editor tests pass | Collision-safe temporary-name reservation and partial-failure/retry coverage should precede treating every rename graph as supported |
| S20 | Same-length edits inside conceal ranges reveal the text; regression passes | No further failure reproduced in the checked mapping cases |
| S21 | CI checks the plugin-disabled build, example/test-guest native tests and component presence | The new loop does not run the separate SDK workspace; presence is not source/artifact reproducibility; release performance gates remain absent |
| S22 | Capability query and clearer linker/version diagnostics added | Minimum-host negotiation, typed status/progress/errors, lifecycle guarantees and broader API contracts are still partial |

## Remaining behavior reproduced during verification

### The preview's claimed 1 MiB cap is a soft threshold

In [`Substitution::apply_limited`](../helix-core/src/substitute.rs), the code
checks `spent >= budget`, then computes the entire next replacement before
adding its length. The live preview uses this function.

A bounded native probe used **1,000,000 `x` characters** and
**`%s/x*/&&/g`**, where `&` expands to the whole match. With a declared budget of
**1,048,576 bytes**, it produced **2,000,000 replacement bytes**. The payload
can overshoot by a whole expansion; match count/vector overhead is separate.
The existing test uses many equal-sized replacements that exactly divide the
budget and does not catch this case.

**Next fix:** enforce a remaining-byte budget during expansion, not after
allocating the replacement. Also bound retained matches/work and show an honest
partial-preview label. Add tests for a single oversized capture expansion and
zero-width/dense matches; preserve final substitution semantics.

### Cancelling background requests leaves queued computation

[`State::cancel_request`](../helix-plugin/src/lib.rs) removes the pending
background id without sending a cancellation signal to its worker. The worker
queue does not recheck that id when starting the next task. Removing pending ids
also frees the admission slots while the queued payloads/work remain.

A bounded probe started and cancelled **12** existing `spin` requests with a
**four-request nominal limit** and a **500 ms worker deadline**. All 12 were
accepted; **two cancelled tasks still executed to timeout**, including work
queued behind the first task. Dropping the host then rejected the remaining
queue, confirming the host-drop fix independently of request cancellation.
No spinning worker was left running after the probe.

**Next fix:** put owner/generation/request cancellation in the queued task,
recheck it before starting, interrupt active CPU work safely, and budget queued
bytes/jobs independently of guest-visible pending callbacks. Reuse this for
disable/reload. This does not require changing the public ABI just to add host
generation guards.

### Grep positions still confuse characters with graphemes

[`quickfix::jump`](../helix-term/src/commands/quickfix.rs) converts a byte column
to a char offset, then passes it to `pos_at_coords`, which counts grapheme
clusters. Those numbers differ for combining marks and multi-scalar emoji.

Evaluating the same conversion on **`e\u{301} needle`**, with the byte column
at `needle`, produces **char offset 4 (`e`)**, rather than the correct **char
offset 3 (`n`)**. This is a native conversion probe, not a full terminal grep
reproduction. The integration test covers precomposed `é` and `✓`, which each
have one scalar per grapheme and therefore do not catch the mismatch.

**Next fix:** for byte-based results, compute the absolute char position from
the line start directly, then enforce grapheme/cursor boundaries as needed.
Test combining marks, emoji/ZWJ, tabs and end-of-line positions through `:grep`.

## Future API status

The additive 0.4 imports are implemented and tested with an older frozen 0.4
component; keeping them in 0.4 follows the repository's import-only policy.

- **F01 is partial:** capability discovery and callback cancellation exist;
  background work cancellation/admission still has the reproduced gap above.
  File effects also need original-load guards. Host-side generation guards can
  be added internally rather than deferring all ownership fixes to API 0.5.
- **F02 has the basic path:** document revisions and call-time conditional
  edits work in the new tests. Deltas, origin subscriptions, coordinated
  multi-document validation and application receipts are not implemented.
  `Action::Edit` does not carry an expected revision to application, so changes
  from preceding queued editor commands need further validation; this was
  code-inspected, not reproduced end to end here.
- **F06's completion path works:** real native snippet tab stops and the kept
  snippet example pass in-editor tests. Direct snippet insertion, variables and
  inline suggestions remain outside the delivered surface.
- **F03–F05, F07–F15 remain proposals:** views/viewport, richer owned-buffer and
  native-command integration, syntax/LSP services, save/action/location-list
  providers, watches, persistent process sessions, richer panels, debugger/VCS
  APIs, folds and URI providers have not appeared in the checked WIT. Existing
  tool/text-buffer workarounds still apply.

Calling the initial delivery fully complete would overstate these contracts.
The separately tracked upstream WASI blocked-filesystem-worker issue also
remains; passing the native auto-reload FIFO test does not establish that a
suspended WASI OS open can be cancelled.

## Validation

| Check on `4bd8f2e9` | Result |
| --- | --- |
| `cargo test -p helix-plugin` | 100 passed; 2 ignored benchmarks |
| `cargo test -p helix-term --lib` | 50 passed |
| Plugin integration, four test threads | 77 passed |
| `cargo test -p helix-view --lib plugin_decorations` | 8 passed |
| Auto-reload integration | 3 passed |
| Quickfix integration | 7 passed |
| Oil native tests | 4 passed |
| Markdown native tests | 3 passed; 1 ignored benchmark |
| Workspace format check | Passed |
| `cargo clippy --workspace --all-targets` | Passed without warnings |
| Normal editor build and scratch tmux checks | Passed: shell expansion refused without a process grant; FIFO replacement left command input responsive |

With the normal debug binary and no plugins, a file-backed session recorded
**0 thread context switches over 3.3 seconds with auto-reload disabled**, versus
**13 with it enabled**. These samples confirm the disabled-path improvement;
they are not release benchmarks, exact wake-up counts or a comparison against
vanilla Helix. Enabled-file polling remains visible in source and the sample.

The full workspace/integration suites, new release benchmarks and non-Linux
platform behavior were not rerun. The three bounded probes above demonstrate
remaining cases despite the passing existing tests. No production fixes, key
bindings, WIT or guest artifacts changed in this verification.
