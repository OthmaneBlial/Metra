# Metra

Fast, safe metadata inspection powered by Rust.

Metra is an independent metadata toolkit whose library is the product and whose
CLI is a thin consumer of the same public API. The current release is a
read-first foundation with narrow, validated rewrites: it inspects container
metadata without decoding image pixels and reports unsupported blocks instead of
pretending to understand them.

## Current status

The first verified vertical slice is:

```text
JPEG / TIFF / PNG / WebP / GIF / SVG / PSD/PSB / RAW / AVI / MKV / WebM / ISO-BMFF media
        ↓
bounded container parsing
        ↓
typed Rust metadata model
        ↓
human-readable or structured output
```

Implemented today:

| Area | Current behavior |
| --- | --- |
| JPEG | Magic-byte detection, segment walking, JFIF properties, JPEG comments, EXIF APP1, structured XMP, reassembled typed ICC profiles with common table values, and IPTC resources from Photoshop blocks |
| TIFF/EXIF | Little- and big-endian classic TIFF and BigTIFF headers, 64-bit IFD counts/offsets, nested EXIF/GPS/Interop directories, common image/exposure/lens tag names, rational values, ASCII/Unicode `UserComment`, common MakerNote container detection with bounded Nikon Type 2 fields, unknown tags, thumbnail range checks, and validated decimal GPS latitude/longitude, altitude, direction, time, and speed helpers |
| PNG | Chunk walking, CRC warnings, tEXt/zTXt/iTXt including bounded zlib text, eXIf, tIME, pHYs, structured XMP, and bounded ICC profile headers from `iCCP` |
| WebP | RIFF chunk walking, VP8X dimensions, EXIF, structured XMP, and typed ICC profiles |
| GIF | GIF87a/GIF89a headers, logical-screen dimensions, comments, and bounded extension validation |
| ISO-BMFF | HEIF/AVIF/MP4/MOV/M4A brand detection, bounded box walking, `ispe` dimensions, direct XMP/EXIF, and QuickTime-style `ilst` text metadata |
| MP3 | ID3v2.2/v2.3/v2.4 text, comments, lyrics, attached-picture metadata, ID3v1 fallback, and first MPEG frame properties |
| FLAC | `STREAMINFO`, Vorbis comments, embedded-picture properties/data, and bounded metadata-block validation |
| PDF | Header/version, bounded Info dictionaries, PDF string decoding, and embedded XMP packets when directly available |
| WAV | RIFF/WAVE chunks, `fmt ` audio properties, `LIST/INFO`, Broadcast Wave `bext`, and bounded validation |
| SVG | Bounded XML detection, root dimensions/version/viewBox, title, description, comments, nesting/text limits, and safe document-text rewrites |
| PSD/PSB | Big-endian header and dimensions, bounded Photoshop image resources, XMP/IPTC/ICC/embedded EXIF delegation, resolution and common resource fields, and preservation of unknown resources as bytes |
| RAW | DNG and TIFF-like CR2/NEF/ARW/ORF/RW2/PEF containers reuse the bounded TIFF/EXIF reader with common DNG tags (version, CFA, levels, matrices, white balance, and camera/lens identity); CR3 reuses ISO-BMFF inspection; RAF is identified with an explicit partial-decoding warning |
| AVI | RIFF/AVI validation, bounded `avih` dimensions and frame timing, and common `LIST/INFO` text fields without decoding video frames |
| MKV/WebM | EBML signature and document-type detection, bounded `Info`/`Tracks`/`Tags` scanning, typed duration, track, title, codec, and tag values, without decoding clusters |
| Output | Human-readable text, JSON, JSON Lines, CSV, TOML, or YAML; schema version `1` is retained in structured output |
| Batch | Deterministic path ordering with bounded parallel inspection through `--jobs N`; human, JSON Lines, and CSV modes stream results with a bounded out-of-order buffer |
| Safety | Checked offsets, bounded reads, recursion and entry limits, deterministic recursive traversal, safe XML entity handling, and structured warnings |

