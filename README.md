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
| JPEG | Magic-byte detection, segment walking, JFIF properties, JPEG comments, EXIF APP1, structured XMP, reassembled typed ICC profiles with common table values, IPTC resources from Photoshop blocks, bounded GoPro APP6 `DEVC`/`STRM` fields, and minimal metadata-container seed creation |
| TIFF/EXIF | Little- and big-endian classic TIFF and BigTIFF headers, 64-bit IFD counts/offsets, nested EXIF/GPS/Interop directories, bounded multiple `SubIFD` offsets, chained IFD0/IFD1/IFD2 thumbnail directories, common image/exposure/lens tag names, rational values, typed EXIF date-time and GPS date/time values, ASCII/Unicode `UserComment`, common MakerNote container detection with bounded Nikon Type 1/2, Canon, Fujifilm, Panasonic, Olympus, legacy Sony, and Apple IFD fields, bounded Apple runtime binary-plist structures, Samsung STMN header/preview fields and DJI IFD fields, GoPro family detection, retained unknown MakerNote values, unknown tags, thumbnail range checks, and validated decimal GPS latitude/longitude, altitude, direction, time, and speed helpers, safe bounded ASCII deletion, and minimal 1×1 TIFF creation with EXIF ASCII seeds |
| PNG | Chunk walking, IHDR dimensions and encoding parameters, CRC warnings, tEXt/zTXt/iTXt including bounded zlib text, eXIf, tIME, pHYs, structured XMP, bounded ICC profile headers from `iCCP`, and minimal 1×1 RGBA creation with bounded `tEXt` seeds |
| WebP | RIFF chunk walking, VP8X/VP8/VP8L dimensions, EXIF, structured XMP, typed ICC profiles, and minimal 1x1 lossless seed creation with bounded XMP |
| GIF | GIF87a/GIF89a headers, logical-screen dimensions, comments, bounded extension validation, and minimal 1x1 creation with comment extensions |
| ISO-BMFF | HEIF/AVIF/MP4/MOV/M4A brand detection, bounded box walking, `mvhd` movie timing, `tkhd` track IDs/durations/dimensions, `ispe` dimensions, `pixi` channels, `irot`/`imir` orientation, `pasp` aspect ratio, `colr` nclx values, `auxC` auxiliary type, direct XMP/EXIF, QuickTime-style `ilst` text metadata, and validated in-place edits for existing text values |
| MP3 | ID3v2.2/v2.3/v2.4 text, comments, lyrics, attached-picture metadata, ID3v1 fallback, first MPEG frame properties, and minimal ID3v2.4 seed creation |
| FLAC | `STREAMINFO`, bounded `SEEKTABLE` seek-point and `CUESHEET` track/index structures, Vorbis comments, embedded-picture properties/data, bounded metadata-block validation, and minimal metadata-only creation with Vorbis comments |
| Ogg/Vorbis/Opus | Bounded Ogg page walking with metadata-page CRC warnings, Vorbis and Opus stream headers, Vorbis Comments/OpusTags, Ogg-FLAC comments, typed FLAC-in-Ogg `STREAMINFO` fields, and minimal Opus seed creation with bounded comments |
| PDF | Header/version, bounded Info dictionaries, PDF string decoding, embedded XMP packets when directly available, and minimal xref-valid document creation with Info fields |
| WAV | RIFF/WAVE chunks, `fmt ` audio properties, `LIST/INFO`, Broadcast Wave `bext`, bounded iXML XML leaves and packet retention, embedded ID3v2 delegation, bounded validation, and minimal 1x1 PCM creation with bounded `LIST/INFO` seeds |
| SVG | Bounded XML detection, root dimensions/version/viewBox, title, description, comments, embedded XMP extraction, nesting/text limits, safe document-text rewrites, and minimal 1x1 metadata-seed creation |
| Standalone XMP/ICC | Signature-based standalone XMP packet and ICC profile readers reuse the bounded XML/profile engines and retain the detected file family; standalone XMP packets and existing ICC text tags can be rewritten within fixed storage, and minimal RGB ICC profiles can also be created and validated as new files |
| PSD/PSB | Big-endian header and dimensions, bounded Photoshop image resources, XMP/IPTC/ICC/embedded EXIF delegation, resolution and common resource fields, preservation of unknown resources as bytes, and bounded replacement of existing PSD XMP resources |
| RAW | DNG and TIFF-like CR2/NEF/ARW/ORF/RW2/PEF containers reuse the bounded TIFF/EXIF reader with common DNG tags (version, CFA, levels, matrices, white balance, and camera/lens identity) and safe rewrites of existing TIFF ASCII slots; a bounded 1x1 DNG/TIFF-like seed can be created with DNGVersion and EXIF ASCII fields; CR3 reuses ISO-BMFF inspection and supports bounded rewrites of existing ISO-BMFF text slots; RAF, legacy Canon CRW, Minolta MRW, and Sigma X3F containers are identified with explicit partial-decoding warnings and remain read-only |
| AVI | RIFF/AVI validation, bounded `avih` dimensions and frame timing, `strh` stream type/codec/rate/duration/frame bounds, video `strf` bitmap properties, audio `strf` format properties, common `LIST/INFO` text fields without decoding media frames, safe rewrites of existing `LIST/INFO` values, and a minimal 1x1 uncompressed-video seed creator |
| MKV/WebM | EBML signature and document-type detection, bounded `Info`/`Tracks`/`Tags`/`Chapters`/`Cues`/`Attachments` scanning, typed duration, track, title, codec, chapter, cue, and attachment-descriptor values, without decoding clusters or loading attachment payloads, plus safe rewrites of existing `Info` title/app strings and `SimpleTag` strings |
| Output | Human-readable text, JSON, JSON Lines, CSV, TOML, or YAML; schema version `1` is retained in structured output |
| Batch | Deterministic path ordering with bounded parallel inspection through `--jobs N`; human, JSON Lines, and CSV modes stream results with a bounded out-of-order buffer |
| Safety | Checked offsets, bounded reads, recursion and entry limits, deterministic recursive traversal, safe XML entity handling, structured warnings, and platform-aware atomic replacement after output validation |

