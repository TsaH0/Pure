# Installing Pure from crates.io

```sh
cargo install pure-editor --locked
pure --version
```

The executable is named `pure`, not `pure-editor`. Rust 1.90 or newer and a C
compiler/linker are required. Default features include Git integration and the
WebAssembly plugin host. This is the Pure fork, not upstream Helix.

Themes, queries, language definitions and the tutor are bundled. On first use,
the portable runtime is atomically unpacked into a content-addressed directory
under Pure's cache (`~/.cache/helix/runtime/` on typical Linux systems). Explicit
runtime paths and `~/.config/helix/runtime` take precedence. A source checkout
and a runtime symlink are not required.

Native tree-sitter parser binaries are platform-specific and are not bundled:

```sh
pure --grammar fetch
pure --grammar build
```

These commands need Git, a C/C++ compiler, and network access to the grammar
repositories. To restrict downloads, set `use-grammars` in your
`~/.config/helix/languages.toml`. Alternatively, compile parsers during install:

```sh
cargo install pure-editor --locked --features build-grammars
```

That opt-in build can take significantly longer and fetches grammar sources.
The default Cargo build does not fetch them.

## Configuration and updates

Pure retains Helix's config paths (`~/.config/helix/` on Linux), including
`config.toml`, `languages.toml`, `plugins.toml` and `plugins/`. Installing or
updating Pure does not replace configuration or install personal plugins.

```sh
cargo install pure-editor --locked --force
```

If replacing a manually copied executable, Cargo may require `--force` for the
initial install too. Keep a backup before replacing a locally modified build.

## Maintainer release checklist

The editor is `pure-editor`; its internal libraries and vendored syntax forks
use `pure-editor-*` names. Their Rust library names and internal import aliases
remain unchanged. These are implementation libraries, not a stable editor SDK.
The separate WASM plugin SDK is not published as part of this release.

1. Update the workspace version and all exact internal dependency versions,
   including the independently versioned LSP types and vendored syntax crates.
2. Update `Cargo.lock`; run `cargo fmt --all --check` and the affected tests.
3. Run `python3 scripts/check-crates.py`. It packages every crate and compiles
   the editor from the **extracted archives**, with internal dependencies patched
   only to those archives. This catches cross-workspace assets and path mistakes
   before any public uploads.
4. Build and smoke-test the editor with an empty config/cache. Test the runtime
   unpacking and plugin input regressions.
5. Commit the release changes, then run `python3 scripts/publish-crates.py`.
   Cargo uses its configured credentials. The script publishes in non-dev
   dependency order and can resume a partially published release.
6. Verify with `cargo install pure-editor --locked --root <temporary-directory>`
   and a clean config/cache. Then push the release commit and tag.

The package-local runtime/language/theme/icon paths are symlinks to canonical
workspace assets. Cargo dereferences them into ordinary files in each archive;
no symlink or external checkout is needed by registry consumers. Dev-only
path dependencies without registry versions are intentionally omitted by Cargo
from published manifests, avoiding the view/TUI test dependency cycle.
