# Security policy

Metra treats every inspected file as untrusted input. The parser applies
checked offsets, bounded allocations, recursion and entry limits, safe XML
rules, and output revalidation for supported writes.

If you find a security issue, do not include private media, credentials, or a
weaponized sample in a public issue. Prefer a private GitHub Security Advisory
for this repository. If that channel is unavailable, open a minimal issue
without the sensitive sample and request a private contact path from the
maintainers.

The detailed hostile-input and rewrite policy is in
[`docs/SECURITY.md`](docs/SECURITY.md).
