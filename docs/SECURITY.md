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
- RAF header offsets, lengths, entry counts, and proprietary directory values
  are range-checked and budgeted before inspection; RAF pixel payloads are never
  loaded by the metadata reader;
- MRW metadata-region boundaries, segment lengths/counts, PRD/WBG/RIF field
  reads, and embedded TTW TIFF regions are checked and budgeted before
  inspection; MRW image payloads are never loaded by the metadata reader;
- X3F header versions, final directory pointers, entry counts, section ranges,
  PROP UTF-16 offsets, and image-section descriptor reads are checked and
  budgeted before inspection; X3F image payloads are never loaded by the
  metadata reader;
- CRW root-directory pointers, entry counts, relative value ranges, data
  locations, recursion depth, and a global entry budget are checked before
  decoding; preview and oversized values are reported as descriptors without
  loading their payloads;
- ISO-BMFF `uuid` user types are read through a fixed 16-byte prefix; only the
  standard Adobe XMP UUID and payloads with an identifiable TIFF/Exif envelope
  are materialized under `max_value_bytes`, while unknown UUID payloads remain
  warning-only;
- typed EXIF/GPS date and time values are materialized only after calendar,
  clock, denominator, and fractional-range validation;
- malformed embedded EXIF can be downgraded to a warning at the container
  boundary;
- recursive CLI traversal uses directory entry file types and does not follow
  symlink directories;
- XML readers accept predefined and numeric character references only; custom
  entities and DOCTYPE declarations are rejected;
- no parser invokes Perl, Python, Node, or another external metadata process.
- RF64/BW64 `ds64` tables are bounded before materialization, and sentinel-sized
  audio chunks are resolved from checked 64-bit entries without loading the
  audio payload.

## Limits

The default `ParseLimits` values are intentionally conservative for a first
read-first release. Library callers can select stricter values for untrusted
batch jobs. Any new decompression or XML implementation must add its own
expansion and nesting limits before being enabled.

## Current rewrite safety

