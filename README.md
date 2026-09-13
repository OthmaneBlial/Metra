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
JPEG / TIFF / PNG / WebP / GIF / SVG / PSD/PSB / RAW / AVI / MKV / WebM / Ogg / ISO-BMFF media
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
| TIFF/EXIF | Little- and big-endian classic TIFF and BigTIFF headers, 64-bit IFD counts/offsets, nested EXIF/GPS/Interop directories, bounded multiple `SubIFD` offsets, chained IFD0/IFD1/IFD2 thumbnail directories, common image/exposure/lens tag names, rational values, ASCII/Unicode `UserComment`, common MakerNote container detection with bounded Nikon Type 2 and Canon IFD fields, retained unknown MakerNote values, unknown tags, thumbnail range checks, and validated decimal GPS latitude/longitude, altitude, direction, time, and speed helpers |
| PNG | Chunk walking, IHDR dimensions and encoding parameters, CRC warnings, tEXt/zTXt/iTXt including bounded zlib text, eXIf, tIME, pHYs, structured XMP, and bounded ICC profile headers from `iCCP` |
| WebP | RIFF chunk walking, VP8X/VP8/VP8L dimensions, EXIF, structured XMP, and typed ICC profiles |
| GIF | GIF87a/GIF89a headers, logical-screen dimensions, comments, and bounded extension validation |
| ISO-BMFF | HEIF/AVIF/MP4/MOV/M4A brand detection, bounded box walking, `mvhd` movie timing, `tkhd` track IDs/durations/dimensions, `ispe` dimensions, `pixi` channels, `irot`/`imir` orientation, `pasp` aspect ratio, `colr` nclx values, `auxC` auxiliary type, direct XMP/EXIF, QuickTime-style `ilst` text metadata, and validated in-place edits for existing text values |
| MP3 | ID3v2.2/v2.3/v2.4 text, comments, lyrics, attached-picture metadata, ID3v1 fallback, and first MPEG frame properties |
| FLAC | `STREAMINFO`, bounded `SEEKTABLE` seek-point and `CUESHEET` track/index structures, Vorbis comments, embedded-picture properties/data, and bounded metadata-block validation |
| Ogg/Vorbis/Opus | Bounded Ogg page walking with metadata-page CRC warnings, Vorbis and Opus stream headers, Vorbis Comments/OpusTags, Ogg-FLAC comments, and typed FLAC-in-Ogg `STREAMINFO` fields |
| PDF | Header/version, bounded Info dictionaries, PDF string decoding, and embedded XMP packets when directly available |
| WAV | RIFF/WAVE chunks, `fmt ` audio properties, `LIST/INFO`, Broadcast Wave `bext`, bounded iXML XML leaves and packet retention, embedded ID3v2 delegation, and bounded validation |
| SVG | Bounded XML detection, root dimensions/version/viewBox, title, description, comments, nesting/text limits, and safe document-text rewrites |
| Standalone XMP/ICC | Signature-based standalone XMP packet and ICC profile readers reuse the bounded XML/profile engines and retain the detected file family |
| PSD/PSB | Big-endian header and dimensions, bounded Photoshop image resources, XMP/IPTC/ICC/embedded EXIF delegation, resolution and common resource fields, and preservation of unknown resources as bytes |
| RAW | DNG and TIFF-like CR2/NEF/ARW/ORF/RW2/PEF containers reuse the bounded TIFF/EXIF reader with common DNG tags (version, CFA, levels, matrices, white balance, and camera/lens identity); CR3 reuses ISO-BMFF inspection; RAF, legacy Canon CRW, Minolta MRW, and Sigma X3F containers are identified with explicit partial-decoding warnings |
| AVI | RIFF/AVI validation, bounded `avih` dimensions and frame timing, `strh` stream type/codec/rate/duration/frame bounds, video `strf` bitmap properties, audio `strf` format properties, and common `LIST/INFO` text fields without decoding media frames |
| MKV/WebM | EBML signature and document-type detection, bounded `Info`/`Tracks`/`Tags`/`Chapters`/`Cues`/`Attachments` scanning, typed duration, track, title, codec, chapter, cue, and attachment-descriptor values, without decoding clusters or loading attachment payloads |
| Output | Human-readable text, JSON, JSON Lines, CSV, TOML, or YAML; schema version `1` is retained in structured output |
| Batch | Deterministic path ordering with bounded parallel inspection through `--jobs N`; human, JSON Lines, and CSV modes stream results with a bounded out-of-order buffer |
| Safety | Checked offsets, bounded reads, recursion and entry limits, deterministic recursive traversal, safe XML entity handling, structured warnings, and platform-aware atomic replacement after output validation |