Broader creation and full PSD/PSB/RAW writing, MakerNote tag
interpretation beyond the bounded Nikon Type 1/2, Canon, Fujifilm, Panasonic, Olympus, legacy Sony, Apple, Pentax, Samsung, and DJI fields, and full media and
complete ExifTool compatibility are intentionally not advertised as implemented yet.
The compatibility layer currently translates a bounded set of legacy query
aliases (`-json`, `-jsonl`, `-Make`, `-Model`, `-Artist`, `-Copyright`,
`-Software`, `-ImageDescription`, `-GPSLatitude`, `-GPSLongitude`,
`-GPSAltitude`, and `-DateTimeOriginal`) into canonical Metra selection and
output; the resulting structured output keeps Metra schema version `1`. A
standalone XMP packet can also be created from caller-supplied XML after the
same bounded parser validation.
An existing standalone XMP packet can be replaced through the validated
equal-byte-length `XMP:Packet` rewrite seam.
`--delete XMP:Packet` clears the parsed XMP properties while retaining a
same-length, valid empty RDF packet envelope; the structural `XMP:Packet` tag
therefore remains present by design.
Existing standalone ICC `Description`, `Copyright`, `ManufacturerDescription`,
and `ModelDescription` text tags can be rewritten or cleared within their
allocated profile payloads.
The library now supports validated, lossless
JPEG comment, existing APP1 EXIF ASCII fields, bounded APP1 XMP, and selected IPTC-IIM datasets in Photoshop
APP13 resources, PNG `tEXt` and uncompressed `iTXt` XMP, GIF comments, WebP XMP, SVG
title/description/comments, WAV `LIST/INFO`, FLAC Vorbis Comment, bounded Ogg
Vorbis/Opus/Ogg-FLAC comment rewrites (including mapping packets), and common ID3v2 text/comment frames, plus existing TIFF/BigTIFF ASCII and ISO-BMFF
QuickTime text values, existing PDF Info string tokens, and existing PSD XMP
resources, existing AVI `LIST/INFO` strings, Matroska/WebM `Info` title/app and
`SimpleTag` strings,
and TIFF ASCII slots in TIFF-like RAW files through format-specific rewrite APIs,
and the CLI exposes the same narrow operations through `--set`,
`--delete`, and `--copy`.
TIFF ASCII values can also be copied from a TIFF-like source into an existing
TIFF ASCII field when the target field has enough storage.
Existing ISO-BMFF text values can be cleared or copied between supported
ISO-BMFF files when the target value slot has enough storage; clearing zero-fills
the existing slot without changing box sizes.
Existing JPEG EXIF ASCII fields can be rewritten or copied when the target field
has enough storage; the JPEG segment size and image bytes remain unchanged.
Repeated IPTC datasets remain typed arrays when read; `--copy` accepts only a
single-valued source dataset, while `--set` replaces all target occurrences
with one bounded dataset.
The public `MetadataEdit::set`/`delete` operations,
`rewrite_metadata_path`/`rewrite_metadata_to_vec`, and
`copy_metadata_path` helpers provide the same
canonical-key surface without requiring callers to depend on a format-specific
writer enum. They dispatch only to the validated writers available for the
detected format; typed numeric/binary mutation and creation for other formats
remain deferred. `TiffCreateOptions` and `create_tiff_to_vec`/
`create_tiff_path` provide the first bounded metadata-seed creation API;
the public `CreateRequest` enum with `create_to_vec`/`create_path` provides a
typed generic dispatch over all currently available creation seams;
`JpegCreateOptions` and `create_jpeg_to_vec`/`create_jpeg_path` provide a
minimal SOI/metadata/EOI JPEG container seed with bounded Comment and XMP.
`PdfCreateEntry`/`PdfCreateOptions` and `create_pdf_to_vec`/`create_pdf_path`
provide a minimal xref-valid PDF document with bounded Info fields, without
creating page content.
`PsdCreateOptions` and `create_psd_to_vec`/`create_psd_path` provide a minimal
1x1 RGB PSD seed with an optional bounded XMP image resource; PSB and full
layer/pixel authoring remain outside this seam.
`DngCreateOptions` and `create_dng_to_vec`/`create_dng_path` provide a bounded
1x1 DNG/TIFF-like RAW seed with a DNGVersion IFD and EXIF ASCII fields;
proprietary camera RAW encoding and full RAW authoring remain outside this seam.
`IsobmffCreateKind`, `IsobmffCreateOptions` and
`create_isobmff_to_vec`/`create_isobmff_path` provide metadata-only MP4, MOV,
M4A, HEIF, and AVIF seeds. MP4/MOV/M4A accept bounded QuickTime text items;
HEIF/AVIF accept bounded image dimensions. Tracks, samples, item locations, and
media encoding remain outside this seam.
`XmpEdit` and `rewrite_xmp`/`rewrite_xmp_path` provide bounded replacement or
property clearing for an existing standalone XMP packet, requiring an equal
byte length and re-reading the result before replacement. Clearing retains the
valid packet envelope and removes parsed properties.
`IccCreateOptions` and `create_icc_to_vec`/`create_icc_path` provide the
standalone ICC creation seam.
`IccEdit` and `rewrite_icc`/`rewrite_icc_path` provide bounded replacement and
deletion for existing ICC text tags stored as `desc`, `text`, or `mluc`
payloads; `mluc` replacement updates the first locale record when its UTF-16
value fits the existing allocation.
`AviCreateOptions` and `create_avi_to_vec`/`create_avi_path` provide a minimal
1x1 uncompressed-video AVI seed with bounded `LIST/INFO` fields.
`MatroskaCreateKind`, `MatroskaCreateOptions` and
`create_matroska_to_vec`/`create_matroska_path` provide bounded MKV or WebM
metadata seeds with `Info` title/app fields and `SimpleTag` strings; they do not
encode tracks, clusters, or media payloads.
`FlacCreateOptions` and `create_flac_to_vec`/`create_flac_path` provide a
metadata-only FLAC creation seam with bounded UTF-8 Vorbis comments.
`GifCreateOptions` and `create_gif_to_vec`/`create_gif_path` provide a minimal
1x1 GIF creation seam with bounded comment extensions.
`Mp3CreateOptions` and `create_mp3_to_vec`/`create_mp3_path` provide a minimal
ID3v2.4 MP3 metadata-seed creation seam with bounded text frames and comments.
`OggCreateOptions` and `create_ogg_to_vec`/`create_ogg_path` provide a minimal
Ogg Opus metadata-seed creation seam with bounded Vorbis-style comments.
`SvgCreateOptions` and `create_svg_to_vec`/`create_svg_path` provide a minimal
1x1 SVG metadata-seed creation seam with bounded title, description, and comments.
`WebpCreateOptions` and `create_webp_to_vec`/`create_webp_path` provide a minimal
1x1 lossless WebP metadata-seed creation seam with optional bounded XMP.
MakerNotes remain partial outside the bounded Nikon Type 1/2, Canon, Fujifilm, Panasonic, Olympus, legacy Sony, Apple, Pentax, Samsung STMN, and DJI fields; GoPro APP6 `DEVC`/`STRM` fields are decoded, while its proprietary MakerNote payload remains detection-only; MP3/ID3,
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
cargo run -- --create-tiff 'EXIF:Make=Metra' --create-tiff 'EXIF:Artist=Othmane' new.tif
cargo run -- --create-dng 'DNG:Make=Metra' --create-dng 'Artist=Othmane' new.dng
cargo run -- --create-jpeg 'Comment=Metra' --create-jpeg 'XMP=<x:xmpmeta><rdf:RDF/></x:xmpmeta>' new.jpg
cargo run -- --create-pdf 'Title=Metra' --create-pdf 'Author=Othmane' new.pdf
cargo run -- --create-psd 'PSD:XMP=<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF/></x:xmpmeta>' new.psd
cargo run -- --create-mp4 'ISOBMFF:Title=Metra' --create-mp4 'Artist=Othmane' new.mp4
cargo run -- --create-mov 'Title=Metra' new.mov
cargo run -- --create-m4a 'Album=Metra' new.m4a
cargo run -- --create-heif 'ISOBMFF:ImageWidth=1920' --create-heif 'ImageHeight=1080' new.heic
cargo run -- --create-avif 'ImageWidth=1920' --create-avif 'ImageHeight=1080' new.avif
cargo run -- --create-png 'Comment=Metra' --create-png 'Author=Othmane' new.png
cargo run -- --create-xmp '<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF/></x:xmpmeta>' new.xmp
cargo run -- --create-wav 'Title=Metra' --create-wav 'Artist=Othmane' new.wav
cargo run -- --create-icc 'Description=Metra sRGB' --create-icc 'Copyright=Othmane' new.icc
cargo run -- --create-avi 'AVI:Title=Metra' --create-avi 'Software=Metra' new.avi
cargo run -- --create-mkv 'Matroska:Title=Metra' --create-mkv 'Matroska:Tag:TITLE=Metra' new.mkv
cargo run -- --create-webm 'WritingApp=Metra' --create-webm 'Tag:TITLE=Metra' new.webm
cargo run -- --create-flac 'TITLE=Metra' --create-flac 'ARTIST=Othmane' new.flac
cargo run -- --create-gif 'Metra' --create-gif 'Othmane' new.gif
cargo run -- --create-mp3 'Title=Metra' --create-mp3 'Artist=Othmane' --create-mp3 'Comment=reviewed' new.mp3
cargo run -- --create-ogg 'TITLE=Metra' --create-ogg 'ARTIST=Othmane' new.ogg
cargo run -- --create-svg 'Title=Metra' --create-svg 'Description=metadata seed' new.svg
cargo run -- --create-webp-xmp '<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF/></x:xmpmeta>' new.webp
cargo run -- --set 'XMP:Packet=<x:xmpmeta>...</x:xmpmeta>' packet.xmp
cargo run -- --copy XMP:Packet=source.xmp target.xmp
cargo run -- --set 'ICC:Description=reviewed' profile.icc
cargo run -- --delete ICC:Description profile.icc
cargo run -- --copy ICC:Description=source.icc target.icc
cargo run -- -Make photo.jpg
cargo run -- -json photo.jpg
cargo run -- --set 'JPEG:EXIF:Make=Sony' photo.jpg
cargo run -- --copy JPEG:EXIF:Make=source.jpg target.jpg
cargo run -- --copy TIFF:EXIF:Make=source.tif target.tif
cargo run -- --set 'ISOBMFF:Title=reviewed' movie.mp4
cargo run -- --copy ISOBMFF:Title=source.mp4 target.mp4
cargo run -- --set 'PDF:Title=reviewed' document.pdf
cargo run -- --delete PDF:Title document.pdf
cargo run -- --copy PDF:Title=source.pdf target.pdf
cargo run -- --set 'PSD:XMP=<x:xmpmeta>...</x:xmpmeta>' design.psd
cargo run -- --delete PSD:XMP design.psd
cargo run -- --copy PSD:XMP=source.psd target.psd
cargo run -- --set 'AVI:Title=reviewed' video.avi
cargo run -- --copy AVI:Title=source.avi target.avi
cargo run -- --set 'Matroska:Title=reviewed' video.webm
cargo run -- --copy Matroska:Title=source.webm target.webm
cargo run -- --set 'Matroska:Tag:TITLE=reviewed' video.webm
cargo run -- --copy Matroska:Tag:TITLE=source.webm target.webm
cargo run -- --set 'TIFF:EXIF:Make=reviewed' capture.dng
cargo run -- --copy TIFF:EXIF:Make=source.dng target.dng
cargo run -- --set 'ISOBMFF:Title=reviewed' capture.cr3
cargo run -- --copy ISOBMFF:Title=source.cr3 target.cr3
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

