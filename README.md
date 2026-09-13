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
JPEG / TIFF / PNG / WebP / GIF / SVG / ISO-BMFF media
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
| JPEG | Magic-byte detection, segment walking, JFIF properties, JPEG comments, EXIF APP1, structured XMP, typed ICC profile headers/descriptions, and IPTC resources from Photoshop blocks |
| TIFF/EXIF | Little- and big-endian headers, IFDs, nested EXIF/GPS/Interop directories, rational values, unknown tags, thumbnail range checks, and validated decimal GPS latitude/longitude, altitude, direction, time, and speed helpers |
| PNG | Chunk walking, CRC warnings, tEXt/zTXt/iTXt including bounded zlib text, eXIf, tIME, pHYs, structured XMP, and bounded ICC profile headers from `iCCP` |
| WebP | RIFF chunk walking, VP8X dimensions, EXIF, structured XMP, and ICC presence warnings |
| GIF | GIF87a/GIF89a headers, logical-screen dimensions, comments, and bounded extension validation |
| ISO-BMFF | HEIF/AVIF/MP4/MOV/M4A brand detection, bounded box walking, `ispe` dimensions, direct XMP/EXIF, and QuickTime-style `ilst` text metadata |
| MP3 | ID3v2.2/v2.3/v2.4 text, comments, lyrics, attached-picture metadata, ID3v1 fallback, and first MPEG frame properties |
| FLAC | `STREAMINFO`, Vorbis comments, embedded-picture properties/data, and bounded metadata-block validation |
| PDF | Header/version, bounded Info dictionaries, PDF string decoding, and embedded XMP packets when directly available |
| WAV | RIFF/WAVE chunks, `fmt ` audio properties, `LIST/INFO`, Broadcast Wave `bext`, and bounded validation |
| SVG | Bounded XML detection, root dimensions/version/viewBox, title, description, comments, nesting/text limits, and safe document-text rewrites |
| Output | Human-readable text, JSON, JSON Lines, CSV, TOML, or YAML; schema version `1` is retained in structured output |
| Batch | Deterministic path ordering with bounded parallel inspection through `--jobs N`; human, JSON Lines, and CSV modes stream results with a bounded out-of-order buffer |
| Safety | Checked offsets, bounded reads, recursion and entry limits, deterministic recursive traversal, safe XML entity handling, and structured warnings |

Generic writing, creation, SVG embedded-XMP extraction, MakerNotes
interpretation, and full media and ExifTool compatibility are intentionally not
advertised as implemented yet. The library now supports validated, lossless
JPEG comment, bounded APP1 XMP, and selected IPTC-IIM datasets in Photoshop
APP13 resources, PNG `tEXt` and uncompressed `iTXt` XMP, GIF comments, WebP XMP, SVG
title/description/comments, WAV `LIST/INFO`, FLAC Vorbis Comment, and common
ID3v2 text/comment frames through format-specific rewrite APIs, and the CLI
exposes the same narrow operations through `--set`, `--delete`, and `--copy`.
Repeated IPTC datasets remain typed arrays when read; `--copy` accepts only a
single-valued source dataset, while `--set` replaces all target occurrences
with one bounded dataset.
Manufacturer-specific MakerNotes are still planned; MP3/ID3, PDF, WAV, and
FLAC remain only partially covered outside their explicit writable fields. ID3
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
is available, rather than relying on human display text. The shared tag catalog
is intentionally partial and will grow through generated definitions.

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
media metadata support, and a generalized rewrite capability layer. The current
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
boxes, bounded PNG zlib expansion, and CLI JSON/human output. Real-world corpus and differential
compatibility tests are separate follow-up gates; passing these local tests does
not claim complete ExifTool compatibility.

The opt-in corpus checks live in [`tests/corpus.rs`](tests/corpus.rs) and require
an explicit local corpus; because no real corpus or oracle run is bundled here,
the corpus axis below remains at its conservative 30 %.

For batch output, human-readable, JSON Lines, and CSV modes render as results
arrive while preserving deterministic input-path order. JSON, TOML, and YAML
need a complete document or collection, so they intentionally retain their
successful results until serialization.

GitHub Actions automatic push and pull-request triggers are currently disabled;
the workflow remains available for a deliberate manual run. The commands above
are the local validation gate.

## Roadmap

Avancement global vérifié : **76 %**. Ce chiffre est une moyenne indicative des
huit axes ci-dessous, calculée uniquement sur le code et les tests présents ; il
ne représente pas un pourcentage de compatibilité ExifTool.

1. **84 %** — Étendre le modèle de lecture et les définitions de tags sans perdre les données brutes ; les dérivés GPS valident maintenant les références, les plages et les conversions altitude/direction/temps/vitesse.
2. **30 %** — Ajouter des corpus réels et des tests différentiels JPEG/TIFF/PNG/WebP ; le harnais opt-in est présent, mais aucune exécution de corpus réel n’est comptée.
3. **90 %** — Approfondir HEIF/AVIF et les conteneurs média, puis couvrir les lecteurs restants.
4. **86 %** — Étendre XMP/IPTC/ICC/ID3 et isoler les espaces MakerNote ; XMP est maintenant réécrit de façon bornée pour JPEG APP1, WebP et PNG, les datasets IPTC-IIM connus peuvent être réécrits dans les ressources Photoshop APP13, les profils ICC JPEG et PNG `iCCP` sont inspectés sous limites, les références XML sûres sont décodées sans entités personnalisées, les textes PNG compressés sont déployés sous budget, et les champs texte/commentaires ID3v2 courants restent sous limites explicites.
5. **65 %** — Concevoir l’écriture read-modify-write avec validation et remplacement atomique ; huit writers bornés couvrent maintenant JPEG, PNG, GIF, WebP, SVG, WAV, FLAC et ID3v2.
6. **94 %** — Ajouter `set`/`delete`/`copy` après les tests round-trip ; les trois opérations couvrent maintenant JPEG `Comment`/`XMP` et datasets IPTC-IIM connus, PNG `tEXt`/`XMP`, GIF `Comment`, WebP `XMP`, SVG `Title`/`Description`/`Comment`, WAV `LIST/INFO`, FLAC Vorbis Comments et ID3v2 texte/commentaire via API et CLI.
7. **60 %** — Ajouter le traitement parallèle contrôlé, le rendu en flux borné et les benchmarks sur collections réelles.
8. **100 %** — Étendre les sorties structurées avec CSV, TOML et YAML versionnés.

## License

Metra is distributed under the MIT license; see [`LICENSE`](LICENSE).
