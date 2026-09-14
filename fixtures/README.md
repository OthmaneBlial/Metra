# Fixtures

The current test suite creates minimal synthetic JPEG, TIFF, PNG, and WebP
inputs at runtime so no private media is stored in the repository. This folder
is reserved for reviewed, redistributable corpus fixtures and their provenance.

Every future fixture should document its source, license, format, expected
behavior, and whether it is intentionally malformed.

## Local corpus checks

The ignored integration checks use an explicit environment variable so a
private or reviewed corpus is never copied into the repository:

```bash
METRA_CORPUS_DIR=/path/to/corpus cargo test --test corpus -- --ignored --nocapture
METRA_CORPUS_DIR=/path/to/corpus METRA_ORACLE=/path/to/exiftool \
  cargo test --test corpus corpus_supported_tags_can_be_compared_with_oracle -- --ignored --nocapture

# Optional aggregate reports; create the output directory first.
mkdir -p artifacts
METRA_CORPUS_DIR=/path/to/corpus \
  METRA_CORPUS_INSPECTION_REPORT=artifacts/corpus-inspection.json \
  cargo test --test corpus corpus_inspection_does_not_panic -- --ignored --nocapture
METRA_CORPUS_DIR=/path/to/corpus METRA_ORACLE=/path/to/exiftool \
  METRA_CORPUS_DIFFERENTIAL_REPORT=artifacts/corpus-differential.json \
  cargo test --test corpus corpus_supported_tags_can_be_compared_with_oracle -- --ignored --nocapture
```

The first check reports recognized files, parser failures, warnings, and
panics. The optional differential check invokes the oracle with JSON output,
verifies that it returns valid documents, and reports stable-key and typed-value
matches, misses, read failures, and panics for the subset Metra currently
exposes. Reports are written only when their corresponding environment variable
is explicitly set. Add `METRA_ORACLE_STRICT=1` to the differential command when
any missing key, panic, or value mismatch should fail the run. A match count is
evidence for follow-up analysis, not a complete compatibility claim.

The versioned compatibility matrix is also checked during the normal workspace
test run. This verifies that its format rows match the public Rust capability
registry and that every metadata-family, MakerNote, and CLI status uses a known
state.

The same public registry is available without a file through
`cargo run -- --capabilities`, or as an automation-friendly JSON array with
`cargo run -- --capabilities --json`.
