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

Security reports should include the smallest reproducible input and the exact
Metra version. Do not include private media or secrets in an issue.
