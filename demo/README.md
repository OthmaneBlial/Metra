# Metra demo

The published demo is a real terminal walkthrough generated from the current
`metra` binary. It creates small metadata seeds, inspects them, performs a
lossless PNG timestamp rewrite, emits JSON Lines, and prints the public
capability matrix.

To reproduce the transcript locally:

```bash
cargo build --release
./demo/record.sh
```

The script writes the generated transcript to a temporary directory by
default. Pass a path explicitly when you want to keep it. The temporary files
contain synthetic metadata only; no personal files, credentials, or external
corpus are used.
