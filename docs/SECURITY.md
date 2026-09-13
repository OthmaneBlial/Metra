# Security and hostile-input policy

Metra treats every inspected file as untrusted input. Rust memory safety does
not by itself prevent logical resource exhaustion, so readers enforce limits
before allocating or traversing metadata.

## Current protections

- magic-byte detection does not trust filename extensions;
- all TIFF offsets and lengths use checked arithmetic and region bounds;
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
- derived GPS coordinates, altitude, direction, speed, and time values reject
  non-finite rationals and out-of-range references or units before conversion;
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

Only JPEG comment, bounded APP1 XMP, and known IPTC-IIM datasets in Photoshop
APP13 resources, PNG `tEXt` and uncompressed `iTXt` XMP, GIF comments, WebP XMP, SVG
title/description/comments, WAV `LIST/INFO`, FLAC Vorbis Comment, common
ID3v2 text/comment replacement/deletion/copy, and bounded TIFF ASCII copy are
implemented, through the
library API and the explicit `--set`/`--delete`/`--copy` CLI flags. JPEG XMP
writes validate replacement packets with the bounded XML reader. IPTC writes
validate the dataset allowlist, NUL-free values, resource sizes, and APP13
segment limits; unrelated Photoshop resources are preserved. SVG replacement
values are XML-escaped, and comment writes reject `--` and a trailing `-` so
the resulting document remains valid XML.
ID3v2
unsynchronization, extended headers, and footers are rejected by the writer
until their round-trip handling is implemented.
Each writer reads and validates the source first,
copies the container through a same-directory temporary file, syncs and
re-reads the output, preserves source permissions, and replaces the original
only after validation. Failures remove the temporary file and leave the source
untouched. Generic tag writes and other formats remain disabled until their
round-trip and recovery tests exist.

Security reports should include the smallest reproducible input and the exact
Metra version. Do not include private media or secrets in an issue.
