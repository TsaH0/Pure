# {{project-name}}

{{description}}

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/{{crate_name}}.wasm plugin.wasm
```

Commit `plugin.wasm` with the source: `:plugin-install <this repo's git URL>`
uses it (nothing is built on install). To try it locally,
`:plugin-install /path/to/this/directory`.
