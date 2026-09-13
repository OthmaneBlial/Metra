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
```

The first check reports recognized files, parser failures, warnings, and
panics. The optional differential check invokes the oracle with JSON output,
verifies that it returns valid documents, and reports stable-key matches for
the subset Metra currently exposes. A key-match count is evidence for follow-up
analysis, not a complete compatibility claim.
