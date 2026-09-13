# Changelog

## 0.1.0 - 2026-09-13

Initial read-first foundation with narrow validated rewrites:

- added the public typed metadata model and versioned JSON schema;
- added signature-based detection for JPEG, TIFF, PNG, WebP, PDF, and GIF;
- added defensive JPEG, TIFF/EXIF, PNG, and WebP readers;
- added bounded XMP/RDF, IPTC IIM, and ICC profile readers;
- added GIF comment and logical-screen inspection;
- added ISO-BMFF box walking with HEIF/AVIF/MP4/MOV/M4A brand detection and
  partial QuickTime-style text metadata;
- added ID3v2.2/v2.3/v2.4 and ID3v1 text/media-frame inspection plus basic MPEG
  audio-frame properties;
- added FLAC STREAMINFO, Vorbis comments, embedded-picture inspection, and
  bounded metadata-block validation;
- added bounded PDF Info/XMP inspection and RIFF/WAVE format, INFO, and BWF
  metadata readers;
- added bounded SVG XML inspection for root geometry, document text, and safe
  comment extraction without rendering;
- added validated, lossless JPEG COM replacement/deletion with same-directory
  temporary files, output re-read validation, and atomic replacement;
- exposed the supported JPEG comment edits through explicit CLI `--set` and
  `--delete` flags with non-JPEG rejection;
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
- added bounded `--jobs` batch inspection with deterministic result ordering;
- expanded ISO-BMFF inspection with `ispe` dimensions, item/handler fields,
  and direct XMP/EXIF decoding when the metadata boxes are available;
- added deterministic CSV output with typed JSON values in the value column;
- added stable tag identifiers, a shared partial definition catalog, and
  numeric `Metadata::find_by_id` lookup;
- added TOML and YAML output while preserving the versioned metadata schema;
- added streaming human-readable, JSON Lines, and CSV batch output with
  deterministic ordering and bounded out-of-order buffering;
- added opt-in local corpus and oracle-differential harnesses without counting
  an unexecuted corpus as compatibility evidence;
- added human-readable, JSON, and JSON Lines CLI output;
- added malformed-input, resource-limit, and end-to-end CLI tests;
- documented the verified surface and remaining compatibility boundaries.
