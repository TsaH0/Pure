# Configuration

To override global configuration parameters, create a `config.toml` file located in your config directory:

- Linux and Mac: `~/.config/pure/config.toml`
- Windows: `%AppData%\pure\config.toml`

> 💡 You can easily open the config file by typing `:config-open` within Pure normal mode.

Pure keeps its files apart from Helix's: configuration in `~/.config/pure`,
plugin data in `~/.local/share/pure` and the log in `~/.cache/pure`. A setup
from before that, in `~/.config/helix` (and the others), keeps working while
there is no `pure` directory. To move it, rename the directories (or copy
them, to keep using Helix too):

```sh
mv ~/.config/helix ~/.config/pure
mv ~/.local/share/helix ~/.local/share/pure   # plugin data, workspace trust
mv ~/.cache/helix ~/.cache/pure
```

`pure --health` shows the config file and runtime directories in use.

Example config:

```toml
theme = "onedark"

[editor]
line-number = "relative"
mouse = false

[editor.cursor-shape]
insert = "bar"
normal = "block"
select = "underline"

[editor.file-picker]
hidden = false
```

You can use a custom configuration file by specifying it with the `-c` or
`--config` command line argument, for example `pure -c path/to/custom-config.toml`.
You can reload the config file by issuing the `:config-reload` command. Alternatively, on Unix operating systems, you can reload it by sending the USR1
signal to the Pure process, such as by using the command `pkill -USR1 pure`.

Finally, you can have a `config.toml` and a `languages.toml` local to a project by putting it under a `.helix` directory in your repository.
Its settings will be merged with the configuration directory and the built-in configuration.

