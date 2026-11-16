# Stewdio v0.2

A standalone Roblox place file toolkit: **decompile → edit → compile**.

## Implemented

| Command | Status |
|---|---|
| `new`, `init` | Done (`--model` arrives v0.3) |
| `decompile <file.rbxl|.rbxlx>` | Done (binary and XML) |
| `compile` | Done (`--binary` default, `--xml`) |
| `validate`, `info`, `version`, `help` | Done |
| `config`, `auth`, `upload` | Stubs (v0.4+) |

### v0.2 additions
- Binary `.rbxl` parser and serializer (LZ4-compressed chunks: META, SSTR, INST, PROP, PRNT, END).
- Property preservation for: String/ProtectedString/BinaryString/Content, bool, int, float, double,
  int64, enum (token), UDim, UDim2, Ray, Faces, Axes, BrickColor, Color3, Color3uint8, Vector2,
  Vector3, Vector3int16, CFrame, NumberSequence, ColorSequence, NumberRange, Rect, PhysicalProperties,
  SharedString and Ref.
- Reference preservation: every instance gets a stable `$id`; `Ref` properties point at `$id`s and are
  re-linked on compile in either format. SharedStrings are now preserved in XML too.
- New projects default to `compiler.format = "binary"` and `build/<name>.rbxl`.

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

## Known limitations

- Binary property types not listed above (e.g. Font, UniqueId, OptionalCFrame, SecurityCapabilities)
  are skipped with a warning on decompile; XML-only types are skipped with a warning in binary output.
- Binary string properties are typed by name (Source → ProtectedString, a list of known asset
  properties → Content, a list of known blobs → BinaryString); other strings become `string`.
  Compiling back to binary is lossless; compiling a binary-origin project to XML may mistype
  unlisted Content properties.
- SharedString keys use a Stewdio content hash instead of MD5.
- Attributes (`AttributesSerialize`) are preserved as a raw `BinaryString`, not decoded.
- Instances of one class with different property sets are written as separate INST groups.
- Sibling order is not preserved (children load alphabetically).

## Build

    cargo build --release
    cargo test

## Exit codes

0 success · 1 general · 2 invalid arguments · 3 invalid project · 4 parsing · 5 compilation ·
6 authentication · 7 upload · 8 unsupported format
