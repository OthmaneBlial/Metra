# Metra

Fast, safe metadata inspection powered by Rust.

Metra is an independent metadata toolkit whose library is the product and whose
CLI is a thin consumer of the same public API. The current release is a
read-only foundation: it inspects container metadata without decoding image
pixels and reports unsupported blocks instead of pretending to understand them.

## Current status

The first verified vertical slice is:

```text
JPEG / TIFF / PNG / WebP / GIF / ISO-BMFF media
        ↓
bounded container parsing
        ↓
typed Rust metadata model
        ↓
human-readable, JSON, or JSON Lines output
```

Implemented today:

| Area | Current behavior |
| --- | --- |
| JPEG | Magic-byte detection, segment walking, JFIF properties, JPEG comments, EXIF APP1, structured XMP, basic ICC profiles, and IPTC resources from Photoshop blocks |
| TIFF/EXIF | Little- and big-endian headers, IFDs, nested EXIF/GPS/Interop directories, rational values, unknown tags, thumbnail range checks, and decimal GPS helpers |
| PNG | Chunk walking, CRC warnings, tEXt/iTXt, eXIf, tIME, pHYs, structured XMP, and ICC presence warnings |
| WebP | RIFF chunk walking, VP8X dimensions, EXIF, structured XMP, and ICC presence warnings |
| GIF | GIF87a/GIF89a headers, logical-screen dimensions, comments, and bounded extension validation |
| ISO-BMFF | HEIF/AVIF/MP4/MOV/M4A brand detection, bounded box walking, `ispe` dimensions, direct XMP/EXIF, and QuickTime-style `ilst` text metadata |
| MP3 | ID3v2.2/v2.3/v2.4 text, comments, lyrics, attached-picture metadata, ID3v1 fallback, and first MPEG frame properties |
| FLAC | `STREAMINFO`, Vorbis comments, embedded-picture properties/data, and bounded metadata-block validation |
| PDF | Header/version, bounded Info dictionaries, PDF string decoding, and embedded XMP packets when directly available |
| WAV | RIFF/WAVE chunks, `fmt ` audio properties, `LIST/INFO`, Broadcast Wave `bext`, and bounded validation |
| Output | Human-readable text, JSON, JSON Lines, or CSV; schema version `1` for structured JSON |
| Batch | Deterministic path ordering with bounded parallel inspection through `--jobs N` |
| Safety | Checked offsets, bounded reads, recursion and entry limits, deterministic recursive traversal, and structured warnings |

Writing, creation, deletion, metadata copying, MakerNotes interpretation, and
full media and ExifTool compatibility are intentionally not advertised as
implemented yet. Manufacturer-specific MakerNotes are still planned;
MP3/ID3, FLAC/Vorbis comments, PDF, and WAV are only partially covered. Their
boundaries are tracked in
[`compat/exiftool-compatibility.json`](compat/exiftool-compatibility.json).

## Quick start

With Rust 1.95 or newer:

```bash
cargo run -- photo.jpg
cargo run -- --json photo.jpg
cargo run -- --jsonl -r photos/
cargo run -- --jsonl --jobs 4 -r photos/
cargo run -- --csv -r photos/
```

Install the local CLI:

```bash
cargo install --path .
metra --json photo.jpg
```

The filename extension is not used as the primary detector. A file with an
unknown signature exits with a clear error; malformed embedded metadata is
reported as a warning when the surrounding container can still be inspected.

## Library API

The root crate exposes the same API used by the binary:

```rust
let metadata = metra::read("photo.jpg")?;

if let Some(make) = metadata.find("EXIF:Make") {
    println!("camera = {}", make.display_value());
}
```

The model keeps namespaces explicit (`EXIF`, `GPS`, `PNG`, `WebP`, `JFIF`,
`XMP`, `IPTC`, `ICC`, and `ISOBMFF`),
retains bounded raw bytes, represents rational and array values without
flattening them into strings, and exposes warnings separately from tags.

The JSON schema is versioned at the document level:

```json
{
  "schema_version": 1,
  "file_info": { "format": "JPEG" },
  "tags": [],
  "warnings": []
}
```

Consumers should use `namespace` plus canonical `name` (for example
`EXIF:DateTimeOriginal`) rather than relying on human display text.

## Architecture

```text
src/main.rs                 CLI argument and output layer
src/lib.rs                  public `metra` facade
crates/metra-core           model, limits, and structured errors
crates/metra-formats        signature detection and format readers
```

Each reader receives a `FileInfo`, a `Read + Seek` source, and explicit
`ParseLimits`. It can add typed tags and non-fatal warnings to the shared model.
The TIFF reader uses checked arithmetic and random access, so a large container
does not need to be copied wholesale into memory. The JPEG, PNG, and WebP
readers only materialize bounded metadata chunks.

The next architectural boundaries are deliberately deferred until behavior
requires them: a generated tag database, isolated MakerNote readers, deeper
media metadata support, and a transactional rewrite engine. This keeps the
current working slice small enough to test while leaving the public model
extensible.

## Validation

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The tests generate small synthetic files at runtime, covering signatures,
little-endian nested EXIF, malformed offsets, JPEG segments, structured XMP,
IPTC/ICC resources, PNG CRC behavior, WebP dimensions, GIF extensions, ISO-BMFF
boxes, and CLI JSON/human output. Real-world corpus and differential
compatibility tests are separate follow-up gates; passing these local tests does
not claim complete ExifTool compatibility.

GitHub Actions automatic push and pull-request triggers are currently disabled;
the workflow remains available for a deliberate manual run. The commands above
are the local validation gate.

## Roadmap

Avancement global vérifié : **45 %**. Ce chiffre est une moyenne indicative des
huit axes ci-dessous, calculée uniquement sur le code et les tests présents ; il
ne représente pas un pourcentage de compatibilité ExifTool.

1. **70 %** — Étendre le modèle de lecture et les définitions de tags sans perdre les données brutes.
2. **30 %** — Ajouter des corpus réels et des tests différentiels JPEG/TIFF/PNG/WebP.
3. **88 %** — Approfondir HEIF/AVIF et les conteneurs média, puis couvrir les lecteurs restants.
4. **65 %** — Étendre XMP/IPTC/ICC/ID3 et isoler les espaces MakerNote.
5. **0 %** — Concevoir l’écriture read-modify-write avec validation et remplacement atomique.
6. **0 %** — Ajouter `set`/`delete`/`copy` après les tests round-trip.
7. **45 %** — Ajouter le traitement parallèle contrôlé et les benchmarks sur collections réelles.
8. **60 %** — Étendre les sorties structurées avec CSV, TOML et YAML versionnés.

## License

Metra is distributed under the MIT license; see [`LICENSE`](LICENSE).
