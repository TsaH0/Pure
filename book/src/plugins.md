# Plugins

Pure runs plugins compiled to WebAssembly, in a sandbox. Nothing about
plugins is built into the editor: it only runs the plugins you install, and
with none installed it does no plugin work at all. Plugins load in the
background, so they never delay startup.

The full guide, including how to write plugins, is
[`helix-plugin/README.md`](https://github.com/TsaH0/helix-editor-custom/blob/master/helix-plugin/README.md). For coding agents, copy
[`helix-plugin/AGENTS.md`](https://github.com/TsaH0/Pure/blob/master/helix-plugin/AGENTS.md)
into your plugin project or `~/.config/helix/`: it has everything an agent
needs to build, test and benchmark a plugin.

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
| `tree` | A file tree in a sidebar, like nvim-tree: `space B` shows it; `enter` opens files and folders |
| `finder` | `files`, `grep` (live), `branches` (git) |
| `todos` | Highlights TODO / FIXME / NOTE with signs and a count; `list` |
| `competitest` | Like competitest.nvim: `space u r` compiles and runs the current file on its testcases and shows the verdicts in a panel below; `space u a` adds a testcase; `space u t` receives testcases from the Competitive Companion browser extension (`:competitest/receive problem` or `contest` makes new files) |
| `outline` | Like aerial.nvim: `space O` shows the file's functions, types and headings in a panel on the right |
| `harpoon` | Like harpoon: mark files with `space +`, jump with `space 1`–`4`, edit the list with `space =` |
| `git-blame` | Like git-blame.nvim: who changed the cursor's line, when and why, at its end |
| `illuminate` | Like vim-illuminate: highlights the other uses of the word under the cursor |
| `snippets` | Snippets with tab stops in the completion menu (`fn`, `test`, `for`...); add your own in `plugins.toml` |
| `markdown` | Renders Markdown in the buffer (headings, lists, checkboxes, quotes, rules, tables, code fences, hidden `**`/backticks/link URLs); `toggle` |

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

## Panels

Plugins can show lists in panels docked left, right or below the views (a
file tree, an outline, test results). `:panel-focus left`, `right` or
`bottom` gives a panel the keys (`j`/`k` move, `enter` runs the item, `esc`
gives the keys back); `:panel-close` closes one.

## Decorations

Plugins (API 0.3 and later) can highlight text, add virtual text and set gutter signs
and statusline text; the `todos` example does all four. They can also draw
characters over the text and hide markup (`**`, link URLs), shown as plain
text on the cursor's line and while inserting or selecting, like
render-markdown.nvim; the `markdown` example renders Markdown that way.
Hidden text is still in the document: motions, search and edits see it.
Signs and
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
show a **sidebar** (a file tree, an outline: `j`/`k` and `enter`
in it, `esc` back to the editor, `q` closes it), open **scratch buffers**
and buffers they own (`:w` goes to the plugin, as
in `oil`), use **registers**, and rename, create or delete files through
the editor so open buffers follow. They can read a document's **syntax
tree** (tree-sitter queries, the node under the cursor), which is how the
`outline` example finds symbols. Inside a plugin's own buffer its
`[keys.buffer]` bindings apply; nowhere else.

### Editing directories with `oil`

`-` (or `:oil/open`) opens the current file's directory as a buffer. Edit
it like text and `:w`: change a name to rename, delete a line to delete,
add a line to create (`name/` for a directory), cut and paste lines between
directory buffers to move, copy and paste to copy. A list of the changes
asks for confirmation first. In directory buffers `enter` opens, `-` goes
up, `ctrl-p` previews, `g?` lists all keys. With `trash = true` in its
`plugins.toml` table, deleted entries go to the system's trash. If a change
fails, the buffer keeps your edits; fix it and `:w` again. It needs
`allow-files = "write"` (and `allow-dirs` for directories outside the
project). To have `pure some/dir` and `:open some/dir` open oil instead of
the file picker:

```toml
# ~/.config/helix/config.toml
[editor]
directory-handler = "oil/open"
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

[oil]
allow-files = "write"
allow-dirs = ["~"]               # more directories than the workspace

[competitest]
allow-listen = [27121]           # local ports it may receive requests on

[my-keys]
allow-key-override = true        # its keys may take precedence over yours

[my-lsp-tool]
allow-lsp = true                 # it may send requests to language servers
```

Plugins that declare their settings have them checked when they load: a
misspelled name or a wrong type is an error that names the setting. After
you change only a plugin's settings, `:config-reload` passes them on
without reloading it; changing what it is allowed (`allow-…`), its keys or
`enabled` reloads the plugins.

`allow-listen` lets a plugin receive requests from programs on your
machine, such as the Competitive Companion browser extension. It listens
on 127.0.0.1 only and accepts only JSON requests addressed to `localhost`,
so web pages can't reach it.

Plugins with the `registers` permission can read your registers (never the
clipboard) and write them.

A plugin that runs editor commands can only run those that start programs
(`:sh`, `:pipe`, `!`, ...) with `allow-process = true`.

A plugin that crashes, takes too long (2 s for a command) or uses too much
memory (256 MiB) is disabled until `:plugin-reload`; the editor keeps
running.

When a plugin misbehaves, `:plugin-doctor` shows each plugin's state, what
it asked for and what you granted (and what is missing), its keys and
their conflicts, its settings and how long its calls take.
`:plugin-trace <plugin>` logs each of its calls with its time (see
`:plugin-log`); `:plugin-trace off` stops.