Generic writing, creation, PSD/PSB/RAW/MKV/WebM writing, SVG embedded-XMP extraction, MakerNote tag
interpretation beyond the bounded Nikon Type 2 fields, and full media and
ExifTool compatibility are intentionally not advertised as implemented yet.
The library now supports validated, lossless
JPEG comment, bounded APP1 XMP, and selected IPTC-IIM datasets in Photoshop
APP13 resources, PNG `tEXt` and uncompressed `iTXt` XMP, GIF comments, WebP XMP, SVG
title/description/comments, WAV `LIST/INFO`, FLAC Vorbis Comment, and common
ID3v2 text/comment frames through format-specific rewrite APIs, and the CLI
exposes the same narrow operations through `--set`, `--delete`, and `--copy`.
Repeated IPTC datasets remain typed arrays when read; `--copy` accepts only a
single-valued source dataset, while `--set` replaces all target occurrences
with one bounded dataset.
MakerNotes remain partial outside the bounded Nikon Type 2 fields; MP3/ID3,
PDF, WAV, and FLAC remain only partially covered outside their explicit
writable fields. ID3
rewrites currently require a supported ID3v2 tag without unsynchronization,
extended-header, or footer flags. Their boundaries are tracked in
[`compat/exiftool-compatibility.json`](compat/exiftool-compatibility.json).

## Quick start

With Rust 1.95 or newer:

