# Metra architecture

## Design goals

Metra is designed as a Rust library first. The binary in `src/main.rs` reads
files through the public root facade and only owns argument parsing, traversal,
bounded worker scheduling, rendering, and exit status. The same facade
dispatches `Read + Seek` streams, bounded batch scheduling, and cooperative
batch cancellation for library callers. Format code must not
shell out to an external metadata executable.

## Current boundaries

### `metra-core`

This crate owns stable, format-independent types:

- `Metadata` and `FileInfo`;
- namespace-aware `Tag` records;
- a format capability matrix with independent read/write/create/delete statuses;
- stable tag identifiers and a shared partial definition catalog generated from
  `data/tag-definitions.tsv` by the crate build script;
- canonical definitions for common EXIF image, exposure, color, and lens tags;
- typed `TagValue` variants, including rationals, arrays, bytes, and unknowns;
- structured `Warning` values;
- `MetraError` categories;
- explicit parser resource limits.

The model preserves a canonical tag name and underlying numeric identifier when
available. TIFF/EXIF numeric IDs and IPTC-IIM dataset numbers are retained by
their readers. `Metadata` exposes both first-match and repeated-match lookup so
duplicate datasets do not need to be flattened. `Metadata::diff` compares value
multisets by canonical key while ignoring source offsets and raw storage details.
Display formatting is a presentation concern and is not a lookup contract.

### `metra-formats`

This crate owns magic-byte detection and format-specific readers. Readers are
separate modules rather than one parser with format-specific branches spread
through the CLI.

