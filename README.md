# Stewdio v0.6

A standalone Roblox place file toolkit: **decompile → edit → compile → upload**, no Roblox Studio required.

## Commands

| Command | What it does |
|---|---|
| `from <universe> <place>` | Create a project by downloading a place from Roblox |
| `pull` | Replace the project's instances with the latest copy of its place |
| `new`, `init` | Create / initialize a place or `--model` project |
| `decompile <file>` | `.rbxl`, `.rbxlx`, `.rbxm`, `.rbxmx` → project directory |
| `compile` | Project → `.rbxl` / `.rbxlx` (places) or `.rbxm` / `.rbxmx` (models) |
| `explorer` | Terminal UI: instance tree, properties and a script editor |
| `watch` | Rebuild automatically when files change |
| `validate`, `info`, `doctor` | Checks and information |
| `api update` / `api status` | Download / inspect the Roblox API database |
| `settings` | Edit `Project.toml` from the CLI (`list`, `get`, `set`, `unset`, `edit`, or interactive) |
| `config` | Global settings |
| `auth` | `login`, `logout`, `status`, `list`, `use <profile>` |
| `upload` | Upload a place with Open Cloud (saved by default, `--publish` to go live) |
| `revert` | Re-upload an earlier snapshot as a new version |
| `diff <a> <b>` | Compare two place/model files or project directories |
| `tree`, `find <text>` | Print the hierarchy / search names, scripts and property values |
| `open` | Compile and open the result in Roblox Studio (default program) |
| `clean`, `completions <shell>` | Remove build output / shell completions |

## Metadata format

`Instance.data.hjson` files are real [HJSON](https://hjson.github.io): unquoted keys, quoteless strings, no commas,
`#` comments. Hand-written JSON-style files (quoted keys, commas) still load.

    # Stewdio instance metadata (HJSON)
    {
      $className: Part
      $name: Baseplate
      $id: instance-002
      Properties: {
        Anchored: true
        size: {
          $type: Vector3
          X: 512
          Y: 1
          Z: 512
        }
      }
      Attributes: {}
      Tags: []
    }

## Downloading a place (`from` / `pull`)

    stewdio auth login                  # key needs the "legacy-asset:manage" permission
    stewdio from 123456789 987654321    # <universe_id> <place_id>
    cd my-place && stewdio explorer
    stewdio pull                        # later: refresh from Roblox (old files go to .stewdio/backup)

`from` asks Roblox's asset delivery API for the place file, converts it into a project (universe and place IDs are
written to `Project.toml`, so `upload` works immediately) and keeps the download as a baseline snapshot for `revert`.
Options: `--name`, `--path`, `--version <n>`, `--no-scripts`, `--profile`. The place must belong to the API key's owner.

## Explorer

`stewdio explorer` opens the project in a split terminal view: the **instance tree** on top and the **properties** of
the selected instance below. Rows are indented by depth; `▸` marks a closed instance with children and `▾` an open one.

| Key | Action |
|---|---|
| Up / Down, PgUp / PgDn | Move |
| Right / Left | Open / close (or jump to the parent) |
| Space / Enter | Action menu: **Edit, Rename, Cut, Copy, Paste, Add, Insert child, Delete** |
| `a` | **Add** a new instance as the sibling directly below the selected one |
| `i` | Insert a child |
| `e` `r` `c` `x` `d` | Edit, rename, copy, cut, delete shortcuts |
| Ctrl+P | Paste the copied/cut instances into the selected parent |
| Tab | Switch between the explorer and the properties tab |
| Ctrl+S / Ctrl+Z | Save the project to disk / undo |
| `/` then `n` | Find instance by name / next match |
| `*` / `-` | Expand all / collapse all |
| `q` | Quit (asks about unsaved changes) |

**Edit** on a Script, LocalScript or ModuleScript opens a full-screen script editor (Luau syntax highlighting, block
balance diagnostics, auto-indent, find, undo). **Ctrl+X exits** the editor; Ctrl+S applies without leaving. **Edit** on any
other instance moves to the properties tab. The **properties tab lists every savable property of the class** (grouped by category, like Studio). Properties you have
not set show their default value in grey; Enter on one creates it with the value you type (enum names such as `Wood` are
accepted), `r` / Delete resets a set property back to its default. This needs `stewdio api update`; without the data only
the properties that are set are listed. Defaults come from the dump when it has them, plus a small built-in table.
Ctrl+D duplicates the selected instance. The mouse works too (click rows and menu items,
scroll wheel). Saving moves the old instance folders to `.stewdio/backup/` first (the newest 5 are kept).

## Output format

The **file extension decides the format** (`--output game.rbxlx` is always XML); `--xml` / `--binary` override the
default, and a conflict is an error. Every compile re-reads what it wrote and checks the instance count, and `upload`
refuses a file whose contents do not match its extension.

## Autofill and validation (Roblox API database)

Run `stewdio api update`. It downloads the current API dump from the
[Roblox-Client-Tracker](https://github.com/MaximumADHD/Roblox-Client-Tracker) repository and caches a compact index.
`compile`, `validate`, `info`, `doctor` and the explorer then nudge you when the data is older than `api.max_age_days`
(default 30) or missing. With the data installed, compiling checks class/property names ("did you mean...?") and
coerces loosely written values (enum names, integers → floats, `[r,g,b]`, `"#rrggbb"`, ids → Refs).
`compiler.autofill = "defaults"` additionally writes default values when the dump provides them.

## Upload and revert

    stewdio auth login          # Open Cloud API key (OS credential store; STEWDIO_API_KEY overrides)
    stewdio upload              # confirm, upload as a *Saved* version
    stewdio upload --publish    # publish instead
    stewdio revert --list       # local upload history
    stewdio revert              # pick an earlier upload and re-upload it as a new version

Open Cloud cannot list or roll back server-side place versions, so `revert` uses snapshots Stewdio keeps in
`.stewdio/history/` whenever it uploads (newest 20). Roblox records a revert as a new version.

## Other

- Settings precedence: CLI > Project.toml > global config > defaults.
- `.stewdioignore` supports `*`, `?`, `**`, trailing `/`, leading `/` and `!` negation.
- Network features use the system `curl`; `stewdio doctor` checks for it.

## Known limitations

- Binary-format property types not interpreted (Font, UniqueId, OptionalCFrame, SecurityCapabilities) are skipped with a warning.
- Binary strings are typed by property name; SharedString keys use a Stewdio hash instead of MD5.
- Attributes are kept as a raw `AttributesSerialize` BinaryString.
- The script editor has no text selection or LSP (highlighting + block diagnostics only).
- Only places can be uploaded.

## Build

    cargo build --release
    cargo test

Exit codes: 0 success · 1 general · 2 arguments · 3 invalid project · 4 parsing · 5 compilation ·
6 authentication · 7 upload · 8 unsupported format
