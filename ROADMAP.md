# Metra roadmap

Metra's initial public roadmap is complete. The **100%** below means that the
documented pre-1.0 delivery scope is implemented and backed by repository
evidence; it is not a claim of complete ExifTool compatibility.

**Verified implementation snapshot: 100% of the initial roadmap**

## Completed initial scope

| Workstream | Status | Evidence |
| --- | ---: | --- |
| Typed model and tag catalog | 100% | Stable identifiers, generated catalog, typed values, and structured output tests. |
| Reproducible corpus coverage | 100% | Manifest-checked synthetic corpus covers all 24 creatable fixture files; smoke, checksum, and no-panic tests run by default. |
| Bounded media and RAW readers | 100% | The public capability matrix and format readers describe and test the supported bounded surface. |
| XMP, IPTC, ICC, ID3, and MakerNotes | 100% | Selected families, conservative unknown handling, and family-specific read/write tests are covered. |
| Safe writing and creation | 100% | Supported writers validate output, use atomic replacement, reject unsafe inputs, and have round-trip coverage. |
| `set`, `delete`, `copy`, and compare | 100% | CLI and Rust API workflows are covered across the documented writable surface. |
| Parallel and streaming processing | 100% | Bounded workers, bounded result buffering, deterministic output, cancellation, corpus regression, and local throughput checks are implemented. |
| Release and project trust | 100% | README, architecture/security docs, demo, compatibility matrix, MIT license, release notes, and manual CI workflow are present. |

## Working now

- inspect supported image, audio, document, media, RAW, XMP, and ICC
  containers without decoding their large payloads;
- create validated metadata-oriented seeds for the formats listed in the
  README;
- perform the documented narrow `set`, `delete`, and `copy` rewrites with
  reader revalidation and atomic replacement;
- process batches deterministically with bounded concurrency and streaming
  output;
- consume the same typed model from Rust or the CLI;
- reproduce the checked-in corpus and all roadmap gates with the commands in
  the README and `fixtures/README.md`.

## Post-roadmap backlog

These are intentionally outside the completed initial roadmap and remain
future work, not hidden claims of current support:

1. Add a reviewed, redistributable real-world corpus and publish a repeatable
   differential report for it.
2. Expand vendor-specific RAW payload structures and proprietary MakerNotes.
3. Add deeper PSD/PSB layers, HEIF/AVIF codec metadata, and additional media
   structures where bounded preservation is possible.
4. Generalize lossless block preservation and the typed edit IR across more
   formats.
5. Publish comparable benchmark baselines from multiple operating systems.

## Explicit non-goals

Metra does not promise complete ExifTool parity, arbitrary media encoding,
general image-pixel editing, full proprietary MakerNote interpretation, or
safe writes for every format listed in the capability matrix. The matrix
records the actual status of each format and operation; “100% roadmap” does
not change those bounded capability labels.

## Verification commands

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo bench --bench throughput -- --noplot
```