The TIFF reader is the low-level building block for EXIF in JPEG, PNG, WebP,
and embedded PSD resources. It accepts a bounded random-access region, so embedded offsets remain
relative to the correct TIFF payload while source offsets can still be
reported against the containing file. Classic TIFF and BigTIFF headers use the
same checked parser with variant-specific entry widths and 64-bit values. EXIF
`UserComment` ASCII/Unicode prefixes
are decoded while the original bytes remain available. Known EXIF date-time
strings and GPS date/time rationals become structured temporal values only
after component and range validation. JPEG, PNG, and WebP delegate structured
XMP to the bounded XML reader; PNG text and `iCCP` chunks use a zlib decoder
capped by `ParseLimits`; Photoshop resources delegate IPTC IIM parsing,
and ICC payloads delegate bounded profile-header, typed illuminant, and tag-table inspection; JPEG
ICC APP2 fragments are collected and reassembled by sequence number before that inspection, while
common ICC text and XYZ table tags retain their 4CC identifiers. PNG and WebP carry their bounded
profile payloads directly. TIFF MakerNote payloads are handed to an isolated
detector. Nikon Type 1/2, Canon, Fujifilm, Panasonic, Olympus, legacy Sony, Apple,
and Pentax payloads additionally pass through bounded embedded IFD readers that
expose known fields with stable numeric identifiers and typed values; Apple
runtime binary plists are decoded into bounded structures while their raw
payloads remain attached to the parent tag. Samsung
STMN payloads expose bounded header/preview fields and preserve the nested
payload under the value budget. DJI payloads with a standard IFD expose known
fields after bounded byte-order selection. Pentax uses its Big Endian IFD and
root-relative value offsets, and structures selected packed information blocks
while preserving their raw payloads. JPEG APP6 GoPro `DEVC` records and nested
`STRM` records are parsed as bounded big-endian typed leaves, with 4-byte
alignment, raw values, stable FourCC identifiers, and explicit source offsets;
GoPro is also detected using the parsed EXIF manufacturer context, while its
proprietary MakerNote payload remains detection-only. Known and unknown values
from those bounded IFDs retain their raw bytes. TIFF also derives GPS
decimal coordinates, signed altitude, image direction, speed
in meters per second, and seconds since midnight only after validating their
rational values, units, and ranges.
The top-level TIFF IFD chain retains explicit `IFD0`, `IFD1`, and subsequent
groups, and bounded `SubIFDs` arrays become `SubIFD1`, `SubIFD2`, and so on. This
keeps embedded thumbnail/sub-image dimensions and compression metadata grouped
without loading or decoding their byte ranges; thumbnail ranges are checked
independently before they are described as valid.
The PSD reader validates PSD/PSB sections, emits typed header properties, and
delegates bounded XMP, IPTC, ICC, and embedded EXIF resources to the shared
readers. Unknown Photoshop resources remain available as bounded byte values;
layer and pixel data are skipped. The ISO-BMFF reader walks bounded boxes and
exposes brands, `mvhd` movie timing, `tkhd` track identifiers/durations/fixed-
point dimensions, image dimensions, channel depths, orientation, pixel aspect
ratio, nclx color properties, auxiliary item types, direct XMP/EXIF boxes, and
a conservative subset of QuickTime-style `ilst` text items. HEIF/AVIF property
containers (`iprp`/`ipco`) are traversed with the same recursion and payload
budgets as top-level boxes.
The WebP reader extracts dimensions from the extended `VP8X` canvas and the
native lossy `VP8 ` and lossless `VP8L` frame headers, validating each bounded
frame signature without decoding image payloads.
The AVI reader scans RIFF lists with checked boundaries, exposes typed `avih`
timing/dimension fields and common `LIST/INFO` text, and skips video payloads.
The Matroska reader validates the EBML signature and document type, walks bounded
`Info`, `Tracks`, and `Tags` elements, exposes typed duration and track fields,
and skips `Cluster` payloads without decoding media frames. WebM uses the same
bounded reader with its document type retained by format detection.
The RAW reader identifies common TIFF-like camera containers by their verified
header or path family, delegates DNG/CR2/NEF/ARW/ORF/RW2/PEF EXIF parsing to the
TIFF reader, delegates CR3 to ISO-BMFF, and decodes the bounded RAF fixed header
and Fuji directory without touching pixel payloads. Legacy Canon CRW, Minolta
MRW, and Sigma X3F remain identified as partially decoded containers. RAW
identity tags keep the container family explicit without claiming proprietary
sensor-payload support. DNG-specific IFD identifiers are catalogued in the `DNG`
namespace while their bounded raw values remain attached to the parsed tags.
The MP3 reader handles bounded ID3v2 frame tables, ID3v1 fixed fields, and a
single MPEG frame header without decoding audio payloads.
The FLAC reader validates the metadata-block chain and decodes STREAMINFO,
bounded SEEKTABLE seek-point structures, CUESHEET catalog/track/index
structures, Vorbis comments, and bounded PICTURE blocks without touching audio
frames. The AVI reader additionally decodes bounded `avih`, `strh`, and
stream-type-aware `strf` video/audio headers, deriving stream duration without
touching media chunks.
The Ogg reader walks bounded pages and logical streams, validates CRCs for pages
whose metadata bodies are inspected, and reconstructs only the
first bounded metadata packets, and decodes Vorbis identification/comments,
OpusHead, OpusTags, Ogg-FLAC mapping headers, and Ogg-FLAC Vorbis Comments
without touching coded audio frames.
The PDF reader scans bounded head/tail windows for Info dictionaries and direct
XMP packets; the WAV reader walks RIFF chunks and decodes `fmt `, `LIST/INFO`,
and Broadcast Wave `bext` fields without loading audio data. The SVG reader
parses a bounded XML document without rendering it, decodes only safe XML
character references, extracts embedded `xmpmeta` packets through the shared
bounded XMP reader, exposes root dimensions, `viewBox`, version, title,
description, and comments, and rejects DOCTYPE/entity
constructs. Batch workers use a bounded atomic work index and restore path order
before rendering.

## Parser invariants

Every untrusted size or offset must satisfy all of the following before use:

1. checked arithmetic succeeds;
2. the resulting range is inside the declared input region;
3. the allocation is within `ParseLimits`;
4. recursion, entry, chunk, or segment counters remain within their limits.

The library exposes these budgets through `ParseLimits`; the CLI can override
the total metadata and per-value budgets per invocation. Rewrite operations pass
the same limits through source parsing, transformation, and output validation.

Malformed embedded metadata is recoverable at the container layer when safe:
the reader adds a warning and retains tags already extracted from other blocks.
Standalone TIFF parsing returns a structured error for an invalid root header or
out-of-range required read.

## Rewrite boundary

