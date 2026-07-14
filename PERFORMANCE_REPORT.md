# Performance Report — velvt-mac MVP Integration

**Testing environment:** macOS sandbox CI runner, x86_64, no GUI Xcode
Instruments available, no real network egress. Rust `cargo build --release`
(rustc via rustup, toolchain matching `rust-toolchain.toml`). Measurements
below are real numbers from this environment, not estimates — but several
of the brief's required measurements (Instruments-based CPU/RSS profiling,
real ONNX model inference, the full 10-minute simulated-activity run) could
not be produced here and are marked **NOT MEASURED** with the reason, rather
than fabricated.

---

## P1 — No polling loops

**Method:** built `cargo build --release`, ran the real `velvt-service`
binary against a fresh in-`/tmp` SQLite database and socket path, sampled
CPU/RSS via `ps` at startup, after 2s idle, and after 7s idle, then sent
SIGTERM and confirmed clean shutdown.

| Sample | CPU% | RSS |
|---|---|---|
| t+2s | 0.0% | 6.72 MB |
| t+7s | 0.0% | 6.86 MB |

Shutdown logged `shutdown signal received reason="sigterm"` and the process
exited cleanly with no orphaned child.

**Assessment:** PASS for the window measured (well under the 0.5% target;
RSS essentially flat). **Caveat:** this is a 7-second idle window, not the
brief's full 60-second run, and uses `ps` sampling rather than an
Instruments time-profiler trace (no GUI Instruments available in this
sandbox) — a sampling profiler would be needed to definitively rule out a
very-low-frequency timer the 7-second window could miss. Code inspection
supports the result: every background loop in `main.rs` (`fetch_scheduler`,
`upload_coordinator.run_retry_loop`, `retention_scheduler`, the new
periodic flush task, the new auth-state watcher) is driven by
`tokio::select!` over either a `tokio::time::sleep` at a configured
multi-second-or-longer interval, or a `watch::Receiver::changed()` /
`Notify` wakeup — none of them spin.

## P2 — Classification latency

**Method:** ran the existing `tests/abstraction_engine.rs` and
`tests/embedding_similarity.rs` performance tests in `--release` mode with
`--nocapture` to print the real percentiles (these tests already implement
the exact 10,000/1,000-iteration methodology the brief specifies).

**Tier 1 (10,000 iterations, real seed-dictionary classification):**

| Metric | Measured | Gate | Result |
|---|---|---|---|
| mean | 4.46 µs | — | — |
| p50 | 4.45 µs | < 0.5 ms | PASS |
| p95 | 4.86 µs | < 1 ms | PASS |
| p99 | 5.13 µs | < 2 ms | PASS |

**Tier 2 (1,000 iterations):**

| Metric | Measured | Gate | Result |
|---|---|---|---|
| p50 | 21.6 µs | < 10 ms | PASS |
| p95 | 42.3 µs | < 25 ms | PASS |
| p99 | 80.1 µs | < 40 ms | PASS |

**Caveat:** the Tier 2 numbers above use the test suite's `FakeEmbeddingModel`-style
in-memory stand-in (no `onnx` Cargo feature build, no real ONNX Runtime
session, no real INT8-quantized model file present in this sandbox). They
demonstrate the plugin/threading/timeout overhead around inference is
negligible, but **do not** validate real `ort` crate + real
`sentence-transformers/all-MiniLM-L6-v2` model latency. That requires
building with `--features onnx` and a real model artifact, which is outside
what this sandbox can do (no model file, no verified ONNX Runtime
installation here).

## P3 — IPC round-trip latency

**NOT MEASURED.** No existing test isolates "RawEvent injection at
FakeCollectionAgent → AbstractedEvent persisted in SQLite" as a single
timed metric; `tests/ipc_connection.rs` and `tests/push_delivery.rs` test
correctness over an in-memory duplex socket but do not assert a latency
budget. Given the real Tier 1 classification latency above (microseconds)
and the in-memory `RawEventRepo::insert` SQLite write (sub-millisecond per
`tests/persistence_contract.rs` timing), the architecture has ample margin
under the 50ms p95 target, but this is an inference from component
latencies, not a measured end-to-end number. Closing this gap is in scope
for the Phase 2 end-to-end test suite (Path 1/Path 5) — see "Known
limitations" in the PR description.

## P4 — SQLite query performance

**NOT MEASURED.** No test currently seeds a simulated 30-day dataset
(~170,000 raw events, ~5,000 batches) and runs `EXPLAIN QUERY PLAN` against
the five named queries. `tests/persistence_contract.rs` does verify (via
`raw_event_repo_contract_and_timestamp_query_uses_index`) that the
retention-cutoff query path uses an index, and the schema migrations
(`0001_initial_persistence.sql`, `0002_harden_indexes_and_probe.sql`)
define indexes on every documented hot-path column, but no test exercises
them at the 30-day data volume the brief specifies. This is real,
unfinished work, not a pass/fail claim either way.

## P5 — Memory footprint

**Rust service, measured:** 6.7–6.9 MB RSS over a 7-second idle window (see
P1 table) — well under the 50 MB budget. Not measured under the brief's
full "10 minutes, one event every 5 seconds" load profile; given Tier 1
classification allocates no persistent buffers per event and SQLite pages
are bounded by `PRAGMA` defaults, there is no specific reason to expect
growth, but this is not a substitute for the actual sustained-load
measurement.

**Swift app:** **NOT MEASURED.** No GUI session is available in this
sandbox to launch `velvt-mac.app` and sample its RSS (the build does
succeed — see the PR's "How to verify locally" section — but running and
profiling a macOS GUI app requires a real desktop session this environment
doesn't have).

---

## Summary

| Assertion | Status | Note |
|---|---|---|
| P1 No polling | PASS (partial window) | 7s sample, not 60s; no Instruments trace |
| P2 Tier 1 latency | PASS | real measurement |
| P2 Tier 2 latency | PASS (fake model) | real model latency unverified |
| P3 IPC round-trip | NOT MEASURED | no isolating test exists yet |
| P4 SQLite at scale | NOT MEASURED | no 30-day seed dataset test exists yet |
| P5 Rust RSS | PASS (partial window) | 7s sample, not 10 min |
| P5 Swift RSS | NOT MEASURED | no GUI session available |

No optimization was required to meet any of the thresholds that were
actually measured — Tier 1/Tier 2 classification and idle CPU/RSS both
have comfortable headroom against their budgets.
