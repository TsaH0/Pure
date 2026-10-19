<div align="center">

<h1>Pure</h1>

<p><em>“Immortal enlightenment is repentant enlightenment, perhaps only by making
a mountain using salt is fastest way to reach heavens.”</em></p>

</div>

Pure is a modal text editor, a fork of [Helix](https://github.com/helix-editor/helix)
(a [Kakoune](https://github.com/mawww/kakoune) / [Neovim](https://github.com/neovim/neovim)
inspired editor, written in Rust) that keeps up with Helix and adds:

- **WebAssembly plugins**: sandboxed plugins in any language that targets WASM
  components, with permissions, time and memory limits. See
  [`helix-plugin/README.md`](helix-plugin/README.md).
- **Vim's `:s`**: `:%s/\(.*\)/"\1",/`, `:s/(\w+) (\w+)/\U\2\E \1/g`, ranges,
  capture groups (Vim and regex syntax), case changes and flags; `space v`
  starts a very magic `:s/\v` on the selection.
- **Quickfix list**: filled from compiler output, `:grep`, diagnostics or a
  buffer; `]q` / `[q` / `]Q` / `[Q`, `space q`, `:cnext` and friends.
- **Running commands**: `space x` streams a command's output into an
  Emacs-style `*compilation*` buffer, `space z` runs it like Vim's `:!`,
  `:run-interactive` hands the terminal to full-screen programs, and a
  built-in `yazi` file picker.
- **Smarter text objects**: `mi(`/`ma"` find the nearest pair on the line,
  and letters work for pairs (`mib`, `miB`, `mir`, `miA`, `mid`, `mis`, `miq`).

Run it with `pure` (`hx` still works). Configuration lives where Helix keeps
it, in `~/.config/helix/`.

# Features (from Helix)

- Vim-like modal editing
- Multiple selections
- Built-in language server support
- Smart, incremental syntax highlighting and code editing via tree-sitter

Note: Only certain languages have indentation definitions at the moment. Check
`runtime/queries/<lang>/` for `indents.scm`.

# Installation

```sh
cargo install --path helix-term --locked
```

installs `pure` and `hx`. See Helix's
[installation documentation](https://docs.helix-editor.com/install.html) for
the runtime files and grammars.

# Documentation

Pure's own additions are documented in the [book](./book/src) (keymap, text
objects, commands) and in [`helix-plugin/README.md`](helix-plugin/README.md).
Everything else works as described in the
[Helix documentation](https://docs.helix-editor.com/).

# Credits

Pure is built on [Helix](https://helix-editor.com) by Blaž Hrastnik and its
contributors. Thanks to [@jakenvac](https://github.com/jakenvac) for designing
the Helix logo.