Standalone XMP edits target an existing `XMP:Packet`; replacements must have
the same byte length as the original packet so the file layout remains fixed.
The public `XmpEdit` API and the generic `rewrite_metadata_*`/`copy_metadata_*`
helpers expose the same bounded operation.

Standalone ICC edits target an existing text tag and preserve the profile
layout. `desc` and `text` payloads accept printable ASCII replacements that fit
their existing storage; `mluc` replacement updates the first locale record when
its UTF-16 value fits. Deletion zero-fills the payload so the tag disappears on
read; additional localized `mluc` records remain untouched.

PDF Info edits target an existing field and preserve the document byte layout;
the replacement must have the same encoded length as the original value token.
`--delete` replaces an existing sufficiently sized Info value token with a
padded `null` object, so the field disappears on read without moving xref data.
PSD XMP edits target an existing image resource and replacements require an
equal packet length so Photoshop section boundaries remain unchanged.
`--delete` zero-fills the existing XMP resource payload at the same length;
the reader treats that explicit tombstone as absent without touching image data.
PSD creation emits a minimal 1x1 RGB document with an optional XMP resource;
it does not author PSB files, layers, or general pixel content.
ISO-BMFF creation emits either a metadata-only `ftyp`/`moov` seed with an
`mvhd` clock and optional QuickTime-style text items, or a HEIF/AVIF `ftyp`/`meta`
seed with bounded `hdlr`/`pitm`/`ispe`/`pixi` properties. It deliberately
contains no tracks, samples, item locations, or encoded media and is intended
as a validated metadata seed.
AVI INFO edits target an existing text chunk and preserve the RIFF layout;
the replacement must fit its existing payload. `--delete` zero-fills the
selected payload without changing the chunk size.

