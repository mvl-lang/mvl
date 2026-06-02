# Contributing to pkg-zmq

Thank you for your interest in contributing to the MVL ZeroMQ-style messaging package.

## Getting Started

1. Fork this repository
2. Create a feature branch: `git checkout -b feat/my-feature`
3. Make your changes
4. Run the type checker: `mvl check src/`
5. Run tests: `mvl test src/`
6. Commit with a conventional message: `git commit -m "feat: add ..."`
7. Push and open a pull request

## Development Setup

You need the [MVL compiler](https://github.com/LAB271/mvl_language) installed:

```bash
git clone https://github.com/LAB271/mvl_language.git
cd mvl_language
cargo build
export PATH="$PWD/target/debug:$PATH"
```

## Code Style

- Follow the MVL syntax conventions (see the [MVL cheat sheet](https://github.com/LAB271/mvl_language/blob/main/CLAUDE.md))
- All public functions must have doc comments (`///`)
- All functions must declare their effects (`! Net` for all I/O)
- IFC labels must be preserved: received frames are `Tainted[String]`
- Use `while true` + `return` for accept loops, not recursive tail-calls
- `encode_frame` and `decode_frame` must remain `total fn` (no effects)

## Testing

```bash
mvl check src/          # type-check all source files
mvl test src/           # run inline unit tests
make test-integration   # actor-based loopback integration tests (requires MVL in PATH)
make sync-check         # verify test re-declarations match source signatures
```

## Wire Protocol

The simplified ZMTP format is: `[4 bytes big-endian u32 length][N bytes body]`.
Changes to frame encoding must maintain backward compatibility with the existing test suite and ZMTP 3.x interop tests.

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
