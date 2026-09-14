# Metra

### Safe metadata workflows in native Rust.

Metra is a local-first metadata toolkit and CLI for reading, validating,
creating, comparing, and narrowly editing image, audio, document, and media
containers. It gives Rust applications a typed metadata model and gives shell
pipelines deterministic human or structured output.

Metra is an open-source alternative for bounded Rust metadata workflows. It is
not a drop-in replacement for ExifTool and does not claim complete format or
tag compatibility.

[![Release](https://img.shields.io/github/v/release/OthmaneBlial/Metra?display_name=tag&sort=semver)](https://github.com/OthmaneBlial/Metra/releases)
[![CI](https://github.com/OthmaneBlial/Metra/actions/workflows/ci.yml/badge.svg)](https://github.com/OthmaneBlial/Metra/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-8baba4.svg)](LICENSE)
[![Rust 1.95+](https://img.shields.io/badge/rust-1.95%2B-orange?logo=rust&logoColor=white)](rust-toolchain.toml)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-36555a.svg)](.github/workflows/ci.yml)

[Website](https://othmaneblial.github.io/Metra/) ·
[Watch the 49-second demo](https://github.com/OthmaneBlial/Metra/raw/main/site/assets/metra-demo.mp4) ·
[Roadmap](ROADMAP.md) ·
[Architecture](docs/ARCHITECTURE.md) ·
[Security](docs/SECURITY.md) ·
[Changelog](CHANGELOG.md) ·
[Issues](https://github.com/OthmaneBlial/Metra/issues)

![Metra terminal demo](https://raw.githubusercontent.com/OthmaneBlial/Metra/main/site/assets/metra-demo-poster.png)

The demo is recorded from the current `metra` binary. It uses synthetic files
only and shows creation, targeted inspection, a validated PNG rewrite, JSON
Lines output, and the public capability matrix.

## Why Metra exists

Metadata code sits on a difficult boundary: files are untrusted, containers
are nested, and a useful tool must preserve evidence instead of flattening
everything into strings. Metra focuses on a small set of dependable building
blocks:

- parse with checked offsets, bounded allocations, typed values, and warnings;
- expose the same model through a Rust API and a scriptable CLI;
- make read, write, create, delete, lossless rewrite, and streaming status
  explicit for every registered format;
- refuse an edit when the writer cannot validate the result safely.

## What it gives you

### Inspect files without decoding media payloads

Metra detects containers from signatures rather than extensions, walks bounded
metadata regions, retains raw values where useful, and keeps warnings separate
from successfully parsed tags. Image pixels, audio samples, video frames, and
large attachment payloads are not decoded by the metadata reader.

### Build deterministic metadata pipelines

Use human-readable output for investigation or JSON, JSON Lines, CSV, TOML, and
YAML for tools. Recursive traversal is deterministic; `--jobs N` adds bounded
parallel inspection, and streaming modes apply bounded backpressure. The JSON
document schema is versioned at `1`.

### Change only what can be proven safe

Supported writers read the source before writing, copy through a same-directory
temporary file, validate the candidate output through the reader, and replace
the source atomically. Fixed-span writers preserve container layout and reject
growth when the existing storage cannot hold the new value.

### Embed the model in Rust

The binary is a thin consumer of the public `metra` facade. Library users get
in-memory reading, configurable parse limits, deterministic batch helpers,
cooperative cancellation, stable tag identifiers, metadata diffs, a capability
registry, and format-specific creation/rewrite APIs.

## Feature surface

The capability matrix is the source of truth for the current read/write/create
surface. `partial` is intentional: it means a format has a bounded, tested
seam, not that every tag or operation is supported.

| Workflow | Available today |
| --- | --- |
| Read | JPEG, TIFF/BigTIFF, PNG, WebP, GIF, SVG, PSD/PSB, DNG and TIFF-like RAW, CR3, RAF, MRW, CRW, X3F, AVI, MKV/WebM, Ogg/Vorbis/Opus, FLAC, WAV/RF64/BW64, MP3/ID3, PDF, XMP, ICC, HEIF/AVIF, MP4/MOV/M4A |
| Create | Bounded metadata seeds for TIFF/BigTIFF, DNG, JPEG, PNG, WebP, GIF, SVG, PSD, PDF, ICC, XMP, WAV/RF64/BW64, MP3, Ogg Opus, FLAC, AVI, Matroska/WebM, MP4/MOV/M4A, HEIF/AVIF |
| Edit | Tested narrow writers for existing fields in JPEG, TIFF/RAW, PNG, WebP, GIF, SVG, PSD, AVI, Matroska/WebM, ISO-BMFF/CR3, PDF, ICC, XMP, ID3, FLAC, Ogg, WAV/iXML/ID3/BWF |
| Operate | `--set`, `--delete`, `--copy`, `--compare`, `--validate`, `--recursive`, `--jobs`, cancellation on Ctrl-C, and configurable metadata/value budgets |
| Output | Text, JSON, JSON Lines, CSV, TOML, YAML, stable namespace/tag identifiers, structured warnings |

### Working

- bounded signature detection and typed metadata parsing across the registered
  image, audio, document, media, RAW, XMP, and ICC families;
- validated creation of small metadata-oriented seeds, with no-overwrite path
  helpers and output revalidation;
- lossless or fixed-span rewrites where the format contract supports them,
  including EXIF/GPS, PNG `tIME`/`pHYs`, XMP, IPTC, ID3, BWF, iXML, INFO, and
  selected container text fields;
- a public capability registry and compatibility matrix checked by tests;
- deterministic batch APIs and CLI output with bounded worker concurrency.

### Experimental or intentionally limited

- MakerNote interpretation is bounded to selected families and retains unknown
  values rather than guessing;
- proprietary RAW families such as RAF, MRW, CRW, and X3F are read-only;
- many writers require existing storage, equal packet length, or a fixed field;
- the differential corpus harness is opt-in and no corpus is distributed;
- ExifTool compatibility is represented by a versioned bounded matrix, not a
  completeness claim.

### Planned

Broader real-world corpus coverage, deeper tag/catalog coverage, more complete
MakerNote interpretation, broader media structures, and additional creation or
rewrite seams remain on the roadmap. See [`ROADMAP.md`](ROADMAP.md) for the
verified delivery snapshot and explicit gates.

## Demo

The real CLI walkthrough is available in the repository and on the project
site:

[![Watch the Metra demo](https://raw.githubusercontent.com/OthmaneBlial/Metra/main/site/assets/metra-demo-poster.png)](https://github.com/OthmaneBlial/Metra/raw/main/site/assets/metra-demo.mp4)

The MP4 is intentionally a terminal demonstration because Metra is a CLI and
library, not a graphical application. GitHub may sanitize embedded HTML video,
so the poster, direct MP4 link, and site player are all provided.

To reproduce the source transcript and temporary synthetic files:

```bash
cargo build --release
./demo/record.sh
```

To rebuild the presentation asset on macOS with FFmpeg and Quick Look:

```bash
./demo/render.sh
```

## Quick start

### Install from source

Metra currently ships as a source release. Rust `1.95` or newer is required;
the repository pins that toolchain in [`rust-toolchain.toml`](rust-toolchain.toml).

```bash
git clone https://github.com/OthmaneBlial/Metra.git
cd Metra
cargo install --path .
metra --version
```

### Inspect a file

```bash
metra photo.jpg
metra --json photo.jpg
metra --jsonl --jobs 4 -r photos/
metra --tag EXIF:Make --tag EXIF:DateTimeOriginal photo.jpg
metra --validate photo.jpg
metra --compare reference.jpg target.jpg
```

### Create a small seed and edit it

```bash
metra --create-png 'Comment=Metra demo' demo.png
metra --set 'PNG:ModificationTime=2026-09-14 12:34:56' demo.png
metra --tag PNG:ModificationTime demo.png

metra --create-tiff 'EXIF:Make=Metra' camera.tif
metra --create-wav 'Title=Metra' --create-wav 'Artist=Metra' take.wav
metra --create-xmp '<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF/></x:xmpmeta>' packet.xmp
```

For a complete list of creation flags and accepted canonical keys:

```bash
metra --help
metra --capabilities --json
```

## Download and release status

The current public release is [`v0.1.0`](https://github.com/OthmaneBlial/Metra/releases/tag/v0.1.0),
an experimental pre-1.0 source release. It includes the validated library and
CLI described above.

No prebuilt platform binaries are attached yet. Build from source with Cargo
for Linux, macOS, or Windows. Platform CI is manual by design, so a push does
not start a workflow; the same workflow can be deliberately dispatched for a
release check.

## Library API

The root crate exposes the same model used by the CLI:

```rust
let metadata = metra::read("photo.jpg")?;

if let Some(make) = metadata.find("EXIF:Make") {
    println!("camera = {}", make.display_value());
}

let changed = metadata.diff(&other_metadata);
println!("{} metadata differences", changed.changes.len());
```

For larger jobs, use `read_from`, `read_with_limits`, `read_many`, or
`read_many_streaming`. Use `find_all` for repeated datasets and
`find_by_id(namespace, id)` when a numeric tag identifier is available. The
public `MetadataEdit`, `CreateRequest`, `FormatHandler`, and format-specific
helpers expose validated creation and rewrite paths without requiring the CLI.

## How it works

```text
CLI (`src/main.rs`)       Rust API (`src/lib.rs`)
          \                      /
           v                    v
     detection + format handler registry
                       |
                       v
       bounded readers / typed metadata model
                       |
       set/delete/copy -> validate -> atomic replace
```

The workspace is split into small responsibilities:

- `metra-core` owns the typed model, tag catalog, schema version, capabilities,
  limits, warnings, and structured errors;
- `metra-formats` owns signature detection, defensive readers, creators, and
  narrow format writers;
- the root `metra` crate provides the public facade and CLI consumer;
- `compat/exiftool-compatibility.json` records bounded compatibility evidence;
- `tests/` exercises parser safety, round trips, CLI behavior, registry
  contracts, and the opt-in corpus harness.

## Security and privacy

Metra is designed for local processing. Runtime readers do not invoke Perl,
Python, Node, ExifTool, or another external metadata process, and they do not
send files to a service. Untrusted input is handled with checked offsets,
allocation budgets, recursion and entry limits, decompression limits, safe XML
rules, and structured warnings.

Writes are conservative: the source is read before mutation, the candidate is
written through a same-directory temporary file, the result is re-read and
validated, and replacement happens only after that validation. A failed write
removes its temporary output and leaves the source untouched. These guarantees
reduce risk; they are not a substitute for backups or review of untrusted
content.

Read the full policy in [`docs/SECURITY.md`](docs/SECURITY.md).

## Building from source

```bash
rustup show active-toolchain
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo bench --bench throughput --no-run
cargo build --release
```

The differential test is intentionally ignored unless a reviewed local corpus
and oracle are supplied. Passing the local suite does not establish complete
ExifTool compatibility.

The repository also ships a deterministic synthetic corpus covering every
currently creatable container family. It is inspected by the normal test suite;
see [`fixtures/CORPUS_MANIFEST.json`](fixtures/CORPUS_MANIFEST.json) for
provenance and checksums.

## Contributing

Start with [`CONTRIBUTING.md`](CONTRIBUTING.md), then open an issue for a new
format, tag family, writer seam, or safety concern. New format behavior should
include focused synthetic fixtures, malformed-input coverage, round-trip tests
for writes, capability-matrix updates, and documentation that distinguishes
working, experimental, and planned behavior.

The GitHub Actions workflow is manual-only. Run the local validation commands
above before opening a pull request, and dispatch the workflow deliberately
when a remote multi-platform check is useful.

## License

Metra is distributed under the [MIT License](LICENSE).
