# Metra architecture

## Design goals

Metra is designed as a Rust library first. The binary in `src/main.rs` reads
files through the public root facade and only owns argument parsing, traversal,
bounded worker scheduling, rendering, and exit status. Format code must not
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

The TIFF reader is the low-level building block for EXIF in JPEG, PNG, and
WebP. It accepts a bounded random-access region, so embedded offsets remain
relative to the correct TIFF payload while source offsets can still be
reported against the containing file. EXIF `UserComment` ASCII/Unicode prefixes
are decoded while the original bytes remain available. JPEG, PNG, and WebP delegate structured
XMP to the bounded XML reader; PNG text and `iCCP` chunks use a zlib decoder
capped by `ParseLimits`; Photoshop resources delegate IPTC IIM parsing,
and ICC payloads delegate bounded profile-header, typed illuminant, and tag-table inspection; JPEG
ICC APP2 fragments are collected and reassembled by sequence number before that inspection, while
common ICC text and XYZ table tags retain their 4CC identifiers. PNG and WebP carry their bounded
profile payloads directly. TIFF
also derives GPS decimal coordinates, signed altitude, image direction, speed
in meters per second, and seconds since midnight only after validating their
rational values, units, and ranges.
The ISO-BMFF
reader walks bounded boxes and exposes brands, image properties, direct
XMP/EXIF boxes, and a conservative subset of QuickTime-style `ilst` text items.
The MP3 reader handles bounded ID3v2 frame tables, ID3v1 fixed fields, and a
single MPEG frame header without decoding audio payloads.
The FLAC reader validates the metadata-block chain and decodes STREAMINFO,
Vorbis comments, and bounded PICTURE blocks without touching audio frames.
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

The current writer surface is deliberately limited to JPEG COM segments, APP1
XMP packets, and known IPTC-IIM datasets inside Photoshop APP13 resources, PNG
`tEXt` chunks and uncompressed `iTXt` XMP chunks, GIF comment extensions, WebP `XMP ` chunks,
SVG title/description/comment nodes, WAV `LIST/INFO` fields, FLAC Vorbis
Comment key/value pairs, and common ID3v2 text/comment frames. The WebP and
PNG writers validate replacement packets with the bounded XMP parser. The
JPEG IPTC writer validates dataset names and lengths, rewrites only the target
dataset in the `0x0404` resource, preserves unrelated Photoshop resources, and
creates a bounded APP13 resource when needed.
The SVG writer validates the source XML, escapes replacement text, rejects
unsafe comment delimiters, and preserves unrelated source ranges. The ID3 writer requires a tag without
unsynchronization, extended-header, or footer flags. Each library writer
validates its source through the reader before writing, streams the original
container while preserving untargeted bytes, validates the temporary output
with the reader again, syncs it, and atomically renames a same-directory
temporary file. The CLI exposes `--set`/`--delete`/`--copy` for
`JPEG:Comment`, `IPTC:<dataset>`, `PNG:XMP`, `PNG:Text:<keyword>`, `SVG:Title`/`Description`/`Comment`, `WAV:<INFO field>`,
`FLAC:<Vorbis field>`, `ID3:<text field>`, `GIF:Comment`, and `WebP:XMP`; generic
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
- manufacturer-specific MakerNote modules;
- deeper HEIF/AVIF and media metadata modules;
- a generalized lossless block-preservation abstraction across more formats;
- cancellation and interrupt propagation for long-running batches.

Deferring a seam is not a compatibility claim. The compatibility matrix records
the actual state for each capability.