Generic writing, creation, PSD/PSB/RAW/MKV/WebM writing, SVG embedded-XMP extraction, MakerNote tag
interpretation beyond the bounded Nikon Type 2 and Canon IFD fields, and full media and
ExifTool compatibility are intentionally not advertised as implemented yet.
The library now supports validated, lossless
JPEG comment, bounded APP1 XMP, and selected IPTC-IIM datasets in Photoshop
APP13 resources, PNG `tEXt` and uncompressed `iTXt` XMP, GIF comments, WebP XMP, SVG
title/description/comments, WAV `LIST/INFO`, FLAC Vorbis Comment, bounded Ogg
Vorbis/Opus/Ogg-FLAC comment rewrites (including mapping packets), and common ID3v2 text/comment frames, plus existing TIFF/BigTIFF ASCII and ISO-BMFF
QuickTime text values through format-specific rewrite APIs, and the CLI
exposes the same narrow operations through `--set`, `--delete`, and `--copy`.
TIFF ASCII values can also be copied from a TIFF-like source into an existing
TIFF ASCII field when the target field has enough storage.
Existing ISO-BMFF text values can be copied between supported ISO-BMFF files
when the target value slot has enough storage.
Repeated IPTC datasets remain typed arrays when read; `--copy` accepts only a
single-valued source dataset, while `--set` replaces all target occurrences
with one bounded dataset.
MakerNotes remain partial outside the bounded Nikon Type 2 and Canon IFD fields; MP3/ID3,
Ogg, PDF, WAV, and FLAC remain only partially covered outside their explicit
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
cargo run -- --copy TIFF:EXIF:Make=source.tif target.tif
cargo run -- --set 'ISOBMFF:Title=reviewed' movie.mp4
cargo run -- --copy ISOBMFF:Title=source.mp4 target.mp4
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

The same detection and format dispatch is available for in-memory or custom
seekable readers through `metra::read_from` and
`metra::read_from_with_limits`; the caller supplies a diagnostic path and
declared byte length in `FileInfo`.

For collections, `metra::read_many` returns deterministic results with bounded
worker concurrency, while `metra::read_many_streaming` emits results in input
order with bounded backpressure for large batches.
Long-running callers can pass a `CancellationToken` to the corresponding
`*_with_cancellation` helpers; the CLI maps Ctrl+C to cooperative cancellation
and exits with status 130 after bounded in-flight reads finish.

