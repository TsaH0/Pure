#!/bin/sh
# Rebuild the test plugins (into tests/fixtures) and the example plugins (each
# into its own directory, ready to be copied into ~/.config/helix/plugins). The
# tests load them. Needs the wasm32-wasip2 target:
#   rustup target add wasm32-wasip2
# Version-suffixed compatibility components are frozen. Never overwrite them
# here: API 0.1/0.2 tests intentionally run components built before SDK changes.
set -eu
cd "$(dirname "$0")"
# Keep machine-specific paths out of the committed components.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$(pwd)=helix-plugin --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=cargo --remap-path-prefix=${RUSTUP_HOME:-$HOME/.rustup}=rustup"
build() {
    (cd "$1" && cargo build --locked --release --target wasm32-wasip2)
    cp "$1/target/wasm32-wasip2/release/$2.wasm" "tests/fixtures/$3.wasm"
}
build tests/plugins/testplugin testplugin testplugin
build tests/plugins/testplugin2 testplugin2 testplugin2
# Every example, into its own directory.
for example in examples/*/; do
    name=$(basename "$example")
    (cd "$example" && cargo build --locked --release --target wasm32-wasip2)
    cp "$example/target/wasm32-wasip2/release/$(echo "$name" | tr - _).wasm" "$example/plugin.wasm"
done
