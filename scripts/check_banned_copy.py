#!/usr/bin/env python3
"""Fail when a string literal in shipped code claims a capability Velvt lacks.

Velvt's drift policy is deterministic and nothing in the product learns. The
honesty rule (GOAL.md, pivot-engineering/08-PITCH.md, testing/04) bans copy
saying that Velvt learns, adapts, predicts, gets smarter, or that behavioural
modelling takes over. Until this check existed nothing enforced that rule on
the Swift client at all: `BANNED_COPY_TOKENS` in
`rust-service/src/work_block/mod.rs` checks Rust-rendered copy at run time,
but the Swift side shipped "Learning from your recent sessions" in the History
baseline label and "Unable to reset classification learning" as an error,
and a test pinned the first one.

What is scanned: every string literal in the code that ships.

- `swift-client/Sources/**/*.swift`
- `rust-service/src/**/*.rs` and `rust-service/shared-types/src/**/*.rs`,
  minus items under a test-only `#[cfg(...)]` (`test`, `all(test, ...)`,
  `any(test, ...)`), which are compiled only into test binaries.

What is not scanned, and why that is the whole allowlist of kinds:

- Comments, including doc comments. They explain code to contributors and
  are never shown to a user; several of them quote the banned copy they
  replaced, which is the point of them.
- Identifiers (`resetClassificationLearning`). They are code, not copy, and
  renaming them churns call sites for no user-visible change. Only literals
  are lexed, so an identifier cannot match.
- Code inside a Swift interpolation (`"\\(model.learningRate)"`). The text
  around it is scanned; the expression is code. String literals nested inside
  the interpolation are scanned.
- Test code (`swift-client/Tests`, `rust-service/tests`, test-only cfg
  items). Tests quote banned copy in order to assert it is gone.

Anything else that has to contain a banned word is listed in
`scripts/banned-copy-allowlist.json` as an exact (path, literal) pair with the
reason. An entry covers that literal in that file only, so a new sentence
using the same word still fails. An entry that no longer matches anything
fails the check too, so the allowlist cannot quietly outlive its reason.

Exit status: 0 when clean, 1 on any finding or stale allowlist entry, 2 on a
usage or lexing error. Runs with the standard library only.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

# Capability claims the product cannot make. Matched case-insensitively on
# word boundaries, so "learn", "learns", "learned", "learning" and "learnt"
# all match and "Learner" in a proper noun would too (allowlist it if one
# ever ships). Voice rules such as absence framing are enforced on rendered
# Rust copy by BANNED_COPY_TOKENS and are deliberately not duplicated here.
BANNED_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("learn*", re.compile(r"\blearn(?:s|t|ed|ing|er|ers)?\b", re.IGNORECASE)),
    ("adapt*", re.compile(r"\badapt(?:s|ed|ing|ive|ively|ation|ations)?\b", re.IGNORECASE)),
    ("predict*", re.compile(r"\bpredict(?:s|ed|ing|ion|ions|ive|or|ors)?\b", re.IGNORECASE)),
    ("smarter", re.compile(r"\bsmarter\b", re.IGNORECASE)),
    ("behavioral model", re.compile(r"\bbehaviou?ral\s+model", re.IGNORECASE)),
)

SWIFT_ROOTS = ("swift-client/Sources",)
RUST_ROOTS = ("rust-service/src", "rust-service/shared-types/src")
DEFAULT_ALLOWLIST = "scripts/banned-copy-allowlist.json"


class LexError(Exception):
    pass


@dataclass(frozen=True)
class Literal:
    line: int
    text: str


def line_of(source: str, index: int) -> int:
    return source.count("\n", 0, index) + 1


# --------------------------------------------------------------------- Swift


def swift_literals(source: str) -> list[Literal]:
    found: list[Literal] = []
    _swift_code(source, 0, found, inside_interpolation=False)
    return found


def _skip_block_comment(source: str, index: int) -> int:
    """`index` is at `/*`. Swift and Rust block comments both nest."""
    depth = 0
    while index < len(source):
        if source.startswith("/*", index):
            depth += 1
            index += 2
        elif source.startswith("*/", index):
            depth -= 1
            index += 2
            if depth == 0:
                return index
        else:
            index += 1
    raise LexError("unterminated block comment")


def _swift_code(source: str, index: int, found: list[Literal], inside_interpolation: bool) -> int:
    depth = 0
    while index < len(source):
        char = source[index]
        if source.startswith("//", index):
            newline = source.find("\n", index)
            index = len(source) if newline < 0 else newline + 1
        elif source.startswith("/*", index):
            index = _skip_block_comment(source, index)
        elif char == "#" or char == '"':
            hashes = 0
            probe = index
            while probe < len(source) and source[probe] == "#":
                hashes += 1
                probe += 1
            if probe < len(source) and source[probe] == '"':
                index = _swift_string(source, probe, hashes, found)
            else:
                index = probe if hashes else index + 1
        elif inside_interpolation and char == "(":
            depth += 1
            index += 1
        elif inside_interpolation and char == ")":
            if depth == 0:
                return index + 1
            depth -= 1
            index += 1
        else:
            index += 1
    if inside_interpolation:
        raise LexError("unterminated string interpolation")
    return index


def _swift_string(source: str, index: int, hashes: int, found: list[Literal]) -> int:
    """`index` is at the opening quote. Returns the index after the literal."""
    start = index
    multiline = source.startswith('"""', index)
    index += 3 if multiline else 1
    closing = ('"""' if multiline else '"') + "#" * hashes
    escape = "\\" + "#" * hashes
    text: list[str] = []
    while index < len(source):
        if source.startswith(closing, index):
            found.append(Literal(line_of(source, start), "".join(text)))
            return index + len(closing)
        if source.startswith(escape, index):
            after = index + len(escape)
            if after < len(source) and source[after] == "(":
                # The interpolated expression is code; literals inside it are
                # collected by the recursive call. A space keeps the words on
                # either side of it from fusing.
                index = _swift_code(source, after + 1, found, inside_interpolation=True)
                text.append(" ")
                continue
            text.append(source[index : after + 1])
            index = after + 1
            continue
        if not multiline and source[index] == "\n":
            raise LexError(f"line {line_of(source, start)}: newline inside a single-line string")
        text.append(source[index])
        index += 1
    raise LexError(f"line {line_of(source, start)}: unterminated string literal")


