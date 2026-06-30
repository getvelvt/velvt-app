# Contributing

This guide summarizes the documentation-aware contribution workflow. The root `CONTRIBUTING.md` remains the full project contribution guide.

## Before You Change Code

1. Identify the affected workspace: `swift-client/`, `rust-service/`, `proto/`, or docs-only.
2. Read `AGENTS.md` for project invariants.
3. Open `docs/DOC_INDEX.md` and locate the documentation files that correspond to your change.
4. Inspect existing code, tests, config, and nearby patterns before designing the change.

Most changes should touch only one active workspace. Any `proto/` change is cross-workspace and must update Swift and Rust together.

## Documentation Expectations

When a change affects architecture, APIs, authentication, settings, persistence, IPC, privacy behavior, or significant runtime behavior, update the relevant file under `docs/` in the same task.

Use `docs/DOC_INDEX.md` as the routing table. Do not edit unrelated docs just because they are nearby.

## Privacy Review

Treat privacy boundary changes as high risk. Before opening a PR that touches event capture, abstraction, upload, IPC DTOs, logging, auth, or persistence, verify:

- No raw app names, bundle IDs, window titles, URLs, paths, filenames, contacts, emails, or raw text are added to upload payloads.
- Logs include only safe codes, statuses, timestamps, and abstract labels.
- Tokens and credentials are stored only in Keychain on Swift or the Rust token store path, never SQLite.
- Rust tests cover forbidden-field exclusion for any upload DTO change.

## Verification

Run the checks for the affected workspace:

```sh
make test-rust
make lint-rust
make test-swift
make lint-swift
```

For docs-only changes, at minimum verify the files exist, links/paths are accurate, and markdown headings are coherent. If you also edit code, run the relevant tests and lint checks.

## Pull Request Notes

In the PR description, include:

- Workspace scope.
- User-visible behavior change, if any.
- Privacy impact.
- Tests and lint run.
- Documentation files updated or a short explanation if no docs update was needed.
