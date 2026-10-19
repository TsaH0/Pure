# Plugin roadmap progress

## Status

- Done: M0.1 (versioned WIT layout), M0.2 (two host worlds), M0.3 (request ids and callbacks), M0.4 (SDK 0.2).
- In progress: M0.5 (0.2 test plugin), on `plugin-api-0.2`.
- Next: M2.1, then M2.1 and the remaining authorized milestones in the requested order.
- Awaiting design review: M5, M6.2, M6.3, M7, M8 and the file-manager sidebar. No implementation of these is authorized.

## Open questions for the owner

- An optional first-use process prompt remains undecided. M1.1 will implement explicit `allow-process` grants and leave a prompt hook.

## Environment and baseline (2026-10-19)

- Installed pinned Rust 1.90.0 with rustfmt, rust-src, clippy, rust-analyzer and `wasm32-wasip2`; Wasmtime remains 41.0.4.
- Development build and Rust/TOML parser and query health checks passed. Ten representative grammars are available; no key bindings changed.
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
