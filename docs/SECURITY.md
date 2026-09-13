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
- malformed embedded EXIF can be downgraded to a warning at the container
  boundary;
- recursive CLI traversal uses directory entry file types and does not follow
  symlink directories;
- no parser invokes Perl, Python, Node, or another external metadata process.

## Limits

The default `ParseLimits` values are intentionally conservative for a first
read-only release. Library callers can select stricter values for untrusted
batch jobs. Any new decompression or XML implementation must add its own
expansion and nesting limits before being enabled.

## Future write safety

Writing is not implemented in this release. Before it is enabled, a writer
must write to a temporary file, validate the rewritten metadata and container,
and replace the original atomically when the platform permits. An explicit
overwrite policy and failure recovery path are required.

Security reports should include the smallest reproducible input and the exact
Metra version. Do not include private media or secrets in an issue.
