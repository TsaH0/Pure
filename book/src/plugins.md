# Plugins

Pure runs plugins compiled to WebAssembly, in a sandbox. Nothing about
plugins is built into the editor: it only runs the plugins you install, and
with none installed it does no plugin work at all. Plugins load in the
background, so they never delay startup.

The full guide, including how to write plugins, is
[`helix-plugin/README.md`](https://github.com/TsaH0/helix-editor-custom/blob/master/helix-plugin/README.md).

## Installing a plugin

Each plugin is a directory in `~/.config/helix/plugins/` holding its
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

## Using plugins

Run a plugin's command with `:plugin-run <plugin>/<command> [args]`, or bind
it:

```toml
# ~/.config/helix/config.toml
[keys.normal.space]
e = ":plugin-run yazi/open"
"/" = ":plugin-run finder/grep"
```

| Command | What it does |
| --- | --- |
| `:plugin-run` | Run a plugin command |
| `:plugin-list` | Show the plugins, their permissions, grants and commands |
| `:plugin-log <plugin>` | Show what a plugin logged |
| `:plugin-reload` | Reload all plugins |

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
