# Security and hostile-input policy

Metra treats every inspected file as untrusted input. Rust memory safety does
not by itself prevent logical resource exhaustion, so readers enforce limits
before allocating or traversing metadata.

## Current protections

- magic-byte detection does not trust filename extensions;
- all TIFF offsets and lengths use checked arithmetic and region bounds;
- thumbnail IFD offsets and lengths are range-checked before the referenced
  bytes are described, and thumbnail payloads are never decoded by the reader;
- `SubIFDs` offset arrays are materialized only within the configured value
  budget and each referenced directory still passes cycle/depth/range checks;
- IFD entry counts, nested depth, JPEG segments, PNG chunks, and WebP chunks
  are capped;
- large values are reported and omitted rather than allocated;
- compressed PNG text is decompressed incrementally and rejected when its
  output exceeds `max_value_bytes`;
- compressed PNG `iCCP` profiles use the same bounded zlib path before ICC
  header and tag-table inspection;
- JPEG ICC fragments are sequence-checked, deduplicated, and reassembled only
  within the configured profile budget;
- direct WebP ICC payloads are parsed only after the same profile-size limit is
  checked;
- Ogg page tables, logical streams, packets, and Vorbis/Opus comment counts are
  bounded before payload materialization;
- CRCs on inspected Ogg metadata pages are checked and reported as recoverable
  warnings, while pages outside the metadata budget are skipped;
- derived GPS coordinates, altitude, direction, speed, and time values reject
  non-finite rationals and out-of-range references or units before conversion;
- typed EXIF/GPS date and time values are materialized only after calendar,
  clock, denominator, and fractional-range validation;
- malformed embedded EXIF can be downgraded to a warning at the container
  boundary;
- recursive CLI traversal uses directory entry file types and does not follow
  symlink directories;
- XML readers accept predefined and numeric character references only; custom
  entities and DOCTYPE declarations are rejected;
- no parser invokes Perl, Python, Node, or another external metadata process.

## Limits

The default `ParseLimits` values are intentionally conservative for a first
read-first release. Library callers can select stricter values for untrusted
batch jobs. Any new decompression or XML implementation must add its own
expansion and nesting limits before being enabled.

## Current rewrite safety

Only JPEG comment, existing APP1 EXIF ASCII slots, bounded APP1 XMP, and known IPTC-IIM datasets in Photoshop
APP13 resources, PNG `tEXt` and uncompressed `iTXt` XMP, GIF comments, WebP XMP, SVG
title/description/comments, WAV `LIST/INFO`, FLAC Vorbis Comment, Ogg Vorbis/Opus/Ogg-FLAC
comment packet rewrites, common
ID3v2 text/comment replacement/deletion/copy, bounded TIFF ASCII copy,
existing ISO-BMFF text replacement/copy, bounded JPEG APP6 GoPro `DEVC`/`STRM`
records, and bounded Nikon/Canon/Fujifilm/Panasonic/Olympus/Sony/Apple/Pentax/DJI MakerNote IFD plus Samsung STMN fields
inspection are implemented, through the
library API and the explicit `--set`/`--delete`/`--copy` CLI flags. JPEG XMP
writes validate replacement packets with the bounded XML reader. IPTC writes
validate the dataset allowlist, NUL-free values, resource sizes, and APP13
segment limits; unrelated Photoshop resources are preserved. SVG replacement
values are XML-escaped, and comment writes reject `--` and a trailing `-` so
the resulting document remains valid XML.
JPEG EXIF ASCII writes validate the existing TIFF entry, type, count, offset,
capacity, and patch range; they never create a missing field or resize the APP1
segment, and the result is re-read before atomic replacement.
PDF Info writes are limited to existing literal or hexadecimal string tokens and
require the replacement to have the same encoded byte length. They do not create
objects or rewrite xref offsets, so the rest of the PDF byte layout is copied
unchanged and the temporary output is re-read before replacement.
PSD XMP writes are limited to an existing `8BIM` XMP image resource and require
an equal packet length. Resource headers, section boundaries, image data, and
unknown resources are copied unchanged; the temporary PSD is re-read before
atomic replacement.
AVI INFO writes are limited to existing known text chunks and bounded payloads;
the replacement is zero-padded within the original chunk, and deletion clears
the same payload, so RIFF sizes and media bytes remain unchanged before the
validated atomic replacement.
Matroska/WebM writes are limited to existing `Info` title/app and `SimpleTag`
string payloads and zero-pad within the original EBML element; ISO-BMFF text
deletion similarly zero-fills only the existing value span. Element/box sizes,
names, and media bytes remain unchanged before the validated atomic replacement.
RAW TIFF-like writes are limited to existing TIFF/BigTIFF ASCII slots; set and
delete operations only replace or zero-fill those slots, so the payload layout
remains unchanged. CR3 writes are limited to existing ISO-BMFF text slots after
the RAW reader has identified the container; both paths re-read the result
before replacement. RAF, CRW, MRW, and X3F are rejected before any write is
attempted.
Ogg rewrites retain page boundaries, recalculate CRCs, preserve opaque packet
bytes, and refuse packet growth unless the existing bounded packet can hold it.
ID3v2
unsynchronization, extended headers, and footers are rejected by the writer
until their round-trip handling is implemented.
Each writer, including the registry-level `FormatHandler::write_metadata`
dispatch and `copy_metadata_path`, reads and validates the source first,
copies the container through a same-directory temporary file, syncs and
re-reads the output, preserves source permissions, and replaces the original
only after validation. Failures remove the temporary file and leave the source
untouched. Generic tag writes and other formats remain disabled until their
round-trip and recovery tests exist.

