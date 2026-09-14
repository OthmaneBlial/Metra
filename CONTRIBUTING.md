# Contributing to Metra

Thanks for helping make metadata tooling safer and easier to embed.

## Before opening an issue

Search existing issues first. For parser bugs, include the format, the smallest
reproducible input you can share, the command or API call, and the observed
warning or error. Do not upload private media or credentials.

## Development workflow

```bash
rustup show active-toolchain
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo bench --bench throughput --no-run
```

The GitHub Actions workflow is deliberately manual-only. A contributor can
dispatch it for an explicit Linux, macOS, and Windows check; normal pushes do
not trigger CI.

## Pull requests

Keep changes focused and explain the safety boundary. New format readers should
add signature, truncation, hostile-offset, and bounded-value tests. New writers
or creators should add round-trip tests, reject unsupported growth or layout
changes, validate the output through the reader, and update the capability
matrix, changelog, README, and roadmap when the public surface changes.

Please distinguish `Working`, `Experimental`, and `Planned` behavior in docs.
Passing a synthetic test does not establish broad real-world or ExifTool
compatibility.

## Security-sensitive reports

Do not publish an exploitable parser issue with a private sample in a public
issue. Follow [`SECURITY.md`](SECURITY.md) for reporting guidance.
