# Toolchain pins and lint gates

What each toolchain is pinned to, where the pin is enforced, and what `make
lint-rust` and `make lint-swift` actually check.

## Pins

| Tool | Pin | Pin file | Enforced by |
|---|---|---|---|
| Rust | 1.96.0, with clippy and rustfmt | `rust-service/rust-toolchain.toml` | rustup, inside `rust-service/`, locally and in CI |
| Xcode | 16.3 (build 16E140, Swift 6.1, macOS 15.4 SDK) | `.xcode-version` | CI's `swift` and `package` jobs select it. `make package-release`, and so `dmg`, `alpha-dmg` and `release`, stops unless `xcodebuild -version` reports it |
| Python | 3.13 | `.python-version` | CI installs it with `actions/setup-python` for `make test-measurement` and for the dmgbuild venv. pyenv, uv and asdf read the file locally. A plain `python3` ignores it, so check `python3 --version` yourself |

**Why these versions.** Velvt 1.0.11 (build 17) was built with Xcode 16.3: its
`Info.plist` records `DTXcode` 1630, `DTXcodeBuild` 16E140, and `DTSDKName`
macosx15.4. The dmgbuild venv on the release Mac was created from Python 3.13.
The CI runner (`macos-15`) defaults to newer versions of both: Xcode 16.4 and
Python 3.14. Its image also installs Xcode 16.3 as `/Applications/Xcode_16.3.app`
and has Python 3.13 in its tool cache. So CI now builds, tests and lints with
the same compiler and swift-format that a release is built with.

**Building with another Xcode.** Run `make package-release
VELVT_ALLOW_XCODE_MISMATCH=1`. The build prints a warning, and the result is for
local verification only. Never distribute it.

**Moving a pin.** Change the pin file in its own pull request. For Xcode, first
check the runner image's README (linked from the "Runner Image" group at the
top of any CI log) and confirm it installs `/Applications/Xcode_<version>.app`.
The SwiftPM cache key includes `.xcode-version`, so CI starts from a clean
`.build` after the change. Also expect swift-format's output to change with the
toolchain: re-run `make lint-swift` and update the baseline described below.

## `make lint-rust`

```sh
cd rust-service
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

`--workspace --all-targets` lints `shared-types`, the integration tests under
`tests/`, and every `#[cfg(test)]` module. The earlier `cargo clippy -- -D
warnings` covered only the root package's lib and bin. That gap let a
`clippy::assertions_on_constants` error sit in `shared-types` tests without any
gate catching it.

## `make lint-swift`

`swift format lint` exits 0 no matter how many warnings it prints, so the old
target enforced nothing. There was also no configuration, so it linted against
swift-format's 2-space defaults. On 2026-09-25 that produced 26,955 warnings on
a codebase that is mostly indented with 4 spaces.

### The configuration

`swift-client/.swift-format` is swift-format 6.1's default configuration with
two changes:

- **`indentation.spaces: 4`.** 71 of the 88 Swift files use 4 spaces.
- **`lineLength: 120`.** 99% of lines are 99 columns or shorter. At 100 columns,
  274 lines are too long. At 120, 38 are.

When the gate was introduced on 2026-09-25, measured under this configuration
with Xcode 16.3, there were 8,136 findings across 56 files:

| Rule | Findings |
|---|---:|
| Indentation | 7,566 |
| AddLines | 181 |
| DoNotUseSemicolons | 163 |
| UseLetInEveryBoundCaseVariable | 77 |
| Spacing | 53 |
| LineLength | 38 |
| TrailingComma | 31 |
| RemoveLine | 10 |
| OrderedImports | 6 |
| OnlyOneTrailingClosureArgument | 4 |
| NoAccessLevelOnExtensionDeclaration | 3 |
| TrailingWhitespace | 2 |
| ReplaceForEachWithForLoop | 1 |
| NoBlockComments | 1 |

Indentation accounted for 93% of the findings. Most of it came from the 14
files that were written with 2-space indentation and the 3 that mixed both
widths. Those included `VelvtPopoverContentView.swift`, `WorkBlockView.swift`,
`WorkBlockCoordinator.swift`, and most of the snapshot and coordinator tests.

### The reformat

Later on 2026-09-25 the tree was reformatted to this configuration in two
commits:

1. The output of `swift format format --in-place` on every file except the
   three listed below, and nothing else. Besides whitespace and line breaks it
   removes statement-separating semicolons, adds trailing commas to multi-line
   collection literals, sorts imports, removes the spaces around `..<` and
   `...`, and turns three `private extension` blocks into `extension` blocks
   with `fileprivate` members. None of these changes behaviour.
2. Hand fixes for the four lint rules that `swift format format` does not
   rewrite: `UseLetInEveryBoundCaseVariable` (`case let .x(value)` becomes
   `case .x(let value)`), `OnlyOneTrailingClosureArgument`,
   `ReplaceForEachWithForLoop` and `NoBlockComments`.

`swift build` and `swift test` (762 tests, 12 skipped) passed before and after.

Three files were left as they were, because open PR #53 edits them and
formatting them first would have made it conflict:

- `Sources/VelvtMac/Service/ServiceManager.swift`
- `Tests/VelvtMacTests/HistoryViewModelTests.swift`
- `Tests/VelvtMacTests/ServiceManagerTests.swift`

Their 486 findings, in 11 (file, rule) pairs, are the whole baseline now. Every
other file is clean on every rule, so any finding in one of them fails the
gate. Run `cd swift-client && swift format format --in-place <file>` on a file
before committing it.

### The gate

`scripts/lint_swift.sh` runs swift-format and sorts its findings against
`swift-client/.swift-format-baseline`. The baseline lists the (file, rule)
pairs that still have findings: 133 when the gate was introduced and 11 after
the reformat. A finding passes only if its file and rule appear together as a
pair. Anything else fails the build. As a result:

- if a file is clean on a rule, it stays clean on that rule;
- if a file is clean on every rule, it stays clean on every rule;
- a new file must pass every rule.

The script reports baseline pairs that no longer match any finding, but they do
not fail the build. Fixing old findings should never turn someone else's pull
request red. Delete those pairs when you see them. Never add a pair to get a
change through.

`scripts/tests/lint_swift_test.sh` runs the gate against canned swift-format
output. It checks that the gate fails on a new finding, on swift-format
crashing, and on a missing configuration. It runs as part of `make
test-measurement`.

### Follow-up: finish the reformat

After PR #53 merges, or is closed:

1. Format the three files:

   ```sh
   cd swift-client
   swift format format --in-place \
     Sources/VelvtMac/Service/ServiceManager.swift \
     Tests/VelvtMacTests/HistoryViewModelTests.swift \
     Tests/VelvtMacTests/ServiceManagerTests.swift
   ```

2. Review the diff. It should be whitespace, line breaks, semicolons and import
   order only. Fix by hand anything `swift format lint` still reports.
3. Make the gate strict: have `scripts/lint_swift.sh` run `swift format lint
   --strict --recursive Sources Tests` (keep its check for a missing
   `.swift-format`), delete `swift-client/.swift-format-baseline`, and update
   `scripts/tests/lint_swift_test.sh`, this document, `AGENTS.md`,
   `CONTRIBUTING.md` and `docs/quickstart.md`, which describe the baseline.
4. `make test-swift lint-swift test-measurement`.