Only JPEG comment, existing APP1 EXIF ASCII slots, bounded APP1 XMP, and known IPTC-IIM datasets in Photoshop
APP13 resources, PNG `tEXt` and uncompressed `iTXt` XMP, GIF comments, WebP XMP, SVG
title/description/comments, WAV `LIST/INFO` and Broadcast Wave `bext` fields on
RIFF/RF64/BW64 containers, FLAC Vorbis Comment, Ogg Vorbis/Opus/Ogg-FLAC
comment packet rewrites, common
ID3v2 text/comment replacement/deletion/copy, bounded TIFF ASCII copy,
existing ISO-BMFF text and direct/Adobe-UUID XMP replacement/copy/deletion,
bounded JPEG APP6 GoPro `DEVC`/`STRM`
records, and bounded Nikon/Canon/Fujifilm/Panasonic/Olympus/Sony/Apple/Pentax/DJI MakerNote IFD plus Samsung STMN fields
inspection are implemented, through the
library API and the explicit `--set`/`--delete`/`--copy` CLI flags. JPEG XMP
writes validate replacement packets with the bounded XML reader. IPTC writes
validate the dataset allowlist, NUL-free values, resource sizes, and APP13
segment limits; unrelated Photoshop resources are preserved. SVG replacement
values are XML-escaped, and comment writes reject `--` and a trailing `-` so
the resulting document remains valid XML.
TIFF and JPEG EXIF date/time rewrites remain limited to existing type-2 ASCII
entries and their original allocations; the parser revalidates the typed value
after the fixed-span write.
GPS coordinate writes are limited to existing type-5 three-rational latitude or
longitude entries and matching type-2 reference fields. Inputs must be finite and
within coordinate range; DMS seconds use a bounded 1e-6 denominator, and deletion
zero-fills only existing coordinate/reference payloads. No GPS IFD or value area
is created.
GPS altitude, direction, and speed writes are likewise limited to existing
single-rational type-5 fields. Altitude accepts signed finite meters and only
updates its existing BYTE reference; direction is bounded to 0–360 degrees; speed
accepts finite non-negative m/s and converts through a validated existing K/M/N
reference. Rational numerators and denominators are bounded to u32, and deletion
zero-fills only existing scalar/reference payloads.
GPS time writes are limited to existing three-rational `GPSTimeStamp` fields;
seconds since midnight must be finite, non-negative, and strictly below 86,400,
then are rounded to a bounded microsecond denominator. Deletion zero-fills only
the existing timestamp payload.
GPS date writes are limited to existing type-2 GPSDateStamp slots and valid
four-digit-year calendar values; deletion only clears that existing allocation.
The `GPS:*` wildcard is expanded to those same bounded operations only when a
complete supported group exists; unknown or incomplete GPS structures are
preserved, and a wildcard with no supported target fails before any write.
JPEG EXIF ASCII writes validate the existing TIFF entry, type, count, offset,
capacity, and patch range; they never create a missing field or resize the APP1
segment, and the result is re-read before atomic replacement.
PDF Info writes are limited to existing literal or hexadecimal string tokens.
Replacements require the same encoded byte length; deletion requires a token at
least four bytes long and writes a padded `null` object of the same length.
Neither operation creates objects or rewrites xref offsets, so the rest of the
PDF byte layout is copied unchanged and the temporary output is re-read before
replacement.
PSD XMP writes are limited to an existing `8BIM` XMP image resource. Set
replacements require an equal packet length; deletion zero-fills only the
resource payload at that same length, and the reader recognizes the all-zero
payload as absent. Resource headers, section boundaries, image data, and unknown
resources are copied unchanged; the temporary PSD is re-read before atomic
replacement.
Standalone XMP rewrites validate exactly one replacement packet with the bounded
entity-safe XML reader and require the same byte length as the existing packet.
They do not create XML structure or resize the file; the temporary output is
re-read before atomic replacement.
Standalone ICC rewrites accept only the allowlisted text tags and existing
`desc`/`text`/`mluc` payloads. `desc` and `text` replacements are printable
ASCII and bounded by the existing allocation; `mluc` updates only the first
locale as bounded UTF-16. Zero-filled deletion never changes the tag table or
profile size. The temporary profile is re-read before atomic replacement.
AVI INFO writes are limited to existing known text chunks and bounded payloads;
the replacement is zero-padded within the original chunk, and deletion clears
the same payload, so RIFF sizes and media bytes remain unchanged before the
validated atomic replacement.
Matroska/WebM writes are limited to existing `Info` title/app and `SimpleTag`
string payloads and zero-pad within the original EBML element; ISO-BMFF text
deletion similarly zero-fills only the existing value span. ISO-BMFF XMP edits
accept only one direct `xml ` or standard Adobe XMP `uuid` packet, require an
exact byte-length replacement with a recognized XMP root, and zero-fill the
packet payload on deletion. Element/box sizes, names, UUID bytes, and media
bytes remain unchanged before the validated atomic replacement.
RAW TIFF-like writes are limited to existing TIFF/BigTIFF ASCII slots; set and
delete operations only replace or zero-fill those slots, so the payload layout
remains unchanged. CR3 writes are limited to existing ISO-BMFF text or XMP slots after
the RAW reader has identified the container; both paths re-read the result
before replacement. RAF, CRW, MRW, and X3F are rejected before any write is
attempted.
For RF64/BW64 rewrites, the writer requires a bounded first `ds64` chunk,
resolves sentinel sizes with checked 64-bit arithmetic, copies the original
audio payload without materializing it, and updates only the 64-bit RIFF size
descriptor after the temporary output has been validated. Malformed or
incomplete descriptors fail before replacement.
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
result through the TIFF reader before returning it. An optional classic-TIFF
GPS coordinate pair accepts only finite decimal latitude/longitude values within
their legal ranges, converts them to bounded DMS rationals, and requires both
coordinates before emitting a new GPS IFD. BigTIFF uses the same bounded
coordinate encoder with 64-bit IFD offsets. Optional altitude, direction, speed,
time, and date fields use finite/range-checked values, bounded unsigned
rationals, an explicit K speed reference, and strict calendar validation. DNG
creation reuses this validated TIFF seed before appending its DNGVersion IFD;
the root IFD chain offset is adjusted for the optional GPS directory. The path
helper refuses an existing destination and removes its temporary file on
failure.

The BigTIFF creation API applies the same allowlist, duplicate/NUL checks,
resource limits, reader revalidation, temporary-file cleanup, and no-overwrite
atomic path rule while emitting 8-byte IFD counts, offsets, and value slots.

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
`LIST/INFO` key/value against the existing reader and limits. Classic RIFF uses
32-bit sizes; RF64/BW64 creation writes a bounded first `ds64`, keeps the
`data` chunk sentinel, and records its checked 64-bit data and sample sizes.
Duplicate, unsupported, NUL-containing, and oversized fields are rejected;
path creation uses a same-directory temporary file, refuses an existing
destination, and removes the temporary output on failure.

ICC creation emits only a minimal RGB monitor profile and validates every
bounded text tag through the existing ICC reader. Names are allowlisted,
values are non-empty printable ASCII without NUL bytes, duplicate tags and
resource-limit violations are rejected, and path creation uses a
same-directory temporary file with no-overwrite and cleanup guarantees.

AVI creation emits only a bounded 1x1 uncompressed-video seed with fixed stream,
bitmap, frame, index, and RIFF structures. INFO names and values are
allowlisted and size-checked; the output is re-read through the AVI parser
before atomic no-overwrite creation. It does not accept arbitrary media frames
or general chunk structures.

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
