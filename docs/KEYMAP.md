# Keymap

Default bindings for lazydisk (terminal and desktop). Every command can be rebound
in `~/.config/lazydisk/config.toml` under `[keys]` using the command id.

Generated from `lazydisk_core::keymap`; regenerate with
`cargo run -p lazydisk-core --example keymap > docs/KEYMAP.md`.

| Keys | Command | Description |
|---|---|---|
| `k` `↑` | `up` | Select previous item |
| `j` `↓` | `down` | Select next item |
| `ctrl+u` `pgup` | `page_up` | Select one page up |
| `ctrl+d` `pgdn` | `page_down` | Select one page down |
| `g` `home` | `first` | Select first item |
| `G` `end` | `last` | Select last item |
| `l` `enter` `→` | `enter` | Open selected folder / scan selected volume |
| `h` `esc` `←` `backspace` | `parent` | Go to the enclosing folder |
| `[` | `back` | Go back in history |
| `]` | `forward` | Go forward in history |
| `1` | `focus_volumes` | Focus the Volumes panel |
| `2` | `focus_files` | Focus the Files panel |
| `3` | `focus_collector` | Focus the Collector panel |
| `4` | `focus_details` | Focus the Details panel |
| `tab` | `next_panel` | Focus next panel |
| `shift+tab` | `prev_panel` | Focus previous panel |
| `/` | `filter` | Filter the current folder by name |
| `s` | `toggle_size_mode` | Toggle allocated / apparent size |
| `u` | `toggle_units` | Toggle binary / SI units |
| `+` `=` | `more_rings` | Show one more sunburst ring |
| `-` | `fewer_rings` | Show one fewer sunburst ring |
| `o` | `reveal` | Reveal in Finder / file manager |
| `space` | `preview` | Quick Look preview |
| `y` | `copy_path` | Copy path to clipboard |
| `c` | `toggle_collect` | Add to / remove from the Collector |
| `C` | `clear_collector` | Empty the Collector |
| `d` | `delete_trash` | Move collected items to the Trash |
| `D` | `delete_permanent` | Delete collected items permanently |
| `r` | `rescan` | Rescan the current folder |
| `R` | `rescan_all` | Rescan everything |
| `x` | `menu` | Open the context menu |
| `?` | `help` | Show this help |
| `q` `ctrl+c` | `quit` | Quit |

## Inside overlays

| Keys | Action |
|---|---|
| any key | close the help or a message |
| typing, `backspace`, `enter`, `esc` | edit, accept or cancel the filter |
| `y` / `n` / `esc` | confirm or cancel moving to the Trash |
| type `DELETE`, `enter` / `esc` | confirm or cancel a permanent delete |
| `j`/`k`, `1`-`9`, `enter`, `esc` | move, pick or close a menu |

## Mouse (terminal)

Click a segment to select it, click it again to open it, click the centre to go up,
scroll over the sunburst to go up and down, click rows to select them.