The current writer surface is deliberately limited to existing TIFF/BigTIFF
ASCII value slots, existing ISO-BMFF QuickTime text item values, existing JPEG APP1
EXIF ASCII value slots, JPEG COM segments, APP1
XMP packets, and known IPTC-IIM datasets inside Photoshop APP13 resources, PNG
`tEXt` chunks and uncompressed `iTXt` XMP chunks, GIF comment extensions, WebP `XMP ` chunks,
and existing standalone XMP packets,
and existing standalone ICC `desc`/`text` payloads,
SVG title/description/comment nodes, WAV `LIST/INFO` fields, FLAC Vorbis
Comment key/value pairs, Ogg Vorbis/Opus comment packets, common ID3v2 text/comment frames,
and existing PDF Info literal or hexadecimal string tokens, existing Matroska/WebM
`Info` title/app strings and `SimpleTag` string values, and existing TIFF/BigTIFF ASCII slots in TIFF-like
RAW containers. Set and delete operations only replace or zero-fill those
existing value spans; Ogg
rewrites preserve the existing packet size and page layout, recompute page CRCs, and refuse
growth that cannot fit in the original packet; deletions use bounded Vorbis padding when
available. Ogg-FLAC comment blocks, whether embedded in the mapping packet or in a
subsequent metadata packet, use the native metadata-block header and the same
lossless packet-size rule. General packet/page restructuring remains planned;
the separate minimal Opus seed creation seam is documented below. TIFF ASCII
values can be copied from a validated TIFF-like source into an existing target
slot when the target field has enough storage. JPEG EXIF ASCII rewrites use the
existing TIFF entry type/count/offset, require a replacement that fits the
original slot, and preserve the APP1 segment size. The WebP and
PNG writers validate replacement packets with the bounded XMP parser. The
JPEG IPTC writer validates dataset names and lengths, rewrites only the target
dataset in the `0x0404` resource, preserves unrelated Photoshop resources, and
creates a bounded APP13 resource when needed.
The PDF writer accepts only existing Info Title, Author, Subject, Keywords,
Creator, Producer, CreationDate, or ModifyDate literal/hexadecimal string
tokens. It preserves the token encoding and requires an exactly equal encoded
byte span for replacements, so it never creates objects, rewrites xref tables,
or moves unrelated PDF bytes. Deletion replaces a token at least four bytes
long with a padded `null` object and the reader omits that field; shorter tokens
are rejected because they cannot preserve the layout safely.
The PSD writer accepts only an existing `8BIM` XMP image resource. Replacements
require an equal packet length; deletion zero-fills the resource payload at the
same length and the reader omits that explicit tombstone. Resource headers,
section boundaries, image data, and unknown resources are copied unchanged.
The standalone XMP writer accepts only one `XMP:Packet` replacement for an
existing packet. It validates the replacement with the bounded XML reader and
requires an exactly equal byte length, so the packet file is not resized or
restructured.
The standalone ICC writer accepts existing `Description`, `Copyright`,
`ManufacturerDescription`, and `ModelDescription` tags when their payload uses
`desc`, `text`, or `mluc` storage. Replacements fit the existing payload;
`mluc` updates only the first locale record, and deletion zero-fills the whole
payload. Profile tag-table restructuring remains deferred.
AVI creation exposes `AviCreateOptions`, `create_avi_to_vec`, `create_avi_path`,
and `--create-avi KEY=VALUE`. It emits one bounded 1x1 24-bit DIB frame,
minimal stream/index headers, and optional `LIST/INFO` text; general video
encoding and arbitrary AVI chunk creation remain outside this seam.
The AVI writer accepts existing known `LIST/INFO` string chunks, writes only
within their allocated payloads, preserves a NUL terminator when space exists,
and never changes RIFF chunk sizes or media data. Delete operations zero-fill
the selected payload so the reader no longer exposes that field.
The Matroska/WebM writer accepts existing `Info` title/app strings and
`SimpleTag` string values, writes only within their allocated EBML payloads, and
never changes element widths, tag names, or media payloads. Set operations replace
text in place; delete operations zero-fill the selected payload so the reader no
longer exposes that field without changing the EBML structure.
The RAW adapter accepts TIFF-like variants (DNG, CR2, NEF, ARW, ORF, RW2, and
PEF), delegates slot validation to the TIFF writer, and revalidates through the
RAW reader. A separate CR3 adapter requires the RAW reader to identify the
container as CR3, delegates existing ISO-BMFF text-slot validation, and then
revalidates through the RAW reader; RAF, CRW, MRW, and X3F remain unsupported for
writes.
The SVG writer validates the source XML, escapes replacement text, rejects
unsafe comment delimiters, and preserves unrelated source ranges. The ID3 writer requires a tag without
unsynchronization, extended-header, or footer flags. ID3v2.4 creation uses the
same bounded text/comment encoders and emits a minimal zeroed MPEG Layer III
frame only as a metadata seed, not as an audio encoder. Each library writer
validates its source through the reader before writing, streams the original
container while preserving untargeted bytes, validates the temporary output
with the reader again, syncs it, and replaces the original through a shared
platform-aware atomic helper. Unix uses `rename`; Windows uses `MoveFileExW`
with replace and write-through flags. The public facade also exposes deterministic `read_many` and
backpressure-bounded `read_many_streaming` helpers, plus cancellation-aware
variants; the CLI uses these same batch APIs before rendering. The CLI exposes
`--set`/`--delete`/`--copy` for
`JPEG:Comment`, `JPEG:EXIF:<ASCII tag>`, `IPTC:<dataset>`, `PNG:XMP`, `PNG:Text:<keyword>`, `SVG:Title`/`Description`/`Comment`, `WAV:<INFO field>`,
`FLAC:<Vorbis field>`, `ID3:<text field>`, `ISOBMFF:<text field>`, `PDF:<Info field>`, `GIF:Comment`, `WebP:XMP`,
`Matroska:Title`/`MuxingApp`/`WritingApp`, `Matroska:Tag:<name>`,
`TIFF:EXIF:<ASCII tag>` in TIFF-like RAW files, and existing
`TIFF:EXIF:<ASCII tag>` values; generic
tag mutation and other format writers remain deferred until their round-trip
acceptance tests exist.

