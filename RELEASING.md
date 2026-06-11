# Releasing MVL

MVL uses two tagging schemes: development versions and phase releases.

## Development Versions (`vX.Y.Z`)

Every merge to main bumps the version in `Cargo.toml` and creates a `vX.Y.Z` tag. These are for CI, development tracking, and internal reference. They do **not** create GitHub releases.

Frequency: multiple per day during active development.

## Phase Releases (`release-0.X.0`)

GitHub releases are created only at phase boundaries — when a development phase closes and the next begins. These releases have curated changelogs summarizing the phase's work.

| Tag | Phase | Notes |
|-----|-------|-------|
| `release-0.9.0` | Phase 9 | Runtime abstraction, integration stack, self-hosting groundwork |
| `release-0.10.0` | Phase 10 | (future) |
| `release-1.0.0` | 1.0 | Production-ready |

### Creating a Phase Release

1. Ensure all phase work is merged and tests pass
2. Tag the release commit:
   ```bash
   git tag -a release-0.X.0 <commit-sha> -m "Phase X complete"
   git push origin release-0.X.0
   ```
3. Create the GitHub release:
   ```bash
   gh release create release-0.X.0 --title "Phase X: <title>" --notes-file PHASE_X_NOTES.md
   ```
   Or use `--generate-notes` with `--notes-start-tag` pointing to the previous phase release.

## Version Numbers

The `vX.Y.Z` versions follow semver loosely during 0.x:
- Patch (Z): bug fixes, minor improvements
- Minor (Y): features, breaking changes within a phase

Major version 1.0.0 comes when the language is production-ready with stable syntax, semantics, and stdlib.
