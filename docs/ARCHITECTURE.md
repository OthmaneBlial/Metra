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
- typed `TagValue` variants, including rationals, arrays, bytes, and unknowns;
- structured `Warning` values;
- `MetraError` categories;
- explicit parser resource limits.

The model preserves a canonical tag name and underlying numeric identifier when
available. Display formatting is a presentation concern and is not a lookup
contract.

### `metra-formats`

This crate owns magic-byte detection and format-specific readers. Readers are
separate modules rather than one parser with format-specific branches spread
through the CLI.

The TIFF reader is the low-level building block for EXIF in JPEG, PNG, and
WebP. It accepts a bounded random-access region, so embedded offsets remain
relative to the correct TIFF payload while source offsets can still be
reported against the containing file. JPEG, PNG, and WebP delegate structured
XMP to the bounded XML reader; Photoshop resources delegate IPTC IIM parsing,
and ICC payloads delegate profile-header/tag-table inspection. The ISO-BMFF
reader walks bounded boxes and exposes brands, image properties, direct
XMP/EXIF boxes, and a conservative subset of QuickTime-style `ilst` text items.
The MP3 reader handles bounded ID3v2 frame tables, ID3v1 fixed fields, and a
single MPEG frame header without decoding audio payloads.
The FLAC reader validates the metadata-block chain and decodes STREAMINFO,
Vorbis comments, and bounded PICTURE blocks without touching audio frames.
The PDF reader scans bounded head/tail windows for Info dictionaries and direct
XMP packets; the WAV reader walks RIFF chunks and decodes `fmt `, `LIST/INFO`,
and Broadcast Wave `bext` fields without loading audio data. Batch workers use a
bounded atomic work index and restore path order before rendering.

## Parser invariants

Every untrusted size or offset must satisfy all of the following before use:

1. checked arithmetic succeeds;
2. the resulting range is inside the declared input region;
3. the allocation is within `ParseLimits`;
4. recursion, entry, chunk, or segment counters remain within their limits.

Malformed embedded metadata is recoverable at the container layer when safe:
the reader adds a warning and retains tags already extracted from other blocks.
Standalone TIFF parsing returns a structured error for an invalid root header or
out-of-range required read.

## Output contract

JSON documents include `schema_version`. The schema is intentionally small and
typed rather than a flattened map. A future schema change must either preserve
version `1` semantics or increment the version and document the migration.

For multiple files, `--json` emits an array of successful metadata documents;
`--jsonl` emits one document per successful file; and `--csv` emits one row per
tag with the typed value serialized as JSON. Errors are sent to stderr and
produce a non-zero exit code.

## Planned seams

The following changes are deferred until their acceptance tests exist:

- a generated tag-definition database instead of a growing handwritten table;
- a `FormatHandler` capability abstraction once write/create behavior creates
  meaningful shared operations;
- manufacturer-specific MakerNote modules;
- deeper HEIF/AVIF and media metadata modules;
- a lossless block-preservation layer for safe rewrites;
- streaming batch output that avoids retaining every successful document.

Deferring a seam is not a compatibility claim. The compatibility matrix records
the actual state for each capability.
