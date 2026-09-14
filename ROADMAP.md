# Metra roadmap

Metra is a pre-1.0 Rust metadata toolkit. The status below is an implementation
snapshot derived from code and tests in the repository; it is not a percentage
of ExifTool compatibility.

**Verified implementation snapshot: 93%** across the current eight-axis plan
(92.8% before rounding).

| Axis | Status | Evidence / next gate |
| --- | ---: | --- |
| Typed model and tag catalog | 100% | Bounded readers, stable identifiers, generated catalog, structured values. |
| Real corpus and differential coverage | 65% | A manifest-checked synthetic corpus now covers all 24 creatable fixture files; a licensed real-world corpus and oracle differential report are next. |
| Media and RAW readers | 99% | Broad bounded detection/read surface; deeper structures and proprietary writes remain limited. |
| XMP, IPTC, ICC, ID3, and MakerNotes | 99% | Selected families are covered; unknown/proprietary payloads remain conservative. |
| Safe writing and creation | 90% | Validated atomic writers and bounded seeds exist; broader restructure/create seams remain planned. |
| `set`, `delete`, `copy`, and compare | 99% | CLI/API round-trip coverage is present for the supported writable surface. |
| Parallel and streaming processing | 90% | Bounded workers, bounded result buffering, deterministic streaming, cancellation, corpus regression coverage, and local throughput baselines are implemented; broader platform baselines remain. |
| Structured output | 100% | Text, JSON, JSON Lines, CSV, TOML, and YAML are versioned and tested. |

## Working now

- inspect supported image, audio, document, media, RAW, XMP, and ICC
  containers without decoding their large payloads;
- create validated metadata-oriented seeds for the formats listed in the
  README;
- perform the documented narrow `set`, `delete`, and `copy` rewrites with
  reader revalidation and atomic replacement;
- process batches deterministically with bounded concurrency and streaming
  output;
- consume the same typed model from Rust or the CLI.

## Next priorities

1. Grow reviewed real-world corpus evidence and differential reports without
   turning local experiments into compatibility claims.
2. Expand typed tags and format structures where the parser can preserve
   bounded evidence safely.
3. Extend writer and creator coverage only when layout preservation,
   revalidation, recovery, and round-trip tests are available.
4. Add broader multi-platform performance baselines and profiling evidence.

## Explicit non-goals for the current release

Metra does not promise complete ExifTool parity, arbitrary media encoding,
general image-pixel editing, full proprietary MakerNote interpretation, or
safe writes for every format listed in the capability matrix.
