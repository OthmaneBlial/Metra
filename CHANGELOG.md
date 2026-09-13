# Changelog

## 0.1.0 - 2026-09-13

Initial read-first foundation with narrow validated rewrites:

- added the public typed metadata model and versioned JSON schema;
- added signature-based detection for JPEG, TIFF, PNG, WebP, PDF, and GIF;
- added defensive JPEG, TIFF/EXIF, PNG, and WebP readers;
- added bounded XMP/RDF, IPTC IIM, and ICC profile readers;
- expanded ICC inspection with typed profile class, platform, manufacturer,
  model, creation date, rendering intent, illuminant, profile ID, and
  description fields;
- added GIF comment and logical-screen inspection;
- added ISO-BMFF box walking with HEIF/AVIF/MP4/MOV/M4A brand detection and
  partial QuickTime-style text metadata;
- added bounded ISO-BMFF QuickTime text replacement and copy for existing
  values, preserving box sizes and validating atomic output for supported
  HEIF/AVIF/MP4/MOV/M4A containers;
- added ID3v2.2/v2.3/v2.4 and ID3v1 text/media-frame inspection plus basic MPEG
  audio-frame properties;
- added FLAC STREAMINFO, Vorbis comments, embedded-picture inspection, and
  bounded metadata-block validation;
- added bounded PDF Info/XMP inspection and RIFF/WAVE format, INFO, and BWF
  metadata readers;
- added bounded SVG XML inspection for root geometry, document text, and safe
  comment extraction without rendering;
- added shared safe XML character-reference handling for SVG and XMP while
  rejecting custom entities and DOCTYPE declarations;
- added validated, lossless JPEG COM replacement/deletion with same-directory
  temporary files, output re-read validation, and atomic replacement;
- added validated JPEG APP1 XMP replacement/deletion/insertion with bounded
  XML validation and atomic replacement;
- added validated JPEG IPTC-IIM dataset replacement/deletion inside Photoshop
  APP13 resources while preserving unrelated Photoshop blocks;
- exposed the supported JPEG comment edits through explicit CLI `--set` and
  `--delete` flags with non-JPEG rejection;
- exposed known JPEG IPTC-IIM dataset edits through CLI `--set`, `--delete`,
  and single-valued-source `--copy` flags;
- added `--copy JPEG:Comment=SOURCE TARGET` with source validation and the same
  atomic target rewrite path;
- added validated PNG `tEXt` replacement/deletion/copy with CRC regeneration,
  streamed image chunks, and atomic target replacement;
- added validated PNG uncompressed `iTXt` XMP replacement/deletion/copy with
  bounded XML validation, CRC regeneration, streamed image chunks, and atomic
  target replacement;
- added validated WAV `LIST/INFO` replacement/deletion/copy with RIFF size
  repair, streamed audio chunks, and atomic target replacement;
- added validated FLAC Vorbis Comment replacement/deletion/copy while
  preserving other metadata blocks and audio frames;
- added validated ID3v2 text/comment replacement/deletion/copy for common
  fields while preserving other frames, padding, and MPEG audio bytes;
- added validated GIF comment-extension replacement/deletion/copy while
  preserving color tables, image descriptors, LZW data, and trailers;
- added validated WebP XMP chunk replacement/deletion/copy with bounded XML
  validation, RIFF size repair, streamed chunk preservation, and atomic target replacement;
- added validated SVG title, description, and comment replacement/deletion/copy
  with XML escaping, safe comment validation, source-range preservation, and
  atomic target replacement;
- added bounded `--jobs` batch inspection with deterministic result ordering;
- added bounded zlib decoding for PNG `zTXt` and compressed `iTXt` text/XMP,
  including expansion-limit warnings instead of unbounded allocation;
- added bounded PNG `iCCP` decompression and typed ICC profile inspection;
- added bounded JPEG ICC APP2 fragment reassembly with sequence and duplicate
  checks before profile inspection;
- added typed WebP `ICCP` profile inspection under the configured value budget;
- added bounded ICC text and XYZ table-tag inspection with stable 4CC IDs;
- added isolated detection of common Nikon, Canon, Fujifilm, Sony, Panasonic,
  and Olympus MakerNote containers without claiming proprietary tag decoding;
- added bounded Nikon Type 2 MakerNote IFD parsing for known camera fields with
  typed values, stable numeric identifiers, source offsets, and ParseLimits;
- added bounded Canon MakerNote IFD parsing for selected camera strings and
  arrays with typed values, source offsets, and ParseLimits;
- retained bounded, decodable unknown Nikon and Canon MakerNote values with
  stable fallback names and raw bytes;
  other detected vendors remain header-only;
- catalogued the bounded Nikon MakerNote identifiers centrally and covered the
  EXIF integration path with absolute source-offset assertions;
- added bounded PSD/PSB inspection for headers, dimensions, common Photoshop
  image resources, delegated XMP/IPTC/ICC/EXIF metadata, and unknown-resource
  byte preservation;
- added bounded AVI inspection for RIFF validation, `avih` dimensions and
  frame timing, and common `LIST/INFO` text without decoding video frames;
- preserved numeric IPTC-IIM dataset identifiers in parsed tags and stable lookup;
- decoded EXIF `UserComment` ASCII and Unicode payloads while preserving raw bytes;
- expanded canonical EXIF names for common image, exposure, color, and lens tags;
- exposed repeated canonical and numeric tag lookup through the public API;
- expanded GPS derived values with validated decimal coordinates, signed
  altitude, image direction, seconds-since-midnight time, and SI speed;
- expanded ISO-BMFF inspection with `ispe` dimensions, item/handler fields,
  and direct XMP/EXIF decoding when the metadata boxes are available;
- added deterministic CSV output with typed JSON values in the value column;
- added stable tag identifiers, a shared partial definition catalog, and
  numeric `Metadata::find_by_id` lookup;
- added TOML and YAML output while preserving the versioned metadata schema;
- added streaming human-readable, JSON Lines, and CSV batch output with
  deterministic ordering and bounded out-of-order buffering;
- added `--validate` to fail automation on recoverable parser warnings while
  retaining parsed metadata and warning evidence;
- exposed configurable `--max-metadata-bytes` and `--max-value-bytes` budgets,
  including during source and rewritten-output validation;
- exposed a typed format capability matrix through the public Rust API;
- added value-level `Metadata::diff` and CLI `--compare` for deterministic
  additions, removals, and changes;
- added opt-in local corpus and oracle-differential harnesses without counting
  an unexecuted corpus as compatibility evidence;
- added human-readable, JSON, and JSON Lines CLI output;
- added malformed-input, resource-limit, and end-to-end CLI tests;
- added bounded TIFF ASCII copy from TIFF-like sources into existing TIFF
  fields, with target-format, string-type, capacity, and output re-read checks;
- exposed the same signature detection and format dispatch for public `Read + Seek`
  streams through `metra::read_from` and `metra::read_from_with_limits`;
- moved deterministic batch inspection into the public facade with bounded
  `read_many` and backpressure-bounded `read_many_streaming` helpers used by the CLI;
- added cooperative batch cancellation through `CancellationToken`, structured
  `MetraError::Cancelled` results, and CLI Ctrl+C handling with exit status 130;
- documented the verified surface and remaining compatibility boundaries.
