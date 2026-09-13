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
- stable tag identifiers and a shared partial definition catalog;
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
are decoded while the original bytes remain available. JPEG, PNG, and WebP delegate structured
XMP to the bounded XML reader; PNG text and `iCCP` chunks use a zlib decoder
capped by `ParseLimits`; Photoshop resources delegate IPTC IIM parsing,
and ICC payloads delegate bounded profile-header, typed illuminant, and tag-table inspection; JPEG
ICC APP2 fragments are collected and reassembled by sequence number before that inspection, while
common ICC text and XYZ table tags retain their 4CC identifiers. PNG and WebP carry their bounded
profile payloads directly. TIFF MakerNote payloads are handed to an isolated
detector. Nikon Type 2 and Canon payloads additionally pass through bounded
embedded IFD readers that expose known fields with stable numeric
identifiers and typed values; other vendors remain detection-only, and
proprietary MakerNote tag decoding remains separate. Known and unknown values
from those bounded IFDs retain their raw bytes. TIFF also derives GPS
decimal coordinates, signed altitude, image direction, speed
in meters per second, and seconds since midnight only after validating their
rational values, units, and ranges.
The top-level TIFF IFD chain retains explicit `IFD0`, `IFD1`, and subsequent
groups, so embedded thumbnail-directory dimensions and compression metadata can
be inspected without loading or decoding the thumbnail byte range; that range
is checked independently before it is described as valid.
The PSD reader validates PSD/PSB sections, emits typed header properties, and
delegates bounded XMP, IPTC, ICC, and embedded EXIF resources to the shared
readers. Unknown Photoshop resources remain available as bounded byte values;
layer and pixel data are skipped. The ISO-BMFF reader walks bounded boxes and
exposes brands, dimensions, channel depths, orientation, pixel aspect ratio,
nclx color properties, auxiliary item types, direct XMP/EXIF boxes, and a
conservative subset of QuickTime-style `ilst` text items. HEIF/AVIF property
containers (`iprp`/`ipco`) are traversed with the same recursion and payload
budgets as top-level boxes.
The AVI reader scans RIFF lists with checked boundaries, exposes typed `avih`
timing/dimension fields and common `LIST/INFO` text, and skips video payloads.
The Matroska reader validates the EBML signature and document type, walks bounded
`Info`, `Tracks`, and `Tags` elements, exposes typed duration and track fields,
and skips `Cluster` payloads without decoding media frames. WebM uses the same
bounded reader with its document type retained by format detection.
The RAW reader identifies common TIFF-like camera containers by their verified
header or path family, delegates DNG/CR2/NEF/ARW/ORF/RW2/PEF EXIF parsing to the
TIFF reader, delegates CR3 to ISO-BMFF, and identifies RAF, legacy Canon CRW,
Minolta MRW, and Sigma X3F as partially decoded containers. RAW identity tags
keep the container family explicit without claiming proprietary sensor-payload
support. DNG-specific IFD identifiers are catalogued in the `DNG` namespace
while their bounded raw values remain attached to the parsed tags.
The MP3 reader handles bounded ID3v2 frame tables, ID3v1 fixed fields, and a
single MPEG frame header without decoding audio payloads.
The FLAC reader validates the metadata-block chain and decodes STREAMINFO,
Vorbis comments, and bounded PICTURE blocks without touching audio frames.
The Ogg reader walks bounded pages and logical streams, reconstructs only the
first metadata packets, and decodes Vorbis identification/comments, OpusHead,
OpusTags, and Ogg-FLAC mapping headers without touching coded audio frames.
The PDF reader scans bounded head/tail windows for Info dictionaries and direct
XMP packets; the WAV reader walks RIFF chunks and decodes `fmt `, `LIST/INFO`,
and Broadcast Wave `bext` fields without loading audio data. The SVG reader
parses a bounded XML document without rendering it, decodes only safe XML
character references, exposes root dimensions,
`viewBox`, version, title, description, and comments, and rejects DOCTYPE/entity
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
ASCII value slots, existing ISO-BMFF QuickTime text item values, JPEG COM segments, APP1
XMP packets, and known IPTC-IIM datasets inside Photoshop APP13 resources, PNG
`tEXt` chunks and uncompressed `iTXt` XMP chunks, GIF comment extensions, WebP `XMP ` chunks,
SVG title/description/comment nodes, WAV `LIST/INFO` fields, FLAC Vorbis
Comment key/value pairs, Ogg Vorbis/Opus comment packets, and common ID3v2 text/comment frames. Ogg
rewrites preserve the existing packet size and page layout, recompute page CRCs, and refuse
growth that cannot fit in the original packet; deletions use bounded Vorbis padding when
available. Ogg-FLAC comment-block writing and new packet/page creation remain planned. TIFF ASCII
values can be copied from a validated TIFF-like source into an existing target
slot when the target field has enough storage. The WebP and
PNG writers validate replacement packets with the bounded XMP parser. The
JPEG IPTC writer validates dataset names and lengths, rewrites only the target
dataset in the `0x0404` resource, preserves unrelated Photoshop resources, and
creates a bounded APP13 resource when needed.
The SVG writer validates the source XML, escapes replacement text, rejects
unsafe comment delimiters, and preserves unrelated source ranges. The ID3 writer requires a tag without
unsynchronization, extended-header, or footer flags. Each library writer
validates its source through the reader before writing, streams the original
container while preserving untargeted bytes, validates the temporary output
with the reader again, syncs it, and replaces the original through a shared
platform-aware atomic helper. Unix uses `rename`; Windows uses `MoveFileExW`
with replace and write-through flags. The public facade also exposes deterministic `read_many` and
backpressure-bounded `read_many_streaming` helpers, plus cancellation-aware
variants; the CLI uses these same batch APIs before rendering. The CLI exposes
`--set`/`--delete`/`--copy` for
`JPEG:Comment`, `IPTC:<dataset>`, `PNG:XMP`, `PNG:Text:<keyword>`, `SVG:Title`/`Description`/`Comment`, `WAV:<INFO field>`,
`FLAC:<Vorbis field>`, `ID3:<text field>`, `ISOBMFF:<text field>`, `GIF:Comment`, `WebP:XMP`, and
existing `TIFF:EXIF:<ASCII tag>` values; generic
tag mutation and other format writers remain deferred until their round-trip
acceptance tests exist.

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

- generated and expanded tag definitions migrated across all format readers;
- a `FormatHandler` capability abstraction once write/create behavior creates
  meaningful shared operations;
- additional manufacturer-specific MakerNote modules beyond the bounded Nikon
  Type 2 reader;
- vendor-specific RAW container structures and RAF/CR3 payload metadata beyond
  the current bounded delegation;
- PSD/PSB resource writers and layer/pixel metadata modules;
- deeper HEIF/AVIF and media metadata modules, including codec-specific fields;
- a generalized lossless block-preservation abstraction across more formats;

Deferring a seam is not a compatibility claim. The compatibility matrix records
the actual state for each capability.
