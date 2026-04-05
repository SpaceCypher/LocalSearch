# Findohh — Skills Roadmap
> From design document to production-grade macOS file search system.
> Skills are ordered within each phase from first-to-invoke to last.

---

## Phase 1 — Architecture & Design
> *Goal: Turn the design doc into a structured, reviewable technical blueprint.*

| # | Skill | Purpose in this project |
|---|---|---|
| 1 | `writing-plans` | Convert `main.md` and `implementation_spec.md` into an atomic, phased implementation plan before touching any code |
| 2 | `concise-planning` | Generate a tight, trackable checklist from the plan — one item per shippable unit |
| 3 | `architect-review` | Validate LSM/trie/BK-tree/HNSW design decisions against production systems |
| 4 | `senior-architect` | ADRs, tradeoff matrices, API surface design for the whole system |
| 5 | `architecture-decision-records` | Document every major decision (mmap vs pread, XPC isolation, size-tiered vs level-tiered compaction, etc.) |
| 6 | `c4-context` | High-level C4 diagram: user → LocalSearch app → macOS filesystem |
| 7 | `c4-container` | Container-level: main process, extractor XPC, compaction XPC, signal DB |
| 8 | `c4-component` | Component-level: WAL, delta index, segment set, query executor, BK-tree, prefix cache |
| 9 | `mermaid-expert` | Diagrams for query execution pipeline, memory pressure state machine, threading model, index update flow |
| 10 | `ddd-strategic-design` | Define bounded contexts: Index domain, Query domain, Signal domain, Extraction domain |
| 11 | `ddd-tactical-patterns` | Model aggregates (Segment, Document, WALEntry) and repository patterns |

---

## Phase 2 — Deep Technical Planning
> *Goal: Specify data formats, interfaces, and performance contracts before writing a line of Rust.*

| # | Skill | Purpose in this project |
|---|---|---|
| 1 | `database-design` | Design signal DB schema (click history, query stats), WAL binary format, segment file layout with version headers |
| 2 | `api-patterns` | Design `ResultProvider` trait interface, XPC channel message contracts, `FaultInjector` API |
| 3 | `embedding-strategies` | Plan v2 semantic layer: MiniLM 4-bit quantization, HNSW parameters, score normalization to [0,1] |
| 4 | `backend-architect` | Multi-process architecture: privilege separation between main/extractor XPC/compaction XPC |
| 5 | `performance-profiling` | Pre-plan latency budgets per query component (tokenizer 1ms, BK-tree 3ms, etc.), benchmark corpus design |
| 6 | `performance-engineer` | Define P50/P95/P99 SLAs, measurement methodology, regression alert thresholds |

---

## Phase 3 — Core Implementation
> *Goal: Build the v1 keyword search stack — FSEvents → WAL → delta index → query executor.*

| # | Skill | Purpose in this project |
|---|---|---|
| 1 | `rust-pro` | Primary implementation language — LSM engine, BK-tree, radix trie, arena allocator, RwLock threading model, XPC bindings |
| 2 | `rust-async-patterns` | Async event processing from FSEvents, bounded MPSC channel between main and extractor XPC, WAL async flushing |
| 3 | `database` | SQLite WAL-mode signal DB — click history, per-query metrics ring buffer, permission audit log |
| 4 | `bash-scripting` | Build scripts, corpus generation for test harness, CI automation, benchmark runner |
| 5 | `bash-linux` | macOS-specific shell: `lstat`, `madvise`, `setiopolicy_np`, `IOServiceGetMatchingService` integration scripts |
| 6 | `tmux` | Multi-process dev environment: watch main process + extractor XPC logs + compaction scheduler simultaneously |
| 7 | `subagent-driven-development` | Run independent implementation tasks (BK-tree, WAL, trie) in parallel sessions |

---

## Phase 4 — Testing & Reliability
> *Goal: Validate correctness, catch silent divergence between index and disk, benchmark performance.*

