# Spike Tests

Spike tests explore speculative or experimental ideas and are **intentionally excluded from CI** and the main `make test` target.

They require manual invocation and may depend on work-in-progress language features.

---

## 001-parser — Parser in MVL (issue #187)

Explores a self-hosted recursive-descent parser written in MVL itself.
Nine experiments, each building on the previous.

**Status:** active / exploratory

**Manual invocation (from repo root):**

```bash
# Build the compiler first
make build

# Run all spike unit tests
make -C tests/spikes/001-parser test

# Run a specific experiment's tests
make -C tests/spikes/001-parser test-09

# Type-check all spike files
make -C tests/spikes/001-parser check

# See all available targets
make -C tests/spikes/001-parser help
```

**Why excluded from CI:**
- Experiments may depend on speculative syntax or features not yet stabilised.
- Failures here are expected during active exploration and should not block the main pipeline.
- See [Makefile](001-parser/Makefile) for the full target list.

---

## Adding a new spike

1. Create `tests/spikes/NNN-name/` with a `Makefile` following the pattern in `001-parser/Makefile`.
2. Add a section to this README describing the spike's purpose and invocation.
3. Do **not** add spike targets to the top-level `make test` or CI workflows.
