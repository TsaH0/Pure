# Plugins

Pure runs plugins compiled to WebAssembly, in a sandbox. Nothing about
plugins is built into the editor: it only runs the plugins you install, and
with none installed it does no plugin work at all. Plugins load in the
background, so they never delay startup.

The full guide, including how to write plugins, is
[`helix-plugin/README.md`](https://github.com/TsaH0/helix-editor-custom/blob/master/helix-plugin/README.md). For coding agents, copy
[`helix-plugin/AGENTS.md`](https://github.com/TsaH0/Pure/blob/master/helix-plugin/AGENTS.md)
into your plugin project or `~/.config/pure/`: it has everything an agent
needs to build, test and benchmark a plugin.

## Installing a plugin

```
:plugin-install https://github.com/someone/pure-plugin   (or a local directory)
```

Pure fetches it, shows its name, version and permissions, and asks before
installing. It uses the plugin's committed `plugin.wasm` (nothing is built).
`:plugin-update` (all, or named plugins) fetches new versions and asks again
if a plugin wants new permissions; `:plugin-remove <name>` removes one.
Installed plugins are recorded in `~/.config/pure/plugins.lock` with their
commit and a SHA-256 of the component: if the component changes behind
Pure's back, it isn't loaded until you update or reinstall it.

You can also install by hand. Each plugin is a directory in `~/.config/pure/plugins/` holding its
`plugin.toml` and `plugin.wasm`:

```
~/.config/pure/plugins/
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
| `find-char` | `f`, `F`, `t`, `T` like Vim's: on the cursor's line, optional smart case; `repeat` and `repeat-reverse` (bind them to `;` and `,` if you like) |
| `vim-registers` | Vim's numbered registers (`"0`, `"1`–`"9`, `"-`), done by a plugin; the same as `editor.registers = "vim"` |
| `script` | Like `init.lua`: script the editor in [Rhai](https://rhai.rs) from `plugins.toml` or `init.rhai` in the config directory: define commands, bind keys, react to events; `:script/eval <code>` runs a line (see below) |
| `markdown` | Renders Markdown in the buffer (headings, lists, checkboxes, quotes, rules, tables, code fences, hidden `**`/backticks/link URLs); `toggle` |

## Using plugins

Run a plugin's command with `:<plugin>/<command> [args]` (or
`:plugin-run <plugin>/<command> [args]`); plugin commands are also in the
command palette (`space ?`) with their docs. Bind them like any command:

```toml
# ~/.config/pure/config.toml
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
# ~/.config/pure/config.toml
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

### Changing built-in behaviour

Plugins can replace what Pure's own keys and commands do, without changing
Pure: they can bind any key above the defaults (`find-char` replaces
`f`/`F`/`t`/`T`), take the key typed after theirs, run commands or play
keys as if typed (like `:normal`), and see each command before it runs,
with its register and count, to add to it or cancel it (`vim-registers`).
A plugin can take over a whole menu key (`g`, `z`, `Z`, `space`...) too,
unless you have bound something in that menu yourself: then your menu
stays.

Code can be folded, like Vim: `zc` (or `zf`) folds the selected lines, or
the syntax node around the cursor (a function, a block; the indented lines
below it where there is no syntax tree), `zo` unfolds, `za` toggles, `zM`
folds every top-level block and `zR` unfolds everything. A fold shows as
its first line followed by `⋯ N lines` (the `ui.virtual.fold` scope, or
`comment`); `j` and `k` step over it, edits move it, and a jump or search
into it opens it. Plugins can fold too.

Splits can be resized: `C-w >` and `C-w <` widen and narrow the split,
`C-w +` and `C-w -` make it taller and shorter (with a count, by that many
columns or rows), `C-w =` makes them all the same size again. Plugins can
open, resize, close and focus splits, choose their gutters, and draw their
own buffers entirely (canvases).

`:normal keys` (`:norm`) plays keys as if typed, like Vim's:
`:normal dd`, `:normal ihello<esc>`.

### Scripting with `script`

For small customisations without writing a plugin, the `script` plugin runs
a [Rhai](https://rhai.rs) script (a small Rust-like language) at startup:

```toml
# plugins.toml
[script]
source = '''
define("upper", "Uppercase the selections", "upper");   // :script/upper
bind("normal", "space u", "upper");
on("save", "saved");
fn upper() { replace_selections(selected().map(|s| s.to_upper())); }
fn saved() { status(`saved ${path()}`); }
'''
```

Or put the script in `init.rhai` in the config directory and let the
plugin read it (`allow-files = "read"`, `allow-dirs = ["~/.config/pure"]`).
Scripts can use `text()`, `path()`, `language()`, `mode()`,
`selections()`, `set_selections()`, `selected()`, `replace_selections()`,
`insert()`, `query()` (tree-sitter captures), `command(name, [args])`,
`status()`, `error()`, `define()`, `bind()` and `on()` (`open`, `change`,
`save`, `close`, `mode`, `idle`). `:script/eval <code>` runs a line and
shows its value, like `:lua`; an endless loop is stopped, not the editor.

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
# ~/.config/pure/config.toml
[editor]
directory-handler = "oil/open"
```

## Settings and permissions

`~/.config/pure/plugins.toml` has one table per plugin. Plugins can only do
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

[markdown]
lazy = false                     # load at startup even if it would wait

[my-ai-tool]
allow-network = ["api.example.com", "*.example.org"]  # HTTPS hosts it may reach
```

Some plugins load only when needed: when a file of their language opens or
one of their commands runs (`:plugin-list` says which haven't loaded yet).

Plugins that declare their settings have them checked when they load: a
misspelled name or a wrong type is an error that names the setting. After
you change only a plugin's settings, `:config-reload` passes them on
without reloading it; changing what it is allowed (`allow-…`), its keys or
`enabled` reloads the plugins.

`allow-listen` lets a plugin receive requests from programs on your
machine, such as the Competitive Companion browser extension. It listens
on 127.0.0.1 only and accepts only JSON requests addressed to `localhost`,
so web pages can't reach it.

`allow-lsp` also lets a plugin be a language server itself (its hover,
code actions, go-to-definition and the like show as any server's), hear
language servers' notifications and change their replies before Pure uses
them. Starting a language server program needs `allow-process` for it too.

`allow-network` lets a plugin send HTTPS requests to the hosts listed
(`*.example.org` means its subdomains). The editor sends them with `curl`,
which must be installed; it never follows redirects and never connects to
your machine or local network, even if a name resolves there.

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