| # | Skill | Purpose in this project |
|---|---|---|
| 1 | `tdd-workflow` | RED-GREEN-REFACTOR for every data structure: BK-tree edit distance, WAL replay, Roaring bitmap ops, BM25 scorer |
| 2 | `tdd-workflows-tdd-red` | Write failing tests first: WAL monotonicity invariant, suffix array consistency, doc_id uniqueness |
| 3 | `tdd-workflows-tdd-green` | Implement minimal code to pass invariant tests |
| 4 | `tdd-workflows-tdd-refactor` | Refactor after green: eliminate malloc on query hot path, squeeze delta lock hold time |
| 5 | `systematic-debugging` | Structured debugging for FSEvents edge cases (MUST_SCAN_SUBDIRS storm, coalesced event loss) |
| 6 | `debugger` | Memory issues (mlock failures, arena overflows), lock contention, page fault spikes mid-query |
| 7 | `vibe-code-auditor` | Audit rapidly written index code for fragility: phantom doc_ids, incorrect edit distance on UTF-8, stale signal poisoning |
| 8 | `code-reviewer` | Full 16-pass adversarial review before any v1 release |

---

## Phase 5 — Security & Permissions
> *Goal: Implement TCC integration, privilege separation, restricted path enforcement.*

| # | Skill | Purpose in this project |
|---|---|---|
| 1 | `security-auditor` | Audit TCC model, XPC sandbox entitlements, privilege separation between processes, restricted path list completeness |
| 2 | `differential-review` | Security-focused review on every PR touching the extractor XPC, file access code, or TCC revocation handler |
| 3 | `gdpr-data-handling` | Ensure signal DB (click history, query logs) respects user privacy — local-only, no telemetry, retention limits |
| 4 | `secrets-management` | If any signing certificates or entitlement keys end up in config or environment |

---

## Phase 6 — Observability & Hardening
> *Goal: The index watches itself, detects its own corruption, surfaces degradation before users notice.*

| # | Skill | Purpose in this project |
|---|---|---|
| 1 | `analytics-tracking` | Design behavioral recall proxy: reformulation rate, click-at-rank-1-3 success rate, zero-result rate |
| 2 | `distributed-tracing` | Trace query lifecycle across threads: tokenizer → BK-tree → inverted index → delta merge → scorer → snippet |
| 3 | `prometheus-configuration` | Expose index health metrics (WAL lag, segment count, phantom rate, stale rate) as Prometheus metrics for the debug panel |
| 4 | `grafana-dashboards` | Build the developer debug panel: HEALTHY/DEGRADED/RECONCILING status, P50/P99 trends, zero-result rate alerts |
| 5 | `llm-evaluation` | Evaluate semantic search quality (v2) — precision@10, recall proxy, RBO comparison between ranking versions |

---

## Phase 7 — Migration, Polish & Release
> *Goal: Safe upgrades, rollback capability, App Store / Gatekeeper distribution.*

| # | Skill | Purpose in this project |
|---|---|---|
| 1 | `database-migrations-migration-observability` | Monitor Tier 2/3 index format migrations — detect stalls, rollback triggers |
| 2 | `documentation` | Generate API docs for `ResultProvider` trait, WAL binary format spec, XPC message schema |
| 3 | `app-store-changelog` | Generate user-facing release notes from git history |
| 4 | `vercel-deployment` | N/A for macOS app — skip |

---

## Recommended Invocation Order (First Sprint)

```
1. writing-plans          → produce implementation_plan.md from main.md and implementation_spec.md
2. concise-planning       → produce task.md checklist
3. architect-review       → validate v1 scope (keyword stack only, no embeddings)
4. rust-pro               → scaffold project structure + Tier 1 (FSEvents + WAL)
5. tdd-workflow           → BK-tree unit tests before implementation
6. tdd-workflows-tdd-red  → WAL invariant tests (monotonicity, checksum)
7. rust-pro               → delta index + query executor
8. systematic-debugging   → handle first FSEvents edge cases
9. vibe-code-auditor      → audit before any external user sees the code
```

---

## Skills NOT Used (and Why)

| Skill | Reason skipped |
|---|---|
| `nextjs-best-practices` | No web frontend — macOS native UI only |
| `react-patterns` | Not applicable |
| `expo-*` | Mobile-only skills, not relevant |
| `azure-*` / `aws-*` | Fully local system, no cloud infrastructure |
| `shopify-*` | Not applicable |
| `flutter-expert` | Not applicable |

---

*Last updated: 2026-04-06*
*Project: findohh / LocalSearch — macOS Production Search System*
