# Stewdio v0.4

A standalone Roblox place file toolkit: **decompile → edit → compile → upload**.
No Roblox Studio required.

## Commands

| Command | What it does |
|---|---|
| `new`, `init` | Create / initialize a place or `--model` project |
| `decompile <file>` | `.rbxl`, `.rbxlx`, `.rbxm`, `.rbxmx` → project directory |
| `compile` | Project → `.rbxl` / `.rbxlx` (places) or `.rbxm` / `.rbxmx` (models) |
| `validate`, `info`, `version`, `help` | Project checks and information |
| `api update` / `api status` | Download / inspect the Roblox API database |
| `config` | Global settings (`get`, `set`, `list`, `reset`, or interactive) |
| `auth` | `login`, `logout`, `status`, `list`, `use <profile>` |
| `upload` | Upload a place with Open Cloud (saved by default, `--publish` to go live) |

## Output format

The **file extension decides the format**: `--output game.rbxlx` is always XML and `game.rbxl` is always
binary, `--xml` / `--binary` override the default, and a conflicting combination is an error. Every compile
re-reads the bytes it just wrote and checks the instance count before reporting success. `upload` also
checks that the file contents match the extension, so a binary file can no longer be uploaded as `.rbxlx`.

## Autofill and validation (Roblox API database)

Run `stewdio api update` once. It downloads the current API dump from the
[Roblox-Client-Tracker](https://github.com/MaximumADHD/Roblox-Client-Tracker) repository and caches a compact index.
Then every `compile` / `validate`:

- checks class and property names ("did you mean ...?"),
- coerces loosely written values to the exact type Roblox expects: enum names → tokens (`"Material": "Wood"`),
  integers → floats, `[r,g,b]` / `{R,G,B}` / `"#rrggbb"` → Color3, instance `$id` strings → Refs, asset strings → Content,
- applies Part aliases without a database too: `Size` → `size`, `Position` → `CFrame`, `Color` → `Color3uint8`.

`compiler.autofill` = `off` | `normalize` (default) | `defaults`. `defaults` additionally writes default values
for missing properties when the dump provides them (the dump format may not include defaults; `api update` tells you).

## Authentication and upload

    stewdio auth login          # prompts for an Open Cloud API key (or --stdin)
    stewdio upload              # compile if needed, confirm, upload as a *Saved* version
    stewdio upload --publish    # publish instead

Keys are kept in the OS credential store (macOS Keychain, `secret-tool` on Linux, DPAPI on Windows) with a
user-only file fallback; they are never written to `Project.toml` or printed. `STEWDIO_API_KEY` overrides
stored credentials (handy in CI). The API key needs the `universe-places:write` permission.
Network features use the system `curl`.

## Project layout

- Each instance is a directory with `Instance.data.hjson`; scripts are `Name.server.lua`, `Name.client.lua`,
  `Name.module.lua` (+ sidecar metadata); a script with children is a directory with `init.server.lua` etc.
- Properties are `true`/`false`, strings, numbers, `[x,y,z]`, or typed objects like
  `{"$type": "Vector3", "X": 1, "Y": 2, "Z": 3}`. References use `$id`.
- Model projects have no services: each top-level directory is a root instance.
- `.stewdioignore` supports `*`, `?`, `**`, trailing `/`, leading `/` and `!` negation.

Settings precedence: CLI > Project.toml > global config > defaults.

## Known limitations

- Binary-format property types not interpreted (Font, UniqueId, OptionalCFrame, SecurityCapabilities) are skipped with a warning.
- Binary strings are typed by property name (Source, known asset/blob properties).
- SharedString keys use a Stewdio hash instead of MD5.
- Attributes are preserved as a raw `AttributesSerialize` BinaryString, not decoded.
- Sibling order is not preserved.
- Only places can be uploaded.

## Build

    cargo build --release
    cargo test

Exit codes: 0 success · 1 general · 2 arguments · 3 invalid project · 4 parsing · 5 compilation ·
6 authentication · 7 upload · 8 unsupported format