AVI creation emits a minimal 1x1, 24-bit uncompressed-video seed with one DIB
frame, a small index, and optional bounded `LIST/INFO` fields. It is a metadata
seed and not a general-purpose video encoder.
Matroska/WebM edits target existing `Info` title/app strings or `SimpleTag`
strings and preserve the EBML layout; replacements fit their existing payloads,
and `--delete` zero-fills the selected payload so the field disappears on read.
The creation API and `--create-mkv`/`--create-webm` commands emit only a
validated metadata seed with a known-size EBML `Segment`; media tracks and
clusters remain outside this bounded seam.
TIFF edits target an existing TIFF/BigTIFF ASCII slot; `--delete` zero-fills
that slot without changing the IFD layout. DNG, CR2, NEF, ARW, ORF, RW2, and
PEF use the same bounded behavior. CR3 edits target existing ISO-BMFF text
slots through the same bounded rewrite rules, while RAF, CRW, MRW, and X3F
remain read-only.

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
build time. The public `FormatHandler` registry now gives each detected format
an explicit read/write contract and capability report. Additional manufacturer-specific
MakerNote readers beyond the bounded Nikon Type 1/2, Canon, Fujifilm, Panasonic,
Olympus, legacy Sony, Apple, Samsung STMN, and DJI readers, deeper media metadata support, and a generalized
rewrite/create capability layer remain deferred.
The current format-specific writers remain intentionally narrow and independently
tested; PSD XMP rewrites preserve the resource section size, AVI INFO rewrites
preserve chunk sizes, and Matroska/WebM rewrites preserve element sizes, then
validate the result through their readers.

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
3,303 Metra tags compared to the oracle, 1,857 stable-key matches, and 1,435
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

