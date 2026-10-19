#!/bin/sh
# Rebuild the test plugin (into tests/fixtures) and the example plugin (into
# its own directory, ready to be copied into ~/.config/helix/plugins). The
# tests load both. Needs the wasm32-wasip2 target:
#   rustup target add wasm32-wasip2
set -eu
cd "$(dirname "$0")"
build() {
    (cd "$1" && cargo build --locked --release --target wasm32-wasip2)
    cp "$1/target/wasm32-wasip2/release/$2.wasm" "tests/fixtures/$3.wasm"
}
build tests/plugins/testplugin testplugin testplugin
build tests/plugins/testplugin2 testplugin2 testplugin2
(cd examples/text-tools && cargo build --locked --release --target wasm32-wasip2)
cp examples/text-tools/target/wasm32-wasip2/release/text_tools.wasm examples/text-tools/plugin.wasm