The TIFF creation API is bounded separately from read-modify-write: it accepts
only an allowlisted set of EXIF ASCII tags, rejects NUL bytes and duplicates,
enforces metadata/value limits, emits a fixed 1x1 seed image, and validates the
result through the TIFF reader before returning it. Its path helper refuses an
existing destination and removes its temporary file on failure.

JPEG creation emits only a bounded SOI/metadata/EOI container. Comment and XMP
segments are size-checked, NUL-containing comments and unsafe XMP packets are
rejected, the result is re-read through the JPEG parser, and path creation uses
a same-directory temporary file with atomic no-overwrite semantics. The
container is intentionally metadata-oriented and does not claim to encode
image pixels.

PDF creation emits only a bounded document skeleton with fixed Catalog and
Pages objects plus an allowlisted Info dictionary. Values are encoded as
bounded UTF-16BE hex strings, duplicate/unknown fields and NUL bytes are
rejected, xref offsets are calculated from the generated bytes, and the result
is re-read before an atomic no-overwrite path create. No page streams or
external resources are accepted.

The PNG creation API applies the same output budget and no-overwrite rule. It
accepts only printable ASCII `tEXt` keywords, rejects NUL bytes and duplicate
keywords, compresses one fixed scanline, validates chunk CRCs through the PNG
reader, and removes its temporary file if path creation fails.

Standalone XMP creation does not synthesize or execute XML; it validates the
caller-provided packet with the bounded entity-safe XMP reader, enforces both
metadata and value budgets, and writes only after validation. Existing paths
are refused and temporary output is removed on failure.

WAV creation emits only a fixed 1x1 PCM container and validates each bounded
`LIST/INFO` key/value against the existing reader and limits. Duplicate,
unsupported, NUL-containing, and oversized fields are rejected; path creation
uses a same-directory temporary file, refuses an existing destination, and
removes the temporary output on failure.

ICC creation emits only a minimal RGB monitor profile and validates every
bounded text tag through the existing ICC reader. Names are allowlisted,
values are non-empty printable ASCII without NUL bytes, duplicate tags and
resource-limit violations are rejected, and path creation uses a
same-directory temporary file with no-overwrite and cleanup guarantees.

FLAC creation emits only a metadata-only stream with fixed `STREAMINFO` and a
bounded Vorbis Comment block. Keys are printable ASCII without `=` or NUL,
values are bounded UTF-8 without NUL, duplicate keys and oversized blocks are
rejected, and the output is re-read before an atomic no-overwrite path create.

GIF creation emits only a fixed 1x1 image and bounded comment extensions.
Comment values reject NUL bytes and are checked against the configured value
budget; sub-block framing, the LZW seed, and the trailer are fixed, and the
result is re-read before an atomic no-overwrite path create.

MP3 creation emits only a bounded ID3v2.4 tag with allowlisted text frames or
one English comment, followed by a fixed zeroed MPEG Layer III seed frame. Text
payloads, frame counts, tag size, and the total output are checked before
allocation; the result is re-read before an atomic no-overwrite path create.
This seed is metadata-oriented and does not claim to encode playable audio.

Ogg creation emits only a minimal Opus stream with bounded UTF-8 `OpusTags`
comments and fixed `OpusHead` fields. Comment keys, values, packet size, page
segmentation, page counts, CRCs, and total output are bounded before creation;
the result is re-read before an atomic no-overwrite path create. The seed does
not encode an Opus audio payload.

SVG creation emits only a fixed 1x1 XML document with optional bounded title,
description, and comment nodes. Text is checked against XML 1.0 characters and
escaped before insertion; comment delimiters, XML node limits, document size,
and the final reader validation are enforced before an atomic no-overwrite path
create.

WebP creation emits only a fixed 1x1 lossless `VP8L` seed with an optional
bounded XMP chunk. The XMP packet is parsed with the same entity-safe limits as
read operations; chunk sizes, total output, final reader validation, temporary
file cleanup, and atomic no-overwrite path creation are enforced before the
destination is committed. The seed is metadata-oriented and does not claim to
be a general WebP image encoder.

Security reports should include the smallest reproducible input and the exact
Metra version. Do not include private media or secrets in an issue.