Avancement global vérifié : **90 %**. Ce chiffre est une moyenne indicative des
huit axes ci-dessous, arrondie à partir de 90,0 %, calculée uniquement sur le code
et les tests présents ; il ne représente pas un pourcentage de compatibilité ExifTool.

1. **100 %** — Étendre le modèle de lecture et les définitions de tags sans perdre les données brutes ; le parseur TIFF couvre maintenant les en-têtes classic et BigTIFF, les offsets/compteurs 64 bits et les valeurs LONG8/SLONG8/IFD8, les offsets multiples `SubIFDs` et les chaînes IFD1/IFD2 de thumbnails restent groupés explicitement sans décoder les pixels, les dates EXIF et les dates/heures GPS sont maintenant des valeurs structurées après validation des composants, tandis que les dérivés GPS valident les références, les plages et les conversions altitude/direction/temps/vitesse, les datasets IPTC-IIM lus conservent leur identifiant numérique stable, `EXIF:UserComment` décode les préfixes ASCII/Unicode sans perdre les octets bruts, les tags TIFF/EXIF d’image, de sensibilité, de capture et d’objectif ont des noms canoniques, les champs Nikon Type 1/2, Fujifilm, Panasonic, Olympus, Sony, Apple et Pentax bornés sont résolus par le catalogue partagé, Apple décode les structures binaires plist de runtime sous limites, les champs d’en-tête et de preview Samsung STMN ainsi que les champs IFD DJI sont bornés et conservés, GoPro APP6 `DEVC`/`STRM` expose ses champs bornés tandis que son MakerNote propriétaire reste non décodé, les profils ICC/XMP autonomes réutilisent le modèle typé, Ogg expose des commentaires Vorbis/Opus et des champs FLAC `STREAMINFO` typés sous namespace explicite, PNG expose son `IHDR` sous forme de tags typés bornés, WAV expose les feuilles iXML et les tags ID3 embarqués sous limites strictes, SVG extrait maintenant les paquets XMP embarqués sous limites, et le catalogue de tags est généré à la compilation depuis une source versionnée avec contrôle des doublons.
2. **52 %** — Ajouter des corpus réels et des tests différentiels JPEG/TIFF/PNG/WebP/Ogg ; le harnais opt-in a été exécuté sur un corpus local de 194 fichiers sans panic, avec 89 fichiers reconnus, 3 303 tags Metra, 1 857 clés et 1 435 valeurs typées alignées, mais 105 fichiers restent hors surface et aucune preuve n’est embarquée dans le dépôt.
3. **99 %** — Approfondir HEIF/AVIF et les conteneurs média, puis couvrir les lecteurs restants ; les lecteurs ISO-BMFF exposent maintenant les timings `mvhd` et les identifiants/dimensions de pistes `tkhd` en plus des propriétés image bornées courantes, WebP lit les dimensions des bitstreams VP8X, VP8 et VP8L sans décoder les pixels, les lecteurs XMP/ICC autonomes et Ogg/Vorbis/Opus sont disponibles avec détection de signature bornée, FLAC et Ogg-FLAC exposent maintenant `STREAMINFO`, `SEEKTABLE` et `CUESHEET` sous forme structurée, WAV décode maintenant les feuilles iXML et délègue les chunks ID3v2 au parseur borné sans activer les entités externes, AVI expose maintenant les descripteurs `strh` et `strf` bornés sans décoder les frames, les conteneurs Matroska/WebM exposent maintenant chapitres, cues et descripteurs de pièces jointes sans charger leurs payloads, les conteneurs RAW hérités CRW/MRW/X3F sont identifiés explicitement, tandis que les lecteurs PSD/PSB et RAW couvrent leurs en-têtes et métadonnées courantes sans décoder les pixels ou les flux vidéo.
4. **99 %** — Étendre XMP/IPTC/ICC/ID3 et isoler les espaces MakerNote ; XMP est maintenant réécrit de façon bornée pour JPEG APP1, WebP, PNG et les paquets autonomes `XMP:Packet`, les tags texte ICC autonomes existants (`Description`, `Copyright`, `ManufacturerDescription`, `ModelDescription`) sont réécrits dans leur stockage `desc`/`text`/`mluc` avec mise à jour bornée de la première locale `mluc`, les datasets IPTC-IIM connus peuvent être réécrits dans les ressources Photoshop APP13, les profils ICC fragmentés JPEG, PNG `iCCP` et WebP `ICCP` sont inspectés sous limites avec descriptions texte et valeurs XYZ courantes, les références XML sûres sont décodées sans entités personnalisées, les textes PNG compressés sont déployés sous budget, les champs texte/commentaires ID3v2 courants restent sous limites explicites, SVG extrait les paquets XMP embarqués avec le même parseur borné, et les conteneurs MakerNote courants sont identifiés ; des IFD Nikon Type 1/2, Canon, Fujifilm, Panasonic, Olympus, legacy Sony, Apple et Pentax bornés exposent maintenant leurs champs connus, Apple structure son runtime binary plist, Samsung STMN expose ses champs d’en-tête/preview et conserve son payload sous budget, DJI expose ses champs IFD connus avec sélection d’endianness bornée, et GoPro expose maintenant ses champs APP6 `DEVC`/`STRM` sous limites, tandis que son MakerNote propriétaire reste detection-only.
5. **89 %** — Concevoir l’écriture read-modify-write avec validation et remplacement atomique ; dix-huit writers bornés couvrent maintenant JPEG, TIFF/BigTIFF, PNG, GIF, WebP, SVG, WAV, FLAC, Ogg Vorbis/Opus/Ogg-FLAC, ID3v2, les champs texte ISO-BMFF existants (y compris CR3 après validation de sa variante RAW), les chaînes Info PDF existantes avec suppression par `null` paddé, les ressources XMP PSD existantes, les paquets XMP autonomes existants, les tags texte ICC autonomes `desc`/`text`/`mluc`, les chaînes AVI `LIST/INFO`, les chaînes `Info` et `SimpleTag` Matroska/WebM et les slots TIFF ASCII des RAW TIFF-like, avec effacement borné de slots TIFF/RAW et réécriture JPEG EXIF ASCII dans des slots existants ; des créations TIFF, PNG, XMP, WAV, ICC, FLAC, GIF, MP3/ID3v2.4, Ogg Opus, SVG, WebP lossless, JPEG metadata-container, PDF xref-valid et DNG/TIFF-like RAW minimales permettent maintenant d’amorcer des fichiers avec des métadonnées bornées ; les writers PDF, PSD et XMP autonome conservent les offsets ou la taille du paquet en exigeant une substitution de longueur encodée identique, tandis que les writers AVI, ICC, Matroska/WebM, RAW TIFF-like, DNG et CR3 conservent les tailles de chunks/éléments/slots ou la taille des payloads ; le registre `FormatHandler::write_metadata` expose ce dispatch sur flux seekable en réutilisant les mêmes validations ; les budgets metadata/valeur sont configurables depuis le CLI. La création générique et la restructuration des autres conteneurs restent planifiées.
Les créateurs AVI, PSD, ISO-BMFF, DNG et Matroska/WebM ajoutent désormais des seeds
bornés avec validation de sortie et refus d’écrasement : AVI émet une frame DIB
1x1, PSD un document RGB 1x1 avec ressource XMP optionnelle, ISO-BMFF un
`ftyp`/`moov` metadata-only pour MP4/MOV/M4A ou `ftyp`/`meta` dimensionné pour
HEIF/AVIF, DNG un conteneur TIFF-like 1x1 avec `DNGVersion` et champs EXIF
ASCII, et Matroska/WebM un `Segment` EBML metadata-only avec `Info`/`SimpleTag` ;
l’encodage vidéo général, les tracks/samples/item
locations/clusters, les calques/PSB et la création arbitraire de chunks restent
planifiés.
6. **99 %** — Ajouter `set`/`delete`/`copy` et comparer après les tests round-trip ; les opérations couvrent maintenant JPEG `Comment`/EXIF ASCII existant/`XMP` et datasets IPTC-IIM connus, PNG `tEXt`/`XMP`, GIF `Comment`, WebP `XMP`, paquets XMP autonomes `XMP:Packet` avec effacement borné de leurs propriétés, tags texte ICC autonomes, SVG `Title`/`Description`/`Comment`, WAV `LIST/INFO`, FLAC et Ogg Vorbis/Opus/Ogg-FLAC Comments, ID3v2 texte/commentaire, les champs texte ISO-BMFF existants y compris CR3 avec effacement borné, les champs Info PDF existants avec suppression de tokens Info existants, les ressources XMP PSD existantes, les chaînes AVI `LIST/INFO` avec effacement borné, les chaînes `Info` et `SimpleTag` Matroska/WebM avec effacement borné, les slots TIFF ASCII des RAW TIFF-like avec effacement borné et la copie de champs ASCII TIFF existants via API et CLI, avec comparaison déterministe des valeurs ; l’API publique ajoute aussi des opérations canoniques `MetadataEdit` qui dispatchent vers ces writers validés.
7. **82 %** — Ajouter le traitement parallèle contrôlé et le rendu en flux borné ; le scheduler est partagé par l’API Rust et le CLI, conserve l’ordre déterministe, borne les workers et la fenêtre de résultats hors ordre, applique une contre-pression au flux parallèle et gère l’annulation coopérative Ctrl+C avec le code 130. Le benchmark réel du corpus mesure environ 20,6 MiB/s en séquentiel, 106 MiB/s avec quatre workers et 89 MiB/s en streaming borné sur cette machine ; les baselines multi-plateformes et le profiling restent à faire.
8. **100 %** — Étendre les sorties structurées avec CSV, TOML et YAML versionnés.

## License

Metra is distributed under the MIT license; see [`LICENSE`](LICENSE).
