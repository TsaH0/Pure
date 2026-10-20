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
| `yazi` | `open`: pick files with yazi |
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
e = ":yazi/open"
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
and statusline text; the `todos` example does all four. Signs and statusline
text only show where you ask for them:

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
```

A plugin that crashes, takes too long (2 s for a command) or uses too much
memory (256 MiB) is disabled until `:plugin-reload`; the editor keeps
running.
