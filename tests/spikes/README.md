# Spike Tests

Spike tests explore speculative or experimental ideas and are **intentionally excluded from CI** and the main `make test` target.

They require manual invocation and may depend on work-in-progress language features.

## Why spikes are excluded from CI

Spikes are time-boxed explorations. They may:
- Depend on features not yet merged
- Require a fully-built `mvl` binary (`cargo build` first)
- Break intentionally as the language evolves
- Represent abandoned experiments kept for reference

Adding spikes to the standard test suite would cause CI to fail on unrelated
changes. They are excluded by design (#683).

## Running spike tests manually

```bash
# Run a specific spike from the repo root
make -C tests/spikes/001-parser test

# Run a specific experiment's tests
make -C tests/spikes/001-parser test-09

# Type-check all spike files
make -C tests/spikes/001-parser check

# See all available targets
make -C tests/spikes/001-parser help
```

## Spikes

| Directory | Topic | Status | Related issue |
|-----------|-------|--------|---------------|
| `001-parser/` | Parser-in-MVL — recursive-descent parser written in MVL itself | Active | #187 |
| `003-gzip/` | gzip compress/decompress in pure MVL (LZ77 + fixed Huffman + RFC 1952 framing) | Active | #1256 |

## Adding a new spike

1. Create `tests/spikes/NNN-topic/` with a `Makefile` following the pattern in `001-parser/Makefile`.
2. Add an entry to the table above.
3. Add a row to the `AGENTS.md` Spike Tests table.
4. Do **not** wire it into `make test` or any CI target.
