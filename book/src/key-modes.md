## Key modes

Key modes are like Emacs' minor modes: named sets of key bindings that you
turn on and off while editing, in one buffer or in all of them. While a mode
is on, its keys go over all the others: Pure's defaults, your `[keys]` and
plugins' keys. Several can be on at once; the one turned on last wins where
they bind the same key. A buffer's modes show in the statusline (the
`modes` element, in the default layout), like `[Vim]`.

- [Turning modes on and off](#turning-modes-on-and-off)
- [Modes that come with Pure: Vim, Emacs, Ki](#modes-that-come-with-pure-vim-emacs-ki)
- [Writing a mode](#writing-a-mode)
- [Modes from plugins](#modes-from-plugins)

### Turning modes on and off

| Command | |
| --- | --- |
| `:mode NAME` | Toggle a mode in this buffer (everywhere, for a global mode) |
| `:mode NAME on` / `off` | Turn it on / off |
| `:modes` | List every mode, on or off here; Enter toggles one |
| `:describe-mode [NAME]` | The modes on here (or one) and every key each binds, like Emacs' `C-h m` |

Bind one to a key like any command:

```toml
[keys.normal.space]
V = ":mode vim"
```

### Modes that come with Pure: Vim, Emacs, Ki

They are off until you turn them on, and turning one on turns the others
off.

- **`vim`**: Vim's normal, visual and insert keys. Motions move the
  cursor where Vim's do (`w` `W` `e` `E` `b` `B` `ge` `gE` are Vim's own
  word motions, not Helix's), `0 ^ $ G gg H M L % { } f t F T ; * # n N`.
  Every operator (`d c y gu gU g~ > < =`) takes every motion and text
  object, with counts: `dw`, `d3w`, `cw` (to the word's end, as in Vim),
  `dt)`, `ci"`, `da(`, `yi{`, `gUiw`, `>j`, `d%`, `dG`, `dgg`, `dd`, `yy`,
  `cc`. Also `x X s S D C Y ~ J p P u Ctrl-r . q @ v V K ZZ ZQ`, `Ctrl-a`
  `Ctrl-x` on numbers, and the insert keys `Ctrl-h/t/d/n/p/[`. The file's
  header lists the few Vim keys Pure has nothing to bind to (block visual,
  `R`, marks, `gv`).
- **`emacs`**: you stay in insert mode and edit with Ctrl and Alt:
  movement (`C-f/b/n/p`, `C-a/e`, `M-f/b`, `M-</>`, `C-v`/`M-v`,
  `C-l`), killing and yanking (`C-d`, `M-d`, `M-Backspace`, `C-k`, `C-w`,
  `M-w`, `C-y`), `M-u/l/c`, `M-z`, `M-;`, `C-s`/`C-r` search, `C-/`
  and `C-_` undo, `M-x`, `M-g n/p` diagnostics, `C-x C-s` save, `C-x C-w`,
  `C-x C-f`, `C-x b`, `C-x k`, `C-x h`, `C-x u`, `C-x 2/3/o/0/1` windows,
  `C-x C-c` quit, `C-h m` (`:describe-mode`), `C-h k`. `C-space` sets the
  mark: the `emacs-mark` mode is then on (`[Mark]` in the statusline), the
  motions extend the region, and `C-w`, `M-w` or `C-g` end it. Pure's
  cursor is a block, so the region takes the character under it. Esc
  still leaves to normal mode (`i` comes back).
- **`ki`**: the [Ki editor](https://github.com/ki-editor/ki-editor)'s keys
  (Qwerty positions). Pick a selection mode: `a` line, `s` word, `w`
  subword, `W` character, `d` syntax node, `E` paragraph. The same keys
  then move in it: `j`/`l` left/right, `u`/`o` previous/next, `i`/`k`
  up/down (out to the parent node and back in, in node mode), `y`/`p`
  first/last. Act with `f` change, `r` delete, `x` cut, `c` copy, `v`
  paste, `h`/`;` insert before/after, `z`/`Z` undo/redo, `I` join, `L`/`J`
  indent/dedent, `m` jump to a word, `/` flip, Enter save, and Ki's menus
  (`F` case, `n` local, `g` open, `q` history). Each selection mode is a
  mode of its own (`ki-word`, `ki-line`...), so the statusline shows `[Ki
  WORD]`. Ki's held-key layers are [chords](#chords): hold `r` and press a
  movement to delete along it, the same with `x` and `c`. They need a
  terminal that reports key releases (the kitty keyboard protocol); in
  others the keys act alone, at once.

The files are in the runtime's `modes/` directory. To change one, copy it
to `~/.config/pure/modes/` and edit the copy: yours wins.

### Writing a mode

A mode is a table under `[modes]` in `config.toml`, or in any `.toml` file
in `~/.config/pure/modes/` (the same tables, so you can share a file). Its
keys are written as in [`[keys]`](./remapping.md):

```toml
[modes.writing]
description = "Prose: soft wrap and word keys"
lighter = "Write"            # in the statusline; the name by default
global = false               # true: on in every buffer at once
enabled = false              # true: on from the start
languages = ["markdown"]     # on by itself in these languages
conflicts = ["vim"]          # turned off when this one goes on
requires = []                # turned on with this one (and off with them)
on-enable = [":set soft-wrap.enable true"]
on-disable = [":set soft-wrap.enable false"]

[modes.writing.keys.normal]
j = "move_visual_line_down"
k = "move_visual_line_up"
"C-s" = ":write"

[modes.writing.keys.normal.space]
w = ":wrap"                  # menus merge with Pure's: space f still works
```

- A per-buffer mode (`global = false`) is on in the buffer you turn it on
  in. `languages` and `enabled` turn it on in buffers by themselves; `:mode
  NAME off` turns it off in one of them.
- A global mode is on everywhere, new buffers included.
- `on-enable` and `on-disable` are commands, written as `[keys]` values.
  A mode can switch others, as `ki` does: `s = ":mode ki-word on"`.
- Key menus merge: a mode binding `g q` keeps the rest of Pure's `g` menu.
  Bind a key to `no_op` to take it away while the mode is on.
- A sequence can hold a command that waits for a key (`find_till_char`,
  `select_textobject_inner`, `replace`, `select_register`...): the rest of
  the sequence runs after the key is typed, so `t = ["find_till_char",
  "delete_selection"]` under `d` is Vim's `dt<char>`. A key macro right
  after it types that key for it: `["select_textobject_inner", "@w",
  "yank"]` yanks the word.
- A mode in `~/.config/pure/modes/` replaces the runtime's of the same
  name, and one in `config.toml` replaces both. `:config-reload` reloads
  them; modes that were on stay on.

### Chords

A mode can have [chords](./editor.md#editorchords-section), a key pressed
while another is held, which run only while it is on:

```toml
[modes.ki.chords.normal]
"r+l" = ["move_next_word_start", "delete_selection"]
```

They are written as `[editor.chords]` are. Pure asks the terminal for key
releases only while such a mode is on.

### Modes from plugins

Plugins define modes with the plugin API's `key-modes` interface (the same
fields as above) and turn them on and off; a plugin's modes go when it is
disabled. See [Plugins](./plugins.md).
