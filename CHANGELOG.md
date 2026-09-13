# Changelog

## 0.1.0 - 2026-09-13

Initial read-only foundation:

- added the public typed metadata model and versioned JSON schema;
- added signature-based detection for JPEG, TIFF, PNG, WebP, PDF, and GIF;
- added defensive JPEG, TIFF/EXIF, PNG, and WebP readers;
- added bounded XMP/RDF, IPTC IIM, and ICC profile readers;
- added GIF comment and logical-screen inspection;
- added ISO-BMFF box walking with HEIF/AVIF/MP4/MOV/M4A brand detection and
  partial QuickTime-style text metadata;
- added human-readable, JSON, and JSON Lines CLI output;
- added malformed-input, resource-limit, and end-to-end CLI tests;
- documented the verified surface and remaining compatibility boundaries.
