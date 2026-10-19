# Installing Pure

Pure is installed by [building it from source](./building-from-source.md):

```sh
cargo install --path helix-term --locked
```

installs the `pure` binary.

Note that:

- The [package managers](./package-managers.md) and pre-built binaries below
  install upstream Helix, without Pure's additions.

- To take full advantage of Pure, install the language servers for your
  preferred programming languages. See the
  [wiki](https://github.com/helix-editor/helix/wiki/Language-Server-Configurations)
  for instructions.

## Pre-built binaries

Upstream Helix (without Pure's additions) publishes pre-built binaries on its [GitHub Releases page](https://github.com/helix-editor/helix/releases).
The tarball contents include an `hx` binary and a `runtime` directory.
To set them up:

1. Add the `hx` binary to your system's `$PATH` to allow it to be used from the command line.
2. Copy the `runtime` directory to a location that `hx` searches for runtime files. A typical location on Linux/macOS is `~/.config/helix/runtime`.

To see the runtime directories that `pure` searches, run `pure --health`. If necessary, you can override the default runtime location by setting the `HELIX_RUNTIME` environment variable.
