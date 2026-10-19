# Plugin roadmap progress

## Status

- Done: M0.1 (versioned WIT layout), M0.2 (two host worlds), M0.3 (request ids and callbacks), M0.4 (SDK 0.2), M0.5 (0.2 test plugin).
- M0 complete and pushed to `plugin-api-0.2`; 216/216 full integration tests pass. PR creation is blocked by GitHub API egress.
- Done independently: M2.1 on `plugin-pickers` (commit `08fe41bf`); it will be rebased onto M1 for the single M2 PR.
- Done: M1.1–M1.2 on `plugin-processes`, based on completed M0.
- In progress: M1.3 terminal hand-over.
- Awaiting design review: M5, M6.2, M6.3, M7, M8 and the file-manager sidebar. No implementation of these is authorized.

## Open questions for the owner

- API 0.1 `run-commands` can invoke existing shell/write commands. Asked whether those plugin calls should also require new process/filesystem grants, or retain their existing trusted semantics; user commands and key bindings stay unchanged either way. Direct process imports already require both permission and grant.
- An optional first-use process prompt remains undecided. M1.1 implements explicit `allow-process` grants and leaves the optional prompt hook unimplemented.

## Environment and baseline (2026-10-19)

- Installed pinned Rust 1.90.0 with rustfmt, rust-src, clippy, rust-analyzer and `wasm32-wasip2`; Wasmtime remains 41.0.4.
- Development build and Rust/TOML parser and query health checks passed. Twelve representative grammars are available; no key bindings changed.
- Rebuilding the existing test plugin for `wasm32-wasip2` passed.
- The initial workspace run passed 185 core unit tests and four indentation tests, then found three loader workspace-trust failures caused by unwritable default data paths. All 12 loader tests pass after redirecting XDG config, cache and data under `/workspace`.
- The full baseline with four test threads aborted with SIGABRT, before any 0.2 host changes. All eight movement tests around the abort pass in isolation. A serial full baseline is running to obtain complete results. The roadmap's known-failure list is the comparison baseline; new failures will be investigated.
- Reusable installation and startup instructions are saved in the cloud environment draft. Publication is a separate user action in environment settings.

## M0.1 Versioned WIT layout (done, 2026-10-19, PR pending)

- Preserved the 0.1 WIT contents exactly at `helix-plugin/wit/v0.1/helix-plugin.wit` and updated host and SDK binding paths.
- Added the designed 0.2 world, including request ids, open actions, processes, pickers and callback exports, in `wit/v0.2/`.
- Both 0.1 fixtures rebuilt successfully; `cargo fmt --all` and `cargo clippy --workspace --all-targets` passed without warnings. All 32 existing host tests pass with the rebuilt fixtures. Editor library tests (21) and plugin integration tests (18) also passed.


## M0.2 Two worlds in the host (done, 2026-10-19, PR pending)

- `helix-plugin/src/api.rs` contains separate Wasmtime bindings for 0.1 and 0.2, thin adapters to the shared `State` methods, and the versioned instance enum. Manifest validation explicitly names both supported versions.
- Added a minimal 0.2 fixture and its reproducible build step, needed to validate the second world. M0.5 will expand it and migrate it to the 0.2 SDK.
- Tests prove both versions load together, 0.2 reads/edits/selections/mode events use shared state, a mismatched manifest fails, and unsupported versions name the supported APIs.
- `cargo check`, formatting, workspace Clippy (no warnings), all 34 host tests and 21 editor library tests passed. All 18 plugin integration tests passed too.
- Corrected the draft 0.2 WIT's callback parameter spelling to `%result`, as required by the WIT parser. API 0.1 remains unchanged. Process and picker imports explicitly report unsupported capabilities until their milestones implement them.


## M0.3 Request ids and callbacks (done, 2026-10-19, PR pending)

- Added typed pending requests, `PluginHost::register_request` for host adapters, and `PluginHost::callback` with fresh snapshots, command deadlines, kind validation and one-shot delivery. Traps clear pending requests; failed commands roll back requests reserved during that call.
- Explicit init/command/event/callback contexts guard UI and editor-command imports. Both commands and callbacks may request UI; init and events are refused.
- Request allocation uses a process-wide checked counter shared by stores, rather than resetting a counter per instance. This prevents a late result from matching a new request after host recreation or reload; exhaustion returns an error instead of wrapping.
- The planned `PromptDone` variant is reserved. The supplied 0.2 WIT has no prompt export, so prompt reservation explicitly fails; M3 needs a new versioned interface rather than modifying the existing 0.2 ABI.
- Added tests for process/picker delivery, wrong-kind/unknown/duplicate ids, clear/recreation, legacy refusal and all four UI contexts. All 38 host tests, all 21 editor library tests and workspace Clippy (no warnings) pass. The first plugin integration rerun overlapped changing text-tools to API 0.2 and saw a transient manifest/component mismatch (13 passed, 5 failed); rerunning after rebuilding the matching component. No assertions were changed.


## M0.4 SDK 0.2 (done, 2026-10-19, PR pending)