```bash
cargo run -- photo.jpg
cargo run -- --json photo.jpg
cargo run -- --jsonl -r photos/
cargo run -- --jsonl --jobs 4 -r photos/
cargo run -- --csv -r photos/
cargo run -- --toml photo.jpg
cargo run -- --yaml photo.jpg
cargo run -- --validate --json photo.jpg
cargo run -- --validate --max-metadata-bytes 1048576 --max-value-bytes 65536 photo.jpg
cargo run -- --compare reference.jpg target.jpg
cargo run -- --set 'JPEG:Comment=reviewed' photo.jpg
cargo run -- --delete JPEG:Comment photo.jpg
cargo run -- --copy JPEG:Comment=source.jpg target.jpg
cargo run -- --set 'JPEG:XMP=<x:xmpmeta>...</x:xmpmeta>' photo.jpg
cargo run -- --delete JPEG:XMP photo.jpg
cargo run -- --copy JPEG:XMP=source.jpg target.jpg
cargo run -- --set 'IPTC:CaptionAbstract=reviewed' photo.jpg
cargo run -- --delete IPTC:Keywords photo.jpg
cargo run -- --copy IPTC:CaptionAbstract=source.jpg target.jpg
cargo run -- --set 'PNG:Text:Comment=reviewed' image.png
cargo run -- --set 'PNG:XMP=<x:xmpmeta>...</x:xmpmeta>' image.png
cargo run -- --set 'WAV:Title=reviewed' audio.wav
cargo run -- --set 'FLAC:Title=reviewed' audio.flac
cargo run -- --set 'ID3:Title=reviewed' audio.mp3
cargo run -- --set 'GIF:Comment=reviewed' animation.gif
cargo run -- --set 'WebP:XMP=<x:xmpmeta>...</x:xmpmeta>' image.webp
cargo run -- --set 'SVG:Title=reviewed' drawing.svg
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

if let Some(make) = metadata.find_by_id("EXIF", 0x010F) {
    println!("stable tag = {}", make.identifier().name);
}

for keyword in metadata.find_all("IPTC:Keywords") {
    println!("keyword = {}", keyword.display_value());
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
`EXIF:DateTimeOriginal`) or `find_by_id` when a format-level numeric identifier
is available, rather than relying on human display text. Use `find_all` or
`find_all_by_id` when a file can contain repeated blocks or datasets. The shared tag catalog
is intentionally partial and will grow through generated definitions. IPTC-IIM
tags retain their numeric dataset identifiers, so `find_by_id("IPTC", 25)` and
structured output remain stable even when repeated values are represented as arrays.

Format support is also available programmatically through
`format_capabilities(format)` and `format_capabilities_all()`. Each entry
reports independent `read`, `write`, `create`, `delete`, `lossless_rewrite`,
and `streaming` statuses instead of implying full support from detection alone.

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
requires them: a generated tag database, additional manufacturer-specific
MakerNote readers, deeper media metadata support, and a generalized rewrite
capability layer. The current
format-specific writers remain intentionally narrow and independently tested.

## Validation

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The tests generate small synthetic files at runtime, covering signatures,
little-endian nested EXIF, malformed offsets, JPEG segments, structured XMP,
IPTC/ICC resources, PNG CRC behavior, WebP dimensions, GIF extensions, ISO-BMFF
boxes, PSD/PSB headers and image resources, RAW container delegation, AVI RIFF lists, Matroska/WebM EBML
elements, bounded PNG zlib expansion, and CLI JSON/human output. Real-world corpus and differential
compatibility tests are separate follow-up gates; passing these local tests does
not claim complete ExifTool compatibility.

The opt-in corpus checks live in [`tests/corpus.rs`](tests/corpus.rs) and require
an explicit local corpus; because no real corpus or oracle run is bundled here,
the corpus axis below remains at its conservative 30 %.

For batch output, human-readable, JSON Lines, and CSV modes render as results
arrive while preserving deterministic input-path order. JSON, TOML, and YAML
need a complete document or collection, so they intentionally retain their
successful results until serialization.

`--validate` keeps the normal metadata output but exits non-zero when a file
produces one or more recoverable parser warnings, which makes corruption checks
usable in scripts without hiding the parsed evidence.
`--compare reference target` reports added, removed, and changed metadata values;
it returns zero for equality and one when a difference or read failure is found.
Use `--max-metadata-bytes` and `--max-value-bytes` to tighten per-file safety
budgets; the same limits are applied to source and output validation during edits.

GitHub Actions automatic push and pull-request triggers are currently disabled;
the workflow remains available for a deliberate manual run. The commands above
are the local validation gate.

## Roadmap

Avancement global vérifié : **79 %**. Ce chiffre est une moyenne indicative des
huit axes ci-dessous, calculée uniquement sur le code et les tests présents ; il
ne représente pas un pourcentage de compatibilité ExifTool.

1. **92 %** — Étendre le modèle de lecture et les définitions de tags sans perdre les données brutes ; le parseur TIFF couvre maintenant les en-têtes classic et BigTIFF, les offsets/compteurs 64 bits et les valeurs LONG8/SLONG8/IFD8, tandis que les dérivés GPS valident les références, les plages et les conversions altitude/direction/temps/vitesse, les datasets IPTC-IIM lus conservent leur identifiant numérique stable, `EXIF:UserComment` décode les préfixes ASCII/Unicode sans perdre les octets bruts, les tags image/exposition/objectif courants ont des noms canoniques, et les champs Nikon bornés sont résolus par le catalogue partagé.
2. **30 %** — Ajouter des corpus réels et des tests différentiels JPEG/TIFF/PNG/WebP ; le harnais opt-in est présent, mais aucune exécution de corpus réel n’est comptée.
3. **94 %** — Approfondir HEIF/AVIF et les conteneurs média, puis couvrir les lecteurs restants ; des lecteurs PSD/PSB, RAW, AVI et MKV/WebM bornés couvrent maintenant leurs en-têtes et métadonnées courantes sans décoder les pixels ou les flux vidéo.
4. **95 %** — Étendre XMP/IPTC/ICC/ID3 et isoler les espaces MakerNote ; XMP est maintenant réécrit de façon bornée pour JPEG APP1, WebP et PNG, les datasets IPTC-IIM connus peuvent être réécrits dans les ressources Photoshop APP13, les profils ICC fragmentés JPEG, PNG `iCCP` et WebP `ICCP` sont inspectés sous limites avec descriptions texte et valeurs XYZ courantes, les références XML sûres sont décodées sans entités personnalisées, les textes PNG compressés sont déployés sous budget, les champs texte/commentaires ID3v2 courants restent sous limites explicites, et les conteneurs MakerNote courants sont identifiés ; un IFD Nikon Type 2 borné expose maintenant les champs connus via le catalogue partagé, avec intégration EXIF et offsets de source absolus testés.
5. **67 %** — Concevoir l’écriture read-modify-write avec validation et remplacement atomique ; huit writers bornés couvrent maintenant JPEG, PNG, GIF, WebP, SVG, WAV, FLAC et ID3v2, et les budgets metadata/valeur sont configurables depuis le CLI.
6. **95 %** — Ajouter `set`/`delete`/`copy` et comparer après les tests round-trip ; les opérations couvrent maintenant JPEG `Comment`/`XMP` et datasets IPTC-IIM connus, PNG `tEXt`/`XMP`, GIF `Comment`, WebP `XMP`, SVG `Title`/`Description`/`Comment`, WAV `LIST/INFO`, FLAC Vorbis Comments et ID3v2 texte/commentaire via API et CLI, avec comparaison déterministe des valeurs.
7. **60 %** — Ajouter le traitement parallèle contrôlé, le rendu en flux borné et les benchmarks sur collections réelles.
8. **100 %** — Étendre les sorties structurées avec CSV, TOML et YAML versionnés.

## License

Metra is distributed under the MIT license; see [`LICENSE`](LICENSE).
