# Changelog

## 0.1.0 - 2026-09-13

Initial read-first foundation with narrow validated rewrites:

- added bounded PDF Info rewrites for existing literal and hexadecimal string
  tokens, with fixed-span validation, atomic replacement, generic API dispatch,
  registry support, and CLI `--set`/`--copy` coverage;
- added bounded JPEG APP1 EXIF ASCII rewrites and source-to-source copies for
  existing fields, preserving segment size and validating the rewritten JPEG;
- added bounded JPEG APP6 GoPro `DEVC`/`STRM` decoding with stable FourCC tag
  identifiers, aligned nested records, raw-value retention, and limit checks;
- added bounded Pentax MakerNote Big Endian IFD decoding, including root-relative
  value offsets and common camera, exposure, preview, and firmware fields;
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
- decoded bounded FLAC SEEKTABLE entries as typed seek-point structures while
  retaining malformed-length and materialization-limit warnings;
- decoded bounded FLAC CUESHEET catalog, lead-in, track, and index structures
  while retaining malformed-layout and materialization-limit warnings;
- decoded bounded AVI `strh` stream descriptors with typed codec, timing,
  duration, quality, and frame-bound fields without loading video frames;
- decoded AVI video `strf` bitmap properties and audio `strf` format fields
  after bounded stream-type dispatch, without loading media frames;
- decoded ISO-BMFF `mvhd` movie timing and `tkhd` track identifiers, durations,
  fixed-point dimensions, and track properties without loading media payloads;
- added bounded Ogg Vorbis/Opus comment replacement, deletion, and copy with
  page CRC regeneration, same-packet-size preservation, output validation, and
  atomic replacement;
- extended Ogg comment rewriting to native Ogg-FLAC Vorbis Comment blocks while
  preserving their metadata-block headers and packet sizes;
- extended Ogg-FLAC rewriting to comments embedded in the initial mapping
  packet, preserving unrelated mapping and streaminfo bytes;
- added explicit chained TIFF `IFD1`/`IFD2` traversal for thumbnail-directory
  metadata while keeping thumbnail pixel payloads out of memory;
- added bounded Ogg-FLAC Vorbis Comment decoding from mapping and subsequent
  metadata packets, with explicit metadata-block limits;
- added recoverable CRC diagnostics for inspected Ogg metadata pages without
  forcing the reader to materialize skipped audio pages;
- added bounded WebP dimension parsing for native lossy `VP8 ` and lossless
  `VP8L` frames alongside extended `VP8X` canvases;
- added bounded TIFF `SubIFDs` offset-array traversal with explicit `SubIFD1`,
  `SubIFD2`, and subsequent groups;
- added bounded PDF Info/XMP inspection and RIFF/WAVE format, INFO, and BWF
  metadata readers;
- added bounded SVG XML inspection for root geometry, document text, and safe
  comment extraction without rendering;
- added bounded extraction of embedded SVG `xmpmeta` packets through the shared
  XMP reader, preserving packet source offsets and warning on invalid packets;
- added a public `FormatHandler` registry covering every currently advertised
  format, with shared detection, bounded read dispatch, and capability reports;
- added public canonical `MetadataEdit` set/delete operations with validated
  dispatch to the existing format-specific rewrite APIs for paths and bytes;
- extended `FormatHandler` with a seekable `write_metadata` contract, routing
  supported writers through the same canonical edit validation and rejecting
  formats without a validated writer;
- added structured `Date`, `Time`, and `DateTime` tag values with strict EXIF
  and GPS temporal decoding while retaining the original raw bytes;
- added public `copy_metadata_path` source-first copying for supported string
  and UTF-8 XMP values through the validated target rewrite pipeline;
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
  Olympus, Samsung, DJI, and GoPro MakerNote containers without claiming
  proprietary tag decoding;
- added bounded Nikon Type 2 MakerNote IFD parsing for known camera fields with
  typed values, stable numeric identifiers, source offsets, and ParseLimits;
- added bounded Canon MakerNote IFD parsing for selected camera strings and
  arrays with typed values, source offsets, and ParseLimits;
- retained bounded, decodable unknown Nikon and Canon MakerNote values with
  stable fallback names and raw bytes;
  Fujifilm, Panasonic, and Olympus remain header-only;
- added a bounded legacy Sony MakerNote IFD reader with shared tag definitions,
  typed values, absolute source offsets, value limits, and raw-byte retention;
  unsupported Sony substructures remain warnings or stable unknown values;
- added a bounded Fujifilm MakerNote IFD reader with relative-offset handling,
  shared tag definitions, typed values, source offsets, and unknown-value retention;
- added bounded Panasonic and Olympus MakerNote IFD readers for modern and legacy
  little-endian layouts, with vendor-specific tag resolution and raw-byte retention;
- added bounded Nikon Type 1 MakerNote IFD decoding at its legacy offset, with
  version-specific names, typed values, and absolute source offsets;
- added a bounded Apple MakerNote Big Endian IFD reader with stable field names,
  typed values, raw-byte retention, and absolute source offsets;
- added EXIF manufacturer-context detection for payloads whose signatures are
  not self-describing, including GoPro and DJI, while keeping proprietary
  Samsung STMN nested fields and DJI/GoPro fields detection-only;
- added bounded Samsung STMN header and preview-field decoding with nested
  payload retention under the configured value budget;
- added bounded DJI MakerNote IFD decoding with manufacturer-context dispatch,
  byte-order selection, known motion fields, and source-range preservation;
- decoded Apple runtime binary-plist integer fields into a bounded structured
  value while retaining the original MakerNote bytes;
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
- strengthened the opt-in corpus differential harness with namespace/group
  aliases and typed numeric, rational, string, and array value comparisons;
- added bounded Panasonic RW2 TIFF-dialect detection and EXIF delegation,
  including a real-corpus validation case;
- added signature-based identification for legacy Canon CRW, Minolta MRW, and
  Sigma X3F RAW containers with explicit partial-decoding warnings;
- centralized validated path-writer replacement behind a platform-aware atomic
  helper, including native Windows replace-and-write-through behavior;
- added a Rust Criterion throughput benchmark for bounded stream reads and
  optional reviewed-corpus collection throughput;
- added standalone, signature-detected XMP packet and ICC profile readers with
  bounded public stream and path dispatch;
- added bounded Ogg page and logical-stream inspection for Vorbis, Opus, and
  Ogg-FLAC metadata packets, including typed stream headers and comments;
- decoded Ogg-FLAC mapping `STREAMINFO` blocks with typed sample, channel,
  frame-size, block-size, total-sample, duration, and MD5 fields;
- expanded Matroska/WebM inspection with bounded chapter, cue-point, and
  attachment descriptors while skipping cluster and attachment payload data;
- expanded corpus differential aliases for Vorbis and Opus namespaces, with
  typed key/value evidence recorded for the reviewed Ogg samples;
- tolerated truncated trailing Ogg segment tables after preserving complete
  preceding pages as structured metadata with a warning;
- documented the verified surface and remaining compatibility boundaries.
- added typed PNG `IHDR` dimensions and encoding parameters with bounded
  validation and raw-byte source ranges;
- added bounded WAV iXML leaf parsing with raw packet retention and safe XML
  handling, plus embedded ID3v2 delegation with translated source offsets;
- expanded the canonical EXIF/TIFF catalog for image, sensitivity, capture,
  focal-plane, and camera identity tags;
- moved the shared tag catalog to versioned tab-separated source data with
  build-time validation for malformed rows, invalid IDs, and duplicate keys;
- disabled automatic push and pull-request CI triggers while retaining manual
  `workflow_dispatch` runs for the repository's explicit release workflow.