The model keeps namespaces explicit (`EXIF`, `GPS`, `PNG`, `WebP`, `Ogg`, `JFIF`,
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
is intentionally partial and can be expanded by editing the versioned source
data and rebuilding. IPTC-IIM
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
crates/metra-core           model, generated tag catalog, limits, and structured errors
crates/metra-formats        signature detection and format readers
```

Each reader receives a `FileInfo`, a `Read + Seek` source, and explicit
`ParseLimits`. It can add typed tags and non-fatal warnings to the shared model.
The TIFF reader uses checked arithmetic and random access, so a large container
does not need to be copied wholesale into memory. The JPEG, PNG, and WebP
readers only materialize bounded metadata chunks.

The tag catalog is maintained as tab-separated source data in
`crates/metra-core/data/tag-definitions.tsv`; `metra-core/build.rs` validates
its fields and identifiers, then generates the compact Rust lookup table at
build time. Additional manufacturer-specific MakerNote readers, deeper media
metadata support, and a generalized rewrite capability layer remain deferred.
The current
format-specific writers remain intentionally narrow and independently tested.

## Validation

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo bench --bench throughput --no-run
```

The tests generate small synthetic files at runtime, covering signatures,
little-endian nested EXIF, malformed offsets, JPEG segments, structured XMP,
IPTC/ICC resources, PNG CRC behavior, WebP dimensions, GIF extensions, ISO-BMFF
boxes, PSD/PSB headers and image resources, RAW container delegation, AVI RIFF lists, Matroska/WebM EBML
elements, Ogg pages and Vorbis/Opus comments, bounded PNG zlib expansion, legacy RAW signatures, and CLI JSON/human output. Real-world corpus and differential
compatibility tests are separate follow-up gates; passing these local tests does
not claim complete ExifTool compatibility.

The opt-in corpus checks live in [`tests/corpus.rs`](tests/corpus.rs) and require
an explicit local corpus and oracle; no corpus is bundled in the repository. A
local 194-file run completed without panics: 89 files were recognized, with
3,008 Metra tags compared to the oracle, 1,837 stable-key matches, and 1,416
typed-value matches. The corpus axis remains conservative because 105 files
were outside the current format surface and the differential test is opt-in.

For batch output, human-readable, JSON Lines, and CSV modes render as results
arrive while preserving deterministic input-path order. The parallel streaming
path uses a bounded in-flight work window so a slow early file cannot retain an
unbounded completed-result map. Ctrl+C requests cooperative cancellation;
bounded in-flight reads finish, remaining paths are reported as cancelled, and
the CLI exits with status 130. JSON, TOML, and YAML
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

The Rust benchmark target `throughput` covers a bounded stream read and an
optional real-corpus pass. Set `METRA_BENCH_CORPUS` to a reviewed local corpus
to measure collection throughput; the corpus is never required to build or
test Metra and benchmark numbers remain machine-specific.

## Roadmap

Avancement global vérifié : **88 %**. Ce chiffre est une moyenne indicative des
huit axes ci-dessous, arrondie à partir de 87,8 %, calculée uniquement sur le code
et les tests présents ; il ne représente pas un pourcentage de compatibilité ExifTool.

1. **99 %** — Étendre le modèle de lecture et les définitions de tags sans perdre les données brutes ; le parseur TIFF couvre maintenant les en-têtes classic et BigTIFF, les offsets/compteurs 64 bits et les valeurs LONG8/SLONG8/IFD8, les offsets multiples `SubIFDs` et les chaînes IFD1/IFD2 de thumbnails restent groupés explicitement sans décoder les pixels, tandis que les dérivés GPS valident les références, les plages et les conversions altitude/direction/temps/vitesse, les datasets IPTC-IIM lus conservent leur identifiant numérique stable, `EXIF:UserComment` décode les préfixes ASCII/Unicode sans perdre les octets bruts, les tags TIFF/EXIF d’image, de sensibilité, de capture et d’objectif ont des noms canoniques, les champs Nikon bornés sont résolus par le catalogue partagé, les profils ICC/XMP autonomes réutilisent le modèle typé, Ogg expose des commentaires Vorbis/Opus et des champs FLAC `STREAMINFO` typés sous namespace explicite, PNG expose son `IHDR` sous forme de tags typés bornés, WAV expose les feuilles iXML et les tags ID3 embarqués sous limites strictes, et le catalogue de tags est généré à la compilation depuis une source versionnée avec contrôle des doublons.
2. **52 %** — Ajouter des corpus réels et des tests différentiels JPEG/TIFF/PNG/WebP/Ogg ; le harnais opt-in a été exécuté sur un corpus local de 194 fichiers sans panic, avec 89 fichiers reconnus, 3 008 tags Metra, 1 837 clés et 1 416 valeurs typées alignées, mais 105 fichiers restent hors surface et aucune preuve n’est embarquée dans le dépôt.
3. **99 %** — Approfondir HEIF/AVIF et les conteneurs média, puis couvrir les lecteurs restants ; les lecteurs ISO-BMFF exposent maintenant les timings `mvhd` et les identifiants/dimensions de pistes `tkhd` en plus des propriétés image bornées courantes, WebP lit les dimensions des bitstreams VP8X, VP8 et VP8L sans décoder les pixels, les lecteurs XMP/ICC autonomes et Ogg/Vorbis/Opus sont disponibles avec détection de signature bornée, FLAC et Ogg-FLAC exposent maintenant `STREAMINFO`, `SEEKTABLE` et `CUESHEET` sous forme structurée, WAV décode maintenant les feuilles iXML et délègue les chunks ID3v2 au parseur borné sans activer les entités externes, AVI expose maintenant les descripteurs `strh` et `strf` bornés sans décoder les frames, les conteneurs Matroska/WebM exposent maintenant chapitres, cues et descripteurs de pièces jointes sans charger leurs payloads, les conteneurs RAW hérités CRW/MRW/X3F sont identifiés explicitement, tandis que les lecteurs PSD/PSB et RAW couvrent leurs en-têtes et métadonnées courantes sans décoder les pixels ou les flux vidéo.
4. **98 %** — Étendre XMP/IPTC/ICC/ID3 et isoler les espaces MakerNote ; XMP est maintenant réécrit de façon bornée pour JPEG APP1, WebP et PNG, les datasets IPTC-IIM connus peuvent être réécrits dans les ressources Photoshop APP13, les profils ICC fragmentés JPEG, PNG `iCCP` et WebP `ICCP` sont inspectés sous limites avec descriptions texte et valeurs XYZ courantes, les références XML sûres sont décodées sans entités personnalisées, les textes PNG compressés sont déployés sous budget, les champs texte/commentaires ID3v2 courants restent sous limites explicites, et les conteneurs MakerNote courants sont identifiés ; des IFD Nikon Type 2 et Canon bornés exposent maintenant leurs champs connus et conservent les valeurs inconnues décodables, avec offsets de source absolus testés.
5. **74 %** — Concevoir l’écriture read-modify-write avec validation et remplacement atomique ; onze writers bornés couvrent maintenant JPEG, TIFF/BigTIFF, PNG, GIF, WebP, SVG, WAV, FLAC, Ogg Vorbis/Opus/Ogg-FLAC, ID3v2 et les champs texte ISO-BMFF existants, et les budgets metadata/valeur sont configurables depuis le CLI.
6. **98 %** — Ajouter `set`/`delete`/`copy` et comparer après les tests round-trip ; les opérations couvrent maintenant JPEG `Comment`/`XMP` et datasets IPTC-IIM connus, PNG `tEXt`/`XMP`, GIF `Comment`, WebP `XMP`, SVG `Title`/`Description`/`Comment`, WAV `LIST/INFO`, FLAC et Ogg Vorbis/Opus/Ogg-FLAC Comments, ID3v2 texte/commentaire, les champs texte ISO-BMFF existants et la copie de champs ASCII TIFF existants via API et CLI, avec comparaison déterministe des valeurs.
7. **82 %** — Ajouter le traitement parallèle contrôlé et le rendu en flux borné ; le scheduler est partagé par l’API Rust et le CLI, conserve l’ordre déterministe, borne les workers et la fenêtre de résultats hors ordre, applique une contre-pression au flux parallèle et gère l’annulation coopérative Ctrl+C avec le code 130. Le benchmark réel du corpus mesure environ 20,6 MiB/s en séquentiel, 106 MiB/s avec quatre workers et 89 MiB/s en streaming borné sur cette machine ; les baselines multi-plateformes et le profiling restent à faire.
8. **100 %** — Étendre les sorties structurées avec CSV, TOML et YAML versionnés.

## License

Metra is distributed under the MIT license; see [`LICENSE`](LICENSE).