The CLI also exposes `--create-tiff KEY=VALUE` and
`--create-bigtiff KEY=VALUE` for the bounded classic TIFF and BigTIFF creation
seams. Both accept repeated EXIF ASCII assignments and exactly one destination;
the destination must not already exist.

JPEG creation exposes `JpegCreateOptions`, `create_jpeg_to_vec`,
`create_jpeg_path`, and the CLI `--create-jpeg KEY=VALUE`. It emits a minimal
SOI/metadata/EOI container with optional bounded Comment and XMP segments,
validates it through the JPEG reader, and refuses to overwrite an existing
destination. It is a metadata seed, not an image encoder.

The parallel PNG seam exposes `PngCreateOptions` and `--create-png KEY=VALUE`.
It emits a 1x1 RGBA image with CRC-checked `tEXt` chunks, validates the PNG by
reading it back, and applies the same no-overwrite destination rule.

Standalone XMP creation exposes `create_xmp_to_vec` and `create_xmp_path`.
Callers provide the XML packet directly; Metra applies the same bounded XML
reader and only creates a new destination after successful validation.

PDF creation exposes `PdfCreateEntry`, `PdfCreateOptions`,
`create_pdf_to_vec`, `create_pdf_path`, and the CLI `--create-pdf KEY=VALUE`.
It emits a minimal valid PDF container with Catalog, Pages, and Info objects,
calculates the xref offsets, validates the Info dictionary through the PDF
reader, and refuses to overwrite an existing destination. Page content and
graphics remain outside this creation seam.

The WAV creation seam exposes `WavCreateOptions`, `create_wav_to_vec`,
`create_wav_path`, and `--create-wav KEY=VALUE`. It emits a fixed 1x1 PCM
RIFF/WAVE seed with optional bounded `LIST/INFO` fields, validates the output
through the WAV reader, and refuses to overwrite an existing destination.

Standalone ICC creation exposes `IccCreateOptions`, `create_icc_to_vec`,
`create_icc_path`, and the CLI `--create-icc KEY=VALUE`. It emits a minimal
RGB monitor profile with bounded ASCII text tags, validates it through the ICC
reader, and refuses to overwrite an existing destination.

FLAC creation exposes `FlacCreateOptions`, `create_flac_to_vec`,
`create_flac_path`, and the CLI `--create-flac KEY=VALUE`. It emits a
metadata-only stream with fixed `STREAMINFO` and bounded UTF-8 Vorbis comments,
validates it through the FLAC reader, and refuses to overwrite an existing
destination.

GIF creation exposes `GifCreateOptions`, `create_gif_to_vec`,
`create_gif_path`, and the CLI `--create-gif COMMENT`. It emits a fixed 1x1
GIF with bounded comment extensions and a valid one-pixel image data stream,
validates it through the GIF reader, and refuses to overwrite an existing
destination.

MP3 creation exposes `Mp3CreateOptions`, `create_mp3_to_vec`,
`create_mp3_path`, and the CLI `--create-mp3 KEY=VALUE`. It emits a bounded
ID3v2.4 tag with common text frames or one English comment, appends a fixed
minimal MPEG Layer III seed frame, validates it through the MP3 reader, and
refuses to overwrite an existing destination.

