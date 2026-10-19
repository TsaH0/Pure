# API 0.3 trial plugin

Kept source and built component for exercising the SDK. Pure does not install
it automatically. Copy `plugin.toml` and `plugin.wasm` into your plugin directory.

- `:plugin-run api-demo/popup centered`: centered popup, status on close.
- `:plugin-run api-demo/close <id>`: close an owned popup.
- `:plugin-run api-demo/live`: query text and generation in a live picker.
- `update <generation> <rows...>` / `push <generation> <rows...>`: query updates.
- `rollback <generation> <rows...>`: intentionally fail after a push.
- `:plugin-run api-demo/stream sh -c "printf 'line\ntail'"`: batches and final tail.
- `:plugin-run api-demo/open-at <path> vsplit 2 3`: native positioned open.

Streaming needs `[api-demo] allow-process = ["sh"]` in `plugins.toml`.
This is a trust grant: a shell can run arbitrary programs with your rights.
Subscribed command, focus and idle events are visible with `:plugin-log api-demo`.
No bindings are installed. Rebuild using `helix-plugin/build-plugins.sh`.