# ---------------------------------------------------------------------- Rust

_CFG = re.compile(r"#\s*\[\s*cfg\s*\((.*?)\)\s*\]", re.DOTALL)


def _cfg_is_test_only(expression: str) -> bool:
    """True for `test`, `all(test, ...)`, and `any(test, ...)`.

    `any(test, feature = "...")` is treated as test-only because every such
    feature in this crate (`test-helpers`, `extensibility-proof`) is off by
    default and exists to expose code to tests. `not(test)` is production
    code and is never skipped.
    """
    expression = " ".join(expression.split())
    if "not(" in expression.replace(" ", ""):
        return False
    return re.search(r"(?<![\w-])test(?![\w-])", expression) is not None


def rust_literals(source: str) -> list[Literal]:
    found: list[Literal] = []
    index = 0
    length = len(source)
    while index < length:
        char = source[index]
        if source.startswith("//", index):
            newline = source.find("\n", index)
            index = length if newline < 0 else newline + 1
        elif source.startswith("/*", index):
            index = _skip_block_comment(source, index)
        elif char == "#" and (cfg := _CFG.match(source, index)):
            index = cfg.end()
            if _cfg_is_test_only(cfg.group(1)):
                index = _skip_rust_item(source, index)
        elif char == "'":
            index = _skip_rust_char_or_lifetime(source, index)
        elif (literal := _rust_string_start(source, index)) is not None:
            index = _rust_string(source, index, literal, found)
        elif char.isalnum() or char == "_":
            # Consume the whole identifier so a `b`, `r` or `c` inside one is
            # never read as a string prefix.
            while index < length and (source[index].isalnum() or source[index] == "_"):
                index += 1
        else:
            index += 1
    return found


def _rust_string_start(source: str, index: int) -> tuple[int, int, bool] | None:
    """If a string literal starts at `index`, return (prefix length, hashes, raw)."""
    match = re.compile(r'(b|c)?(r)?(#*)"').match(source, index)
    if match is None:
        return None
    raw = match.group(2) is not None
    hashes = len(match.group(3))
    if hashes and not raw:
        return None
    return (match.end() - 1 - index, hashes, raw)


def _rust_string(source: str, index: int, literal: tuple[int, int, bool], found: list[Literal]) -> int:
    prefix, hashes, raw = literal
    start = index
    index += prefix + 1
    closing = '"' + "#" * hashes
    text: list[str] = []
    while index < len(source):
        if source.startswith(closing, index):
            found.append(Literal(line_of(source, start), "".join(text)))
            return index + len(closing)
        if not raw and source[index] == "\\":
            text.append(source[index : index + 2])
            index += 2
            continue
        text.append(source[index])
        index += 1
    raise LexError(f"line {line_of(source, start)}: unterminated string literal")


