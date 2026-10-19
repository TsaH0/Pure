# Plugin roadmap progress

## Status

- Done: M0.1 (versioned WIT layout).
- In progress: M0.2 (two host worlds), on `plugin-api-0.2`.
- Next: M0.3–M0.5, then M2.1 and the remaining authorized milestones in the requested order.
- Awaiting design review: M5, M6.2, M6.3, M7, M8 and the file-manager sidebar. No implementation of these is authorized.

## Open questions for the owner

- An optional first-use process prompt remains undecided. M1.1 will implement explicit `allow-process` grants and leave a prompt hook.

## Environment and baseline (2026-10-19)

- Installed pinned Rust 1.90.0 with rustfmt, rust-src, clippy, rust-analyzer and `wasm32-wasip2`; Wasmtime remains 41.0.4.
- Development build and Rust/TOML parser and query health checks passed. Ten representative grammars are available; no key bindings changed.
- Rebuilding the existing test plugin for `wasm32-wasip2` passed.
- The initial workspace run passed 185 core unit tests and four indentation tests, then found three loader workspace-trust failures caused by unwritable default data paths. All 12 loader tests pass after redirecting XDG config, cache and data under `/workspace`.
- Full editor integration baseline and milestone checks are in progress. The roadmap's known-failure list is the comparison baseline; new failures will be investigated.
- Reusable installation and startup instructions are saved in the cloud environment draft. Publication is a separate user action in environment settings.

## M0.1 Versioned WIT layout (done, 2026-10-19, PR pending)

- Preserved the 0.1 WIT contents exactly at `helix-plugin/wit/v0.1/helix-plugin.wit` and updated host and SDK binding paths.
- Added the designed 0.2 world, including request ids, open actions, processes, pickers and callback exports, in `wit/v0.2/`.
- Both 0.1 fixtures rebuilt successfully; `cargo fmt --all` and `cargo clippy --workspace --all-targets` passed without warnings. All 32 existing host tests pass with the rebuilt fixtures. Editor library and plugin integration checks are still running and will be recorded before the milestone PR.
