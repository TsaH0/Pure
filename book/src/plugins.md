# Plugins

Pure runs plugins compiled to WebAssembly, in a sandbox. Nothing about
plugins is built into the editor: it only runs the plugins you install, and
with none installed it does no plugin work at all. Plugins load in the
background, so they never delay startup.

The full guide, including how to write plugins, is
[`helix-plugin/README.md`](https://github.com/TsaH0/helix-editor-custom/blob/master/helix-plugin/README.md).

## Installing a plugin

```
:plugin-install https://github.com/someone/pure-plugin   (or a local directory)
```

Pure fetches it, shows its name, version and permissions, and asks before
installing. It uses the plugin's committed `plugin.wasm` (nothing is built).
`:plugin-update` (all, or named plugins) fetches new versions and asks again
if a plugin wants new permissions; `:plugin-remove <name>` removes one.
Installed plugins are recorded in `~/.config/helix/plugins.lock` with their
commit and a SHA-256 of the component: if the component changes behind
Pure's back, it isn't loaded until you update or reinstall it.

You can also install by hand. Each plugin is a directory in `~/.config/helix/plugins/` holding its
`plugin.toml` and `plugin.wasm`:

```
~/.config/helix/plugins/
└── finder/
    ├── plugin.toml
    └── plugin.wasm
```

Ready-made examples are in
[`helix-plugin/examples`](https://github.com/TsaH0/helix-editor-custom/tree/master/helix-plugin/examples):

| Plugin | Commands |
| --- | --- |
| `text-tools` | `upper`, `lower`, `sort-lines`, `count` |
| `oil` | Edit directories as buffers, like oil.nvim: `-` opens the current file's directory; rename, move, copy, create and delete files by editing lines, then `:w` |
| `lint` | Diagnostics for trailing whitespace, long lines, mixed indentation and conflict markers |
| `emoji` | Completes `:name` to emoji in the completion menu |
| `pomodoro` | A pomodoro timer in the statusline; `start`, `stop` |
| `yank-history` | `list`: pick an earlier yank to paste |
| `finder` | `files`, `grep` (live), `branches` (git) |
| `todos` | Highlights TODO / FIXME / NOTE with signs and a count; `list` |
| `markdown` | Renders Markdown in the buffer (headings, lists, checkboxes, quotes, rules, tables, code fences); `toggle` |

## Using plugins

Run a plugin's command with `:<plugin>/<command> [args]` (or
`:plugin-run <plugin>/<command> [args]`); plugin commands are also in the
command palette (`space ?`) with their docs. Bind them like any command:

```toml
# ~/.config/helix/config.toml
[keys.normal.space]
e = ":oil/open"
"/" = ":finder/grep"
```

| Command | What it does |
| --- | --- |
| `:plugin-run` | Run a plugin command |
| `:plugin-list` | Show the plugins, their permissions, grants and commands |
| `:plugin-log <plugin>` | Show what a plugin logged |
| `:plugin-reload` | Reload all plugins |
| `:plugin-install <url or dir>` | Install a plugin (asks first) |
| `:plugin-update [names]` | Update installed plugins |
| `:plugin-remove <name>` | Remove a plugin (asks first) |

## Decorations

Plugins (API 0.3) can highlight text, add virtual text and set gutter signs
and statusline text; the `todos` example does all four. They can also draw
characters over the text, shown as plain text on the cursor's line while you
edit it; the `markdown` example renders Markdown that way. Signs and
statusline text only show where you ask for them:

```toml
# ~/.config/helix/config.toml
[editor]
gutters = ["diagnostics", "plugins", "line-numbers", "spacer", "diff"]

[editor.statusline]
right = ["plugins", "diagnostics", "selections", "position"]
```

Plugins can come with default key bindings (`[keys]` in their
`plugin.toml`, e.g. `todos` binds `space T`). They only use keys that
neither your config nor Pure binds, so they never change a binding you
have; `:plugin-list` shows them. Turn a plugin's keys off with
`keys = false` in its `plugins.toml` table.

## What plugins can add

Besides commands, pickers, popups and prompts, plugins can show
**diagnostics** (like a language server: gutter, `]d`, `space d`), be
**completion sources** (items join the completion menu), run **timers**,
open **scratch buffers** and buffers they own (`:w` goes to the plugin, as
in `oil`), use **registers**, and rename, create or delete files through
the editor so open buffers follow. Inside a plugin's own buffer its
`[keys.buffer]` bindings apply; nowhere else.

### Editing directories with `oil`

`-` (or `:oil/open`) opens the current file's directory as a buffer. Edit
it like text and `:w`: change a name to rename, delete a line to delete,
add a line to create (`name/` for a directory), cut and paste lines between
directory buffers to move, copy and paste to copy. A list of the changes
asks for confirmation first. In directory buffers `enter` opens, `-` goes
up, `ctrl-p` previews, `g?` lists all keys. It needs
`allow-files = "write"` (and `allow-dirs` for directories outside the
project).

## Settings and permissions

`~/.config/helix/plugins.toml` has one table per plugin. Plugins can only do
what their `plugin.toml` asks for; running programs and touching files also
need your consent here:

```toml
[text-tools]
enabled = false                  # don't load it

[finder]
allow-process = ["rg", "git"]    # programs it may run (or true for any)

[my-notes]
allow-files = "write"            # the workspace: "read" or "write"
width = 80                       # anything else is passed to the plugin

[oil]
allow-files = "write"
allow-dirs = ["~"]               # more directories than the workspace
```

Plugins with the `registers` permission can read your registers (never the
clipboard) and write them.

A plugin that crashes, takes too long (2 s for a command) or uses too much
memory (256 MiB) is disabled until `:plugin-reload`; the editor keeps
running.