- SDK 0.2 generates the new world by default; the `api-v0-1` feature preserves the original ABI and source-compatible trait. The original test plugin uses that feature. Archived the unchanged 0.1 text-tools component and run all its behavior assertions against both 0.1 and the rebuilt 0.2 example.
- Added one-shot, thread-local callback routing with reentrant chaining, unknown/duplicate/wrong-kind protection and failed-command cleanup. `register_plugin!` exports callbacks automatically. Added documented spawn/interactive options, picker choices/items and closure helpers.
- text-tools source is unchanged; its manifest now selects 0.2. All rebuilt artifacts and updated guest lockfiles are included. Fixture builds now use `--locked`.
- Both SDK unit tests, its generated export doctest, SDK Clippy with `-D warnings`, SDK legacy-feature build/doctest, 39 host tests, 21 editor library tests, all 18 plugin integration tests, formatting and workspace Clippy pass. The pre-existing illustrative SDK snippet remains ignored as before.
- Process execution and picker display remain explicit unsupported errors until M1/M2; README and design status distinguish bindings from implemented capabilities. The transient M0.3 integration artifact mismatch is resolved by the final 18/18 pass.


## M0.5 0.2 test plugin (done, 2026-10-19, PR pending)

- Migrated `tests/plugins/testplugin2` to SDK 0.2 and automatic callback exports. Commands cover shared buffer functions and the process, interactive, picker, push-items and kill imports, ready to expand as the corresponding abilities land.
- Added real-Wasm tests for callback snapshots, queued editor commands, SDK error reporting, callback time limits and pending cleanup, plus UI refusal from event handlers. Added an editor integration test for 0.1 and 0.2 plugins coexisting, Unicode edits and undo.
- Formatting, workspace Clippy (no warnings), all 42 host tests, all 21 editor library tests and all 19 plugin integration tests pass. Rebuilt and included the wasm fixture. The new integration test explicitly includes its expected trailing newline in the input; no assertions were weakened.
- The old serial baseline completed 215 tests: 209 passed, one known HTML/JavaScript injection failure, and five old-host/new-example mismatches because it read the evolving fixture files. Those fixture mismatches are resolved in the current targeted suite. Added HTML/JavaScript grammars to remove the known missing-parser prerequisite; parser/query health checks pass.
- A fresh full current suite is running with four test threads and a 16 MiB Rust thread-stack diagnostic after the untouched baseline's SIGABRT. A prior exploratory run was interrupted to recompile the corrected test input; it is not counted as validation.
- PR creation is externally blocked: the HTTPS proxy rejects CONNECT to `api.github.com` with 403. Added that domain to the saved environment draft; review/save it in environment settings to apply access. No credential values were requested or stored.


## M0 milestone validation (2026-10-19)

- Full integration suite: **216 passed, 0 failed, 0 ignored**, `--test-threads 4`, with `RUST_MIN_STACK=16777216` (197.46 s). This includes the existing quickfix, Vim substitution, default keys and both plugin API versions. No known-failure exemptions were needed after installing the required HTML/JavaScript grammars.
- The larger Rust thread stack resolves the original baseline abort in this environment; it is captured in the activation script and saved startup instructions. Tests and assertions remain enabled and unchanged except for new coverage.
- Host: 42 passed. Editor library: 21 passed. Plugin integrations: 19 passed. SDK: two unit tests and one generated export doctest passed; one existing illustrative snippet remains ignored. Workspace and standalone SDK Clippy passed without new warnings; formatting passed; wasm fixtures rebuilt.
- All five M0 tasks are committed and the branch was successfully pushed using the platform Git proxy. PR draft text is prepared at `/workspace/m0-pr.md`; API access is separately blocked as described above.


## M1.1 Process permission and explicit grant (done, 2026-10-19, M1 PR pending)

- Added manifest `process` permission and user grants (`true`, `false`, or exact program lists), parsed before init and removed from plugin JSON settings. Missing grants default to deny; invalid grants skip only the affected plugin.
- Host process imports enforce both checks. List and health diagnostics report grant status without instantiating components for health checks. The optional prompt hook is left unimplemented as requested.
- Formatting, workspace Clippy (no warnings), all 45 host tests, all 23 editor library tests and all 20 plugin integration tests passed. Added checks for default denial, malformed grants, exact program paths, both permission layers, settings stripping and diagnostics. A development build and real CLI `--health` request also passed using a manifest without a component, confirming health does not run plugin code.
- Guest code did not change; no fixture rebuild was necessary for this task.


## M1.2 Background processes and cancellation (done, 2026-10-19, M1 PR pending)

- Added queued spawn/kill actions, 16-request quotas, checked ownership and timeout defaults/caps. Requests start direct argv commands on Tokio tasks, write/close stdin concurrently with bounded output capture, and deliver callbacks through the editor job queue with access to jobs.
- Timeout covers inherited pipes even after the parent exits. Unix cancellation kills the owned process group; children are reaped. Output remains valid UTF-8 and within the byte cap after lossy decoding. Reload and disable cancel owned processes; reload drops old authorization before creating a new host.
- Added a small `job.rs` callback hook so callbacks can schedule editor commands. Plugin-specific execution remains in `plugin_processes.rs`. Nested plugin action application restores the previous event-suppression state.
- Rebuilt the 0.2 fixture with stdin, timeout, quota, callback edit/command and failure commands. Added real-process tests and in-editor callback tests. Initial new integration inputs needed the existing `--` flag separator and `<gt>` key notation; inputs were corrected without changing command parsing or assertions.
- Final validation: all 49 host tests, all 30 editor library tests (including seven executor tests), all 26 plugin integration tests, formatting and workspace Clippy passed without new warnings. New integration coverage includes output/exit codes, stdin, callback edits/undo, editor commands, timeouts, kill, spawn errors, grant refusal, reload/disable, and nested event suppression. Rebuilt fixture committed.
