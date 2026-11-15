# Stewdio v0.1

A standalone Roblox place file toolkit: **decompile → edit → compile**.

## Implemented in v0.1

| Command | Status |
|---|---|
| `new`, `init` | Done (`--model` arrives v0.3) |
| `decompile <file.rbxlx>` | Done (binary `.rbxl` arrives v0.2) |
| `compile` | Done, XML only (`--xml`; binary arrives v0.2) |
| `validate`, `info`, `version`, `help` | Done |
| `config`, `auth`, `upload` | Stubs (v0.4+) |

Defaults differ slightly from the full spec because only XML exists in v0.1:
new projects get `compiler.format = "xml"` and output `build/<name>.rbxlx`.

## Project layout

- Each instance is a directory with `Instance.data.hjson`.
- Scripts are `Name.server.lua`, `Name.client.lua`, `Name.module.lua` with a sidecar
  `Name.server.data.hjson`. A script with children becomes a directory containing
  `init.server.lua` (etc.) plus its children.
- Properties are stored as `true`/`false`, strings, or typed objects such as
  `{"$type": "Vector3", "X": 1, "Y": 2, "Z": 3}`. References use `$id`:
  `{"$type": "Ref", "$value": "instance-005"}`.
- Untyped values are inferred when compiling (`{X,Y,Z}` → Vector3, `{R,G,B}` → Color3,
  integers → int, decimals → float).

## Known v0.1 limitations

- Attributes (`AttributesSerialize`) are preserved as a raw `BinaryString` property, not decoded.
- `<SharedStrings>` and `<Meta>` entries are not preserved.
- Sibling order is not preserved (children load alphabetically).

## Build

    cargo build --release
    cargo test

## Exit codes

0 success · 1 general · 2 invalid arguments · 3 invalid project · 4 parsing · 5 compilation ·
6 authentication · 7 upload · 8 unsupported format