Ogg creation exposes `OggCreateOptions`, `create_ogg_to_vec`, `create_ogg_path`,
and the CLI `--create-ogg KEY=VALUE`. It emits a minimal Opus stream with
bounded `OpusHead`/`OpusTags` packets, valid page segmentation and CRCs,
validates it through the Ogg reader, and refuses to overwrite an existing
destination.

SVG creation exposes `SvgCreateOptions`, `create_svg_to_vec`,
`create_svg_path`, and the CLI `--create-svg KEY=VALUE`. It emits a bounded
1x1 XML document with optional title, description, and comments, escapes text,
validates it through the SVG reader, and refuses to overwrite an existing
destination.

WebP creation exposes `WebpCreateOptions`, `create_webp_to_vec`,
`create_webp_path`, and the CLI `--create-webp-xmp PACKET`. It emits a minimal
1x1 lossless WebP seed with an optional bounded XMP chunk, validates it through
the WebP reader, and refuses to overwrite an existing destination.

The public `CreateRequest` enum is the common creation dispatch boundary for
the validated format-specific seams. `create_to_vec` supports in-memory
staging, while `create_path` delegates to the corresponding no-overwrite
atomic path helper; the CLI remains a consumer of these same format APIs.

Legacy read queries are handled by a thin argument normalizer: selected
single-dash aliases such as `-Make` and `-GPSLatitude` become `--tag` selectors,
while `-json` and `-jsonl` become the corresponding Metra output flags. The
normalizer is deliberately outside the parser and writers, so compatibility
aliases cannot expand the format implementation surface or invoke an external
oracle.

The public `MetadataEdit` API is the common string-edit boundary for the
currently supported narrow operations. `FormatHandler::write_metadata` gives
the same dispatch a stream-oriented contract, while `rewrite_metadata_path` keeps the
safe writer contract by detecting the input first and delegating to the
format-specific atomic implementation; `rewrite_metadata_to_vec` provides the
same dispatch for callers that own the byte buffer, and `copy_metadata_path`
reads the source value before rewriting the target.
Numeric/binary mutation, new metadata block creation, and deletion semantics
that require layout changes remain intentionally outside this API. Creation
seams are intentionally format-specific: `TiffCreateOptions` can build a
minimal classic 1x1 TIFF, while `create_bigtiff_to_vec` and
`create_bigtiff_path` build the corresponding BigTIFF seed; both accept bounded
EXIF ASCII fields. `Mp3CreateOptions` and `OggCreateOptions` can build minimal
audio metadata seeds, `SvgCreateOptions` can build a minimal XML metadata seed, and
`WebpCreateOptions` can build a minimal lossless image metadata seed, while
`JpegCreateOptions` can build a minimal JPEG metadata container seed; all
validate their output by reading it back and create a new path without
overwriting an existing file.

## Output contract

JSON documents include `schema_version`. The schema is intentionally small and
typed rather than a flattened map. A future schema change must either preserve
version `1` semantics or increment the version and document the migration.

For multiple files, `--json` emits an array of successful metadata documents;
`--jsonl` emits one document per successful file; `--csv` emits one row per tag
with the typed value serialized as JSON; and `--toml`/`--yaml` serialize the
same versioned model. Human-readable, JSON Lines, and CSV modes render in input
order as workers finish, retaining only out-of-order results. JSON, TOML, and
YAML intentionally aggregate successful documents because their output is one
complete document or collection. Errors are sent to stderr and produce a
non-zero exit code.
The CLI `--validate` flag additionally treats any recoverable warning as a
non-zero validation result while retaining the warning in the emitted metadata.
The CLI `--compare reference target` renders the value-level diff and returns a
non-zero code when differences or read failures are found.

## Planned seams

The following changes are deferred until their acceptance tests exist:

- additional tag definitions beyond the current generated catalog;
- a typed write/create/edit operation IR on top of the existing public
  `FormatHandler` registry;
- additional manufacturer-specific MakerNote modules beyond the bounded Nikon
  Type 1/2, Canon, Fujifilm, Panasonic, Olympus, legacy Sony, Apple, Pentax, Samsung
  STMN, and DJI readers;
- vendor-specific RAW container structures and RAF/CR3 payload metadata beyond
  the current bounded delegation;
- PSD/PSB layer/pixel metadata modules and resource types beyond existing XMP;
- deeper HEIF/AVIF and media metadata modules, including codec-specific fields;
- a generalized lossless block-preservation abstraction across more formats;

Deferring a seam is not a compatibility claim. The compatibility matrix records
the actual state for each capability.