def _skip_rust_char_or_lifetime(source: str, index: int) -> int:
    """`index` is at a `'`. Skips a char literal whole, or just the quote of a lifetime."""
    if source.startswith("\\", index + 1):
        close = source.find("'", index + 3)
        if close < 0:
            raise LexError(f"line {line_of(source, index)}: unterminated char literal")
        return close + 1
    if index + 2 < len(source) and source[index + 2] == "'":
        return index + 3
    return index + 1


def _skip_rust_item(source: str, index: int) -> int:
    """Skips the item after a test-only cfg: to its `;`, or past its `{...}` body."""
    depth = 0
    while index < len(source):
        char = source[index]
        if source.startswith("//", index):
            newline = source.find("\n", index)
            index = len(source) if newline < 0 else newline + 1
            continue
        if source.startswith("/*", index):
            index = _skip_block_comment(source, index)
            continue
        if char == "'":
            index = _skip_rust_char_or_lifetime(source, index)
            continue
        literal = _rust_string_start(source, index)
        if literal is not None:
            index = _rust_string(source, index, literal, [])
            continue
        if char.isalnum() or char == "_":
            while index < len(source) and (source[index].isalnum() or source[index] == "_"):
                index += 1
            continue
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1
        elif char == ";" and depth == 0:
            return index + 1
        index += 1
    raise LexError("test-only item runs to the end of the file")


# --------------------------------------------------------------------- Check


def iter_sources(root: Path) -> list[tuple[str, Path]]:
    files: list[tuple[str, Path]] = []
    for kind, bases, suffix in (("swift", SWIFT_ROOTS, ".swift"), ("rust", RUST_ROOTS, ".rs")):
        for base in bases:
            directory = root / base
            if not directory.is_dir():
                continue
            files.extend((kind, path) for path in sorted(directory.rglob(f"*{suffix}")))
    return files


def load_allowlist(path: Path) -> list[dict[str, str]]:
    entries = json.loads(path.read_text(encoding="utf-8"))
    for entry in entries:
        missing = {"path", "literal", "why"} - entry.keys()
        if missing or not entry["why"].strip():
            raise ValueError(f"{path}: every entry needs path, literal and a non-empty why: {entry}")
    return entries


def check(root: Path, allowlist: list[dict[str, str]]) -> tuple[list[str], list[str], int]:
    allowed = {(entry["path"], entry["literal"]) for entry in allowlist}
    used: set[tuple[str, str]] = set()
    findings: list[str] = []
    scanned = 0
    for kind, path in iter_sources(root):
        relative = path.relative_to(root).as_posix()
        source = path.read_text(encoding="utf-8")
        try:
            literals = swift_literals(source) if kind == "swift" else rust_literals(source)
        except LexError as error:
            raise LexError(f"{relative}: {error}") from error
        scanned += 1
        for literal in literals:
            hits = [name for name, pattern in BANNED_PATTERNS if pattern.search(literal.text)]
            if not hits:
                continue
            key = (relative, literal.text)
            if key in allowed:
                used.add(key)
                continue
            findings.append(f"{relative}:{literal.line}: {', '.join(hits)} in {json.dumps(literal.text)}")
    stale = [
        f"{entry['path']}: {json.dumps(entry['literal'])}"
        for entry in allowlist
        if (entry["path"], entry["literal"]) not in used
    ]
    return findings, stale, scanned


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=str(Path(__file__).resolve().parent.parent))
    parser.add_argument("--allowlist", default=None, help=f"default: <root>/{DEFAULT_ALLOWLIST}")
    arguments = parser.parse_args(argv)

    root = Path(arguments.root).resolve()
    allowlist_path = Path(arguments.allowlist) if arguments.allowlist else root / DEFAULT_ALLOWLIST
    try:
        allowlist = load_allowlist(allowlist_path) if allowlist_path.exists() else []
        findings, stale, scanned = check(root, allowlist)
    except (LexError, ValueError, json.JSONDecodeError) as error:
        print(f"check_banned_copy: {error}", file=sys.stderr)
        return 2

    if scanned == 0:
        print(f"check_banned_copy: no Swift or Rust sources under {root}", file=sys.stderr)
        return 2
    for finding in findings:
        print(f"BANNED  {finding}", file=sys.stderr)
    for entry in stale:
        print(f"STALE   allowlist entry matches nothing: {entry}", file=sys.stderr)
    if findings or stale:
        print(
            f"\n{len(findings)} banned capability claim(s) and {len(stale)} stale allowlist "
            "entr(y/ies). Velvt's policy is deterministic: say what it compares or counts, "
            "not that it learns, adapts or predicts. A literal that is not copy goes in "
            f"{DEFAULT_ALLOWLIST} with the reason.",
            file=sys.stderr,
        )
        return 1
    print(f"check_banned_copy: {scanned} files, no banned capability claims in string literals")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
