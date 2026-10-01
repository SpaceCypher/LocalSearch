# Backend Tasks 31-46 Checklist (Code-Verified)

Last updated: 2026-04-14

Status key:
- COMPLETE: implemented and wired
- PARTIAL: implemented but missing planned behavior or integration
- MISSING: not implemented

## Task Status Matrix

| Task | Status | Evidence | Remaining Work |
|---|---|---|---|
| 31 Prefix Cache | COMPLETE | `src/index/trie.rs`, `src/query/executor.rs` | Keep perf guard in CI |
| 32 ResultProvider + normalization | COMPLETE | `src/query/provider.rs`, `src/query/executor.rs` | Add provider-level regression tests |
| 33 Directory proximity signal | COMPLETE | `src/query/ranker.rs`, `src/query/executor.rs` | Tune weights from telemetry |
| 34 Thread registry + watchdog | COMPLETE | `src/metrics/threads.rs`, `src/metrics/watchdog.rs`, `src/metrics/dashboard.rs`, `src/main.rs` | None |
| 35 Fault injector infra | COMPLETE | `src/fault.rs` | Add more injection points beyond current enum |
| 36 System invariants | COMPLETE | `src/metrics/invariants.rs`, `src/startup.rs`, `src/index/segment.rs` | Extend invariant coverage as new subsystems are added |
| 37 Security/permissions model | COMPLETE | `src/fs/permissions.rs` | Extend denylist coverage tests |
| 38 TCC revocation handler | COMPLETE | `src/fs/tcc.rs`, `src/main.rs` | Optional: migrate watcher backend to platform bindings if needed |
| 39 SSD/HDD + cold pread strategy | COMPLETE | `src/resource/storage.rs`, `src/index/segment.rs` | Optional: replace command-based probe with direct system API |
| 40 External/network volume support | COMPLETE | `src/fs/volumes.rs`, `src/query/executor.rs`, `src/main.rs` | Optional: enrich volume metadata (network/share type) |
| 41 Spotlight bootstrap fallback | COMPLETE | `src/query/spotlight_fallback.rs` | Optionally upgrade from `mdfind` bridge to native API bindings |
| 42 Benchmarks + shadow ranking | COMPLETE | `benches/query_latency.rs`, `src/metrics/shadow_ranking.rs` | Add larger corpus benchmark profiles over time |
| 43 content_hash idempotent re-extraction | COMPLETE | `src/extract/client.rs`, `src/ffi.rs` | None |
| 44 Warmup + signal DB bootstrap | COMPLETE | `src/startup.rs`, `src/index/signals.rs` | Optional: add term-to-segment page targeting |
| 45 Health dashboard CLI | COMPLETE | `src/metrics/dashboard.rs`, `src/main.rs`, `src/metrics/collector.rs` | Optional: add thread-registry and power-state panels |
| 46 Final production gate | PARTIAL | `docs/production-gate-2026-04-14.md` | Complete manual gates and store benchmark threshold outputs |

## Active Work Order

1. Task 46: complete remaining manual production gates and attach reports
