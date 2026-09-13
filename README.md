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
| ISO-BMFF | HEIF/AVIF/MP4/MOV/M4A brand detection, bounded box walking, and QuickTime-style `ilst` text metadata |
| MP3 | ID3v2.2/v2.3/v2.4 text, comments, lyrics, attached-picture metadata, ID3v1 fallback, and first MPEG frame properties |
| Output | Human-readable text, one JSON document, or JSON Lines; schema version `1` |
| Safety | Checked offsets, bounded reads, recursion and entry limits, deterministic recursive traversal, and structured warnings |

Writing, creation, deletion, metadata copying, MakerNotes interpretation, and
full media and ExifTool compatibility are intentionally not advertised as
implemented yet. PDF, FLAC, and manufacturer-specific MakerNotes are still
planned; MP3 and ID3 are only partially covered. Their boundaries are tracked in
[`compat/exiftool-compatibility.json`](compat/exiftool-compatibility.json).

## Quick start

With Rust 1.95 or newer:

```bash
cargo run -- photo.jpg
cargo run -- --json photo.jpg
cargo run -- --jsonl -r photos/
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

## Roadmap

1. Expand the read model and generated tag definitions without losing raw data.
2. Add corpus and differential tests for JPEG/TIFF/PNG/WebP edge cases.
3. Deepen HEIF/AVIF and media container readers, then add MP3/FLAC/PDF readers.
4. Expand XMP/IPTC/ICC/ID3 coverage and add isolated MakerNote namespaces.
5. Design read-modify-write with validation, temporary files, and atomic replace.
6. Add carefully scoped set/delete/copy commands only after round-trip tests.
7. Add controlled parallel batch processing and benchmark real collections.

## License

Metra is distributed under the MIT license; see [`LICENSE`](LICENSE).
