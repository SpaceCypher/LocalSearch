This is a full system design doc. Let me work through every section precisely, then render the architecture and state machine diagrams.

---

# LocalSearch — Frontend System Design Document
### v1.0 · Staff-level spec · Implementation-ready

---

## 1. UX Principles & Invariants

### Perceived Latency Goals

The system must feel instant. "Instant" is defined operationally, not aspirationally:

```
Event                              Target         Hard ceiling
──────────────────────────────────────────────────────────────
Keystroke → UI acknowledges        0ms            0ms (synchronous)
Keystroke → first result visible   ≤80ms          150ms
First results → full results       ≤200ms         500ms
Scope/filter change → update       ≤50ms          120ms
Result open (Finder reveal)        ≤16ms UI ack   N/A (OS handles)
```

"Acknowledges" means the UI visually responds — query text updates, stale results dim, a loading indicator activates. The user must never type into a void. Zero-latency acknowledgment of input is non-negotiable; it is implemented synchronously on the main thread before any async work begins.

### Interaction Guarantees

These are invariants, not goals. Any build that violates them has a bug:

**Guarantee 1 — Monotonic responsiveness**: The UI never freezes. Every keypress produces a synchronous visual response within one frame (16.7ms). All network/IPC calls happen off the main thread, always.

**Guarantee 2 — Stale results are never presented as fresh**: When the user modifies the query, the previous result set is immediately visually demoted (dimmed to 40% opacity) before new results arrive. The user always knows they are looking at results for the previous query, not the current one.

**Guarantee 3 — System state is always visible**: Any degraded mode (index paused, low memory, partial results, permission holes) is shown as a persistent, non-blocking indicator. It is never hidden. It is never shown as an error modal that requires dismissal.

**Guarantee 4 — No phantom confidence**: The system never presents a result it cannot deliver. If a file is indexed but its permission state is `REVOKED`, the result shows a lock indicator before the user clicks it — not an error after.

**Guarantee 5 — Cancellation is instantaneous**: When the user types a new character, the previous query's result stream is cancelled before the new one is issued. The user never sees results from a previous query contaminate the current result set. This is enforced in the data layer with a strict query generation counter, not in the UI layer with timing heuristics.

### Rules for User Trust

Trust is built through consistency, not aesthetics. Three rules:

**Never lie about completeness**: If results are partial, say so. Show "Searching…" while streaming, replace with a result count when complete. If the backend is in degraded mode, the result count shows "47 results (index partial)." Never show a confident result count while still streaming.

**Never silently change behavior**: If fuzzy matching is disabled (memory pressure), the search behavior has changed. Show it: the search field badge changes from `~` (fuzzy active) to `=` (exact only). A user who types "receit" and gets zero results deserves to know why.

**Own failures explicitly**: A zero-result state says "No results for [query]" plus, if available, a diagnostic: "Fuzzy matching paused · Tip: check spelling." This is more trustworthy than silent emptiness.

---

## 2. Interaction Model

### Invocation

Global hotkey: `⌥Space` (default, user-remappable). This is distinct from Spotlight (`⌘Space`) to avoid conflict. The window appears at the vertical center, horizontal center of the primary display. It does not appear at cursor position — cursor position varies and introduces visual instability.

On invocation:
- If the window is hidden: appear with a 120ms spring animation, search field focused, previous query text selected
- If the window is visible: bring to front, re-focus search field, select all text
- If the window is visible and focused: dismiss (toggle)

The window is a floating panel (`NSPanel`, `NSWindowStyleMaskNonactivatingPanel`). It does not steal focus from the underlying application. `⎋` always dismisses. This is an invariant.

### Keyboard Navigation — Complete Map

```
Character input    Update query, trigger search
⌫                 Delete character, trigger search
⌥⌫               Delete word
⌘A                Select all query text
↑↓               Move selection through results
→                 Expand result (show metadata panel)
←                 Collapse metadata panel / move cursor in query
⏎                 Open selected result (default action)
⌘⏎               Reveal in Finder
⌥⏎               Copy path to clipboard
⇧⏎               Open with… menu
⌘↓               Open result's containing folder
Tab               Move to scope/filter bar
⇧Tab             Move from scope bar back to query
⌘1-5             Quick-switch result type filter (1=All, 2=Docs, 3=Images, 4=Code, 5=Folders)
⌘,               Open preferences
⌘R               Force reindex current scope
⎋               Dismiss window (from any focus state)
```

Mouse interaction is fully supported but the design does not optimize for it. Every action reachable by mouse is reachable by keyboard. Mouse clicks on results trigger the same action as `⏎`. Hover shows metadata inline without requiring a click. This is a keyboard-first design where mouse is a first-class affordance, not an afterthought.

### Query Lifecycle

```
State: IDLE
  → user types character
    → State: TYPING
      → debounce 80ms (see below)
        → State: SEARCHING
          → results stream in
            → State: RESULTS_PARTIAL (show with streaming indicator)
              → stream complete
                → State: RESULTS_COMPLETE
      → user types another character before debounce expires
        → reset debounce timer, stay in TYPING

State: RESULTS_COMPLETE
  → user types character
    → immediately: dim current results to 40% opacity
    → State: TYPING (previous results visible but demoted)

State: SEARCHING
  → user types character
    → immediately: cancel active query (send cancellation token)
    → dim current results to 40% opacity
    → State: TYPING
```

**Debounce strategy**: 80ms debounce on character input. This is not a fixed delay — it is a trailing-edge debounce. The query fires 80ms after the *last* keystroke. For fast typists (>10 chars/sec), this means the query fires once when they pause, not on every character. For slow typists, it fires almost immediately after each character.

80ms is the threshold because: below 80ms, users perceive the query as firing while still typing (jarring). Above 120ms, users perceive the system as lagging behind their input (sluggish). 80ms sits inside the "instant" perception band.

**Exception**: Prefix cache hits (results for the current prefix are precomputed) bypass the debounce and render immediately at 0ms. The first 2–3 characters of common queries render instantly; the debounce activates when the prefix cache misses.

### Scope and Filter Interaction

The scope bar sits below the search field, above the result list. It is a horizontal strip of toggleable chips:

```
[All]  [Documents]  [Images]  [Code]  [Folders]  [Audio/Video]  |  [This folder ▾]
```

Scope chips are activated by keyboard (`⌘1-5`) or click. Multiple content-type chips can be active simultaneously; they are OR'd. The location chip (`This folder ▾`) opens a dropdown with the current folder hierarchy as options.

Scope changes do not debounce — they apply instantly. The result list updates within one frame of the scope change, using the already-computed result set filtered client-side if the filter is a subset of `All`. For scope changes that require a new backend query (e.g., switching from `All` to `This folder`), the same streaming flow applies.

**Advanced query syntax** (power users):

```
kind:pdf              filter by file type
in:~/Projects         path scope
after:2024-01-01      modified date filter
before:2024-06-01     modified date filter
size:>10mb            file size filter
tag:important         macOS tag filter
content:"exact phrase" phrase search in content
-term                 exclude term
```

Syntax tokens are parsed on the fly and rendered as colored chips in the search field — they do not stay as raw text. A user who types `kind:pdf` sees the text transform into a `PDF` chip in the query field, replacing the raw syntax. This reduces visual noise and confirms to the user that the filter was parsed correctly.

---

## 3. UI Layout System

### Window Structure

```
┌─────────────────────────────────────────────────────────┐  ← 640px wide, variable height
│  [search icon]  [query field ──────────────────] [⌫][×] │  ← 52px fixed
├─────────────────────────────────────────────────────────┤
│  [All] [Docs] [Images] [Code] [Folders] │ [This folder▾]│  ← 36px fixed, scope bar
├─────────────────────────────────────────────────────────┤
│                                                         │
│  result list (variable, max 8 results before scroll)    │  ← 56px per result row
│                                                         │
├─────────────────────────────────────────────────────────┤
│  [status bar: result count · index state · shortcuts]   │  ← 28px fixed
└─────────────────────────────────────────────────────────┘
```

Total window height: 52 + 36 + (N × 56) + 28, capped at 52 + 36 + (8 × 56) + 28 = **564px**. Beyond 8 results the list scrolls; the window does not grow. This cap is deliberate — a window that grows as results load causes visual instability that makes the result list harder to scan.

Window width is fixed at **640px**. Variable-width windows create visual noise as the layout reflows. 640px is narrow enough to not occlude the entire screen, wide enough for full paths and filenames without truncation in most cases.

The window uses `NSVisualEffectView` with `.hudWindow` material for the standard macOS floating panel appearance. This is not aesthetic — it visually communicates "this is a transient overlay, not a persistent window" via the macOS visual vocabulary.

### Result Row Layout

Each result row is 56px tall and uses a strict grid:

```
┌─────────────────────────────────────────────────────────────┐
│ [icon 32×32] [filename ──────────────────] [recency badge]  │  ← row 1: 28px
│              [path ─────────────────────] [type · size]     │  ← row 2: 24px (4px gap)
└─────────────────────────────────────────────────────────────┘
margin: 12px left, 12px right
```

**File icon (32×32)**: `NSWorkspace.icon(forFile:)` at 32pt. This is the actual system icon for the file, not a generic type icon. Users recognize files by their app icon (a Keynote icon looks different from a Pages icon even if both are documents). 32px is the minimum size where icons are distinguishable.

**Filename (row 1)**: SF Pro Text 14px Medium. Truncated in the middle (not the end) for long filenames — `quarterly_report...final_v2.pdf` rather than `quarterly_report_sales_an….pdf`. Middle truncation preserves both the start (which carries meaning) and the extension (which carries type information).

**Recency badge (row 1, right-aligned)**: Relative time in SF Pro Text 12px Regular, muted color. "2h ago", "Yesterday", "Mar 3". Not an absolute timestamp — relative time has lower cognitive load for "how recent is this" judgments. The full timestamp is shown in the metadata panel.

**Path (row 2)**: SF Mono 11px Regular, tertiary color. The path is shortened: `~/Documents/Work/2024/` not `/Users/alice/Documents/Work/2024/`. `~` substitution is non-negotiable — absolute paths with a username prefix add characters with zero information value. Path is truncated on the left if too long: `…/Work/2024/` not `~/Documents/Work/2024/Q…`.

**Type · size (row 2, right-aligned)**: "PDF · 2.3 MB" in 11px Regular, tertiary color. Kept to two tokens maximum — users need type and size, not MIME type and inode number.

### Visual Hierarchy

The hierarchy maps directly to decision priority:

1. File icon — instant type recognition, no reading required
2. Filename — primary identification
3. Match highlights — why this result appeared
4. Recency badge — "is this the recent version I want"
5. Path — "is this the right file or a copy"
6. Type/size — confirmation only, not primary decision

Items 4–6 are rendered in a muted secondary color (`NSColor.secondaryLabelColor`). Items 1–3 are primary color. This two-tier palette ensures the user's eye lands on the identification information first, confirmation information second, without reading everything in the row.

### Metadata Expansion Panel

Pressing `→` or hovering for 400ms expands a detail panel to the right of the result list:

```
┌────────────────────────────────┐
│ [large preview / icon]         │  ← 120×120 QuickLook thumbnail
│                                │
│ full_filename.pdf              │  ← full name, no truncation
│ ~/Documents/Work/2024/         │
│                                │
│ Modified  March 3, 2025 14:22  │
│ Created   January 15, 2025     │
│ Size      2.3 MB               │
│ Kind      PDF Document         │
│ Tags      ■ Work  ■ Invoice    │
│                                │
│ Content preview:               │
│ "…Q3 revenue exceeded target   │
│  by 12%, driven by…"           │
│                                │
│ [Open] [Reveal] [Copy Path]    │
└────────────────────────────────┘
```

The metadata panel is 260px wide. It slides in from the right in 180ms (spring, no bounce). The main window expands from 640px to 900px. The panel appears on the same layer as the result list — it does not overlay it.

QuickLook thumbnail is generated asynchronously. While generating: show the large file icon. Replace with thumbnail when ready. No flash — the icon and thumbnail occupy the same space; the crossfade is 150ms.

---

## 4. Real-Time Data Flow

This is the most critical section. The behavior described here is what determines whether the system feels fast or broken.

### Query State Machine

The query controller maintains a monotonically incrementing `queryGeneration: UInt64`. Every result that arrives is tagged with the generation that produced it. Any result whose generation does not match the current generation is discarded without touching the UI.

```
IDLE ──[user types]──→ TYPING
  TYPING ──[debounce expires]──→ SEARCHING
    emit: generation++, cancel previous stream
    show: stale results at 40% opacity, spinner in search field
  TYPING ──[user types again]──→ TYPING (reset debounce)

SEARCHING ──[first result arrives, gen matches]──→ STREAMING
    clear stale results, show first result at full opacity
    show: "Searching…" in status bar
SEARCHING ──[timeout 150ms, no results]──→ SEARCHING_SLOW
    show: skeleton rows (2 rows) to hold space
    this prevents layout jump when results eventually arrive
SEARCHING ──[user types]──→ TYPING
    cancel current stream, generation++

STREAMING ──[result arrives, gen matches]──→ STREAMING
    append result to list, maintain sort order
    do NOT re-sort entire list on every result (causes flicker)
    use insertion sort — O(n) but n ≤ 20 results shown
STREAMING ──[stream complete]──→ RESULTS_COMPLETE
    replace "Searching…" with "47 results"
    remove spinner
STREAMING ──[user types]──→ TYPING
    cancel stream, generation++

RESULTS_COMPLETE ──[user types]──→ TYPING
    dim results to 40% opacity immediately (sync)
    generation++, start debounce timer
RESULTS_COMPLETE ──[result refinement arrives, gen matches]──→ RESULTS_COMPLETE
    (backend may send ranking updates after initial results)
    animate rank changes: results slide to new position, 200ms spring
```

### Result Insertion Strategy

Results arrive from the backend as a stream: `AsyncStream<SearchResult>`. Each result carries a `rank: Float` that represents its position in the ranked list.

The UI maintains a `displayResults: [SearchResult]` array of max 20 items (8 visible, 12 in scroll buffer). On each arrival:

1. If `displayResults.count < 20`: insert at the rank-correct position using binary search. Animate: the inserted row appears with a 0→1 opacity transition (80ms). Rows below the insertion point shift down (spring, 120ms). No layout recalculation — only the affected rows animate.

2. If `displayResults.count == 20` and new result's rank is better than rank of last item: replace last item. Animate: the replaced row fades out (60ms), new row fades in (80ms). No other rows move.

3. If `displayResults.count == 20` and new result's rank is worse than all displayed: discard silently.

This strategy means the list *settles* progressively rather than *reordering* constantly. The first 3–4 results are almost always the final top results (ranking is stable for high-confidence matches). The list rarely re-sorts after the first 200ms.

**Critical rule**: The selection highlight must follow the selected result if it moves due to ranking insertion. If the user has pressed `↓` to select row 2 and a new result is inserted at rank 1, the selection moves to row 3 (tracking the previously-selected document, not the position). The user's navigation state is preserved across result updates.

### Cancellation

Every query issues a `CancellationToken` to the backend IPC layer. On cancellation:

1. The token is signaled (atomic flag set)
2. The backend IPC layer checks the token on every result batch before writing to the channel
3. The UI layer discards all arrivals from the previous generation (generation check is free — one integer comparison)
4. The IPC channel is not torn down between queries — it stays open to avoid reconnection latency

The frontend does not "wait" for cancellation to complete. It immediately increments the generation counter and begins the new query. Old results from the cancelled query are discarded by the generation check. The backend cleans up asynchronously. This is a **fire-and-forget cancellation model** — correct because the generation check is the source of truth, not the cancellation signal.

---

## 5. Perceived Performance Design

### Before Results Arrive: The 0–80ms Window

The user has typed. The debounce is ticking. No results exist yet. What does the UI show?

**Immediate actions (0ms, synchronous)**:
- New characters appear in the search field instantly (obvious but non-negotiable — any text input lag destroys trust)
- If previous results exist: dim to 40% opacity (one `withAnimation` modifier, no layout change)
- If query field is now empty: snap back to empty state immediately, no fade

**Prefix cache hit (0–15ms)**:
- If the query matches a precomputed prefix cache entry, inject those results immediately
- These are shown at full opacity — they are real results, not placeholders
- Labeled with nothing — they look like any other result
- If the actual query result arrives and differs: animate to new ranking (200ms spring)

**Spinner activation (80ms)**:
- If no results have arrived by 80ms: activate the search field spinner
- The spinner is a `ProgressIndicator` at 12pt, inline in the search field, right side
- It does not replace the query text. It does not move anything. It appears in reserved space.

### Skeleton States

Skeleton rows appear only when:
1. No results are available (not even stale ones), AND
2. More than 150ms have elapsed since the query was issued

Skeleton rows are exactly the same height as real result rows (56px). They show a shimmering rectangle where the filename would be, and a shorter rectangle where the path would be. The shimmer animation runs at exactly 1.2s period — slow enough to not be distracting, fast enough to communicate activity.

**Critical**: Never show skeletons when stale results are visible. Stale results (at 40% opacity) are always better than skeletons — they give the user something to read and predict. Skeletons and stale results are mutually exclusive states.

Show at most **3 skeleton rows**. More skeletons communicate a larger expected result set than is usually true, creating a disappointment effect when only 2 results arrive.

### Progressive Rendering

Results render in strict arrival order. The first result that arrives renders at full opacity immediately. Subsequent results animate in (80ms fade). This creates the perception of the system "filling in" the answer, which is more trustworthy than a blank wait followed by a sudden full list.

The list does not wait to receive all results before rendering. It does not batch results for visual grouping. Every result renders as soon as it arrives, subject to the generation check. This is a streaming-first rendering model.

### Hiding Backend Latency

Four techniques, applied in layers:

**Layer 1 — Prefix cache**: Common queries return instantly from precomputed results. The debounce never fires. The user perceives the system as impossibly fast. This is the most effective technique.

**Layer 2 — Optimistic stale display**: Previous results stay visible (dimmed) while the new query runs. The user is reading something while waiting, not staring at emptiness. This makes 150ms feel like 50ms.

**Layer 3 — Progressive streaming**: First result at 50–80ms; full results by 200ms. The user sees movement immediately. A system that shows one result in 80ms and ten results in 200ms feels faster than a system that shows all ten results at 200ms, even though the final state is identical.

**Layer 4 — Animation timing calibration**: Animations run at 120ms for transitions, 80ms for fades. These are not aesthetic choices — they are calibrated to overlap with backend latency. A 120ms slide animation fills the 80–200ms window where the system is fetching results. The user watches the animation; the results arrive while they're watching. Zero perceived gap.

---

## 6. State & System Feedback

The status bar (28px, bottom of window) is the exclusive channel for system state communication. No modals. No toasts. No banners that overlap results. The status bar always shows something; it is never blank.

### Status Bar States (left to right layout)

```
[result count / status]                    [system state badge]  [shortcut hint]
```

**Result count states**:
```
(empty query)          "Start typing to search"
SEARCHING              "Searching…"
RESULTS_PARTIAL        "Finding results…"
RESULTS_COMPLETE       "47 results"
RESULTS_COMPLETE       "47 results in ~/Documents"     (when scoped)
ZERO_RESULTS           "No results for 'receit'"
ZERO_RESULTS_DEGRADED  "No results · Exact mode active" (fuzzy paused)
```

**System state badge** (right side, appears only when non-nominal):

```
Badge text                   Condition                         Color
──────────────────────────────────────────────────────────────────────
"Index paused"               Indexing suspended                Amber
"Index building (43%)"       First-launch bootstrap            Amber
"Exact only"                 Fuzzy matching evicted            Amber
"Partial results"            Backend in degraded mode          Amber
"No content search"          Content index evicted             Amber
"Results may be stale"       WAL lag > 30s                     Amber
"Network volumes offline"    Mounted volumes disconnected      Gray
```

Amber communicates "behavior has changed, not an error." Gray communicates "a feature is unavailable, not affecting primary use." Red is reserved for actual failures (index corruption, permission revocation) and appears as a `!` badge on the app icon in the Dock, never in the search window.

All badge text is ≤ 20 characters. If multiple conditions are active, show the highest-priority one (priority: index building > partial results > exact only > stale > volumes offline). The full list is accessible via `⌘,` → Status tab.

**Shortcut hint** (rightmost, rotates every 8 seconds):
```
"⌘↵ Reveal in Finder"
"→ Expand details"
"⌥↵ Copy path"
```

This is the system's documentation surface. It rotates through the 6 most relevant shortcuts for the current state. Users learn the keyboard shortcuts through ambient exposure without reading a manual.

### Index Progress During First-Launch Bootstrap

During the first-launch bootstrap (Phase 1 and Phase 2 from the cold-start design), the window shows an additional element below the scope bar:

```
┌──────────────────────────────────────────────────┐
│  Indexing your files · Documents complete         │
│  [■■■■■■■□□□□□□□□□□□□□] 32%   Est. 4 min         │
└──────────────────────────────────────────────────┘
```

This bar appears below the scope bar, pushing the result list down. It has its own 28px height allocation. It shows: a natural language description of what is currently being indexed, a progress bar with percentage, and a rough ETA (computed from indexing rate over the last 30 seconds, displayed as nearest minute).

The bar does not animate or pulse — it is a static progress bar with a moving fill. Animated progress bars create anxiety; static progress bars communicate control.

---

## 7. Ranking and Result Presentation

### Match Highlighting

Every result shows exactly why it matched the query. There are three match sources, each rendered distinctly:

**Filename match**: Matched characters are highlighted with a colored background pill — not bold, not underlined. Pill highlighting (background color change) is scannable at higher speed than weight-based highlighting because it creates a color contrast edge that the eye catches in peripheral vision. Color: `systemOrange` at 30% opacity in light mode, 40% in dark mode.

**Path match**: Matched path components are highlighted with the same pill, but at 70% the opacity of filename highlights — visually subordinate because path matches are lower-value signals than filename matches.

**Content match**: When the match is in file content (not the name or path), show a content snippet below the path row:

```
┌─────────────────────────────────────────────────────────────┐
│ [icon] quarterly_report_final.pdf                    2h ago  │
│        ~/Documents/Work/2024/                       PDF·2MB  │
│        "…revenue exceeded target by 12%, driven by          │  ← snippet row
│           synergy between the sales and…"                    │
└─────────────────────────────────────────────────────────────┘
```

The snippet row adds 20px to the row height (total 76px for content-match rows). It shows at most two lines of content with matched terms highlighted using the same pill style. The snippet is extracted around the first match occurrence, with 40 characters of context on each side.

Content snippet rows appear only when:
1. The query term matched content but NOT the filename
2. The filename alone is insufficient to explain why the file appeared

If the query "synergy" matches both the filename `synergy_memo.pdf` and its content, show only the filename highlight — the content snippet is redundant and adds visual noise.

### Scan Speed Optimization

Users scan result lists in 200–400ms before deciding whether to continue reading or navigate. The layout must support this scan speed.

Three layout decisions that directly serve scan speed:

**Left-anchored icons**: The 32×32 icon sits at the left edge of every row. When scanning vertically, the eye travels down the left column. App icons are recognizable in peripheral vision — a Keynote icon vs a PDF icon vs a folder icon are distinguishable without fixating. The user screens out irrelevant file types before reading any text.

**Consistent row height within type**: Content-match rows (76px) and metadata-only rows (56px) must not be interleaved randomly in the result list. Group them: metadata-only rows first, content-match rows below. This creates visual rhythm — a uniform row height zone followed by a taller zone — that the eye can navigate without recalibrating row height on every fixation.

**Recency signal at far right**: The recency badge ("2h ago") sits at the far right of row 1. When scanning for "the file I was working on today," the eye looks right. When scanning for "the right file by name," the eye reads left-to-right. These two scan patterns do not interfere because their target information is at opposite ends of the row.

### Communicating Ranking

The result list communicates ranking through position only — not through explicit rank labels, not through score badges, not through visual weight changes between items. Rank 1 is at the top; rank N is at the bottom. No other ranking signal is shown in the default view.

The one exception: when a result is boosted by a context signal (active application, time of day, session coherence), show a subtle `★` indicator at the right edge of the filename row. This communicates "this result has been prioritized for your current context" without explaining the mechanism. Hovering the `★` shows a tooltip: "Prioritized because you've been working in this folder." This is transparency without noise — visible for power users who notice it, ignorable for users who don't.

---

## 8. Power User Features

### Filter Syntax (Complete)

Filters are entered inline in the query field or via the scope bar. Inline filters are detected by the query parser and rendered as chips:

```
kind:pdf         → [PDF] chip, amber
kind:image       → [Image] chip, amber
kind:code        → [Code] chip, amber (detects by extension: .swift, .py, .js, .rs, etc.)
kind:folder      → [Folder] chip
in:~/Projects    → [~/Projects] chip, teal
in:.             → [This folder] chip, teal (current Finder selection)
after:today      → [Today] chip, blue
after:yesterday  → [Yesterday] chip, blue
after:2024-01-01 → [Jan 1 2024+] chip, blue
before:2024-06-01→ [Before Jun 2024] chip, blue
size:>10mb       → [>10 MB] chip, gray
size:<1mb        → [<1 MB] chip, gray
tag:red          → [■ Red] chip with color swatch
-term            → [−term] chip, red-outlined
content:"phrase" → ["phrase"] chip, purple (forces content search)
```

Multiple chips stack horizontally in the search field, left-to-right, pushing query text to the right. If chips overflow the field width, they collapse into a `+N more` overflow indicator. The full chip list is visible in the metadata expansion panel.

### Quick Actions

Quick actions are available on the selected result via keyboard or via a `⌘K` command palette:

```
⏎          Open (default app)
⌘⏎         Reveal in Finder
⌥⏎         Copy path to clipboard
⇧⏎         Open with… (system sheet)
⌘C         Copy file (same as ⌘C on a Finder selection)
⌘⌫         Move to Trash
⌘D         Duplicate
⌘T         Add/remove macOS tag (opens tag picker)
⌘I         Get Info (opens Finder info panel)
```

The command palette (`⌘K`) is a secondary search interface that appears when a result is selected. It shows the full action list filtered by the typed string. Typing "re" filters to "Reveal in Finder", "Rename", etc. This is the standard Linear/Raycast command pattern — it is adopted here because it is already part of power users' muscle memory.

### Query History

`↑` in an empty search field navigates backward through query history (last 50 queries, stored in signal DB). This is identical to shell history navigation. Power users use this constantly; casual users never discover it. No UI chrome is added to explain it — it is a discoverable affordance for users who think to try it.

### Saved Searches

`⌘S` on a completed query saves the search as a named smart folder in the scope bar. The scope bar grows a new chip with the saved query name. Clicking it re-runs the query. Saved searches are stored in the signal DB with their full query string and filter set. This feature is not surfaced in onboarding — it is discoverable through the `⌘,` preferences panel.

---

## 9. macOS Integration

### Global Shortcut Registration

Use `MASShortcut` or direct `Carbon` event tap for global shortcut registration. The hotkey must work when the app is in the background, in all spaces, and on all displays. Register it as a `CGEventTap` with `kCGEventTapOptionDefault` — this is the only mechanism that reliably intercepts hotkeys before the system processes them.

The window always appears on the display that contains the mouse cursor at invocation time, not the main display. Users work across multiple monitors; appearing on the wrong display is a friction point that breaks flow.

### Window Behavior

```
Level:           NSWindow.Level.floating
Collection:      .canJoinAllSpaces (appears on all Mission Control spaces)
Activation:      NSPanel with .nonactivatingPanel style mask
Focus:           searchField.becomeFirstResponder() on window appear
Dismiss:         key handler for ⎋; resignFirstResponder() on focus loss
                 (optional, configurable: dismiss-on-focus-loss default ON)
```

The window does not appear in Mission Control. It does not appear in the Dock. It does not appear in the app switcher (`⌘Tab`). It is a tool, not an application window.

### Animations

All animations use `NSSpringAnimation` (or SwiftUI's `spring(response:dampingFraction:)`) with consistent parameters:

```
Window appear:      response: 0.3, damping: 0.85   (slight bounce, fast)
Window dismiss:     response: 0.2, damping: 1.0    (no bounce, snap out)
Result insertion:   response: 0.25, damping: 0.9
Metadata panel:     response: 0.3, damping: 0.85
Selection move:     response: 0.2, damping: 1.0    (no bounce — feels precise)
Scope chip add:     response: 0.2, damping: 1.0
```

`damping: 1.0` on selection movement is critical. A bouncy selection cursor feels playful, not precise. Selection is a navigation tool; navigation requires crisp, immediate response.

All animations respect `NSWorkspace.shared.accessibilityDisplayShouldReduceMotion`. When reduce motion is enabled: replace all animations with instant transitions. No fades, no slides, no springs. Instant.

### Accessibility

VoiceOver support is non-negotiable:
- Search field: `accessibilityLabel = "Search files"`, `accessibilityHint = "Type to search your files"`
- Result rows: `accessibilityLabel = "\(filename), \(fileType), \(recencyString), in \(path)"`
- Status bar: `accessibilityLiveRegion = .polite` — VoiceOver announces status changes without interrupting current speech
- Keyboard navigation is fully accessible by definition (keyboard-first design)
- Dynamic Type: all font sizes scale with system font size preference

---

## 10. Frontend Architecture

### Component Hierarchy

```
SearchWindowController (NSWindowController)
├── SearchWindow (NSPanel)
│   └── SearchRootView (SwiftUI hosting view)
│       ├── QueryFieldView
│       │   ├── SearchIconView
│       │   ├── QueryTextField (NSTextField bridged)
│       │   ├── ChipContainerView [FilterChip]
│       │   ├── SpinnerView (conditional)
│       │   └── ClearButton (conditional)
│       ├── ScopeBarView
│       │   ├── TypeFilterChip (×6)
│       │   └── LocationFilterButton
│       ├── IndexProgressView (conditional, first-launch only)
│       ├── ResultListView
│       │   └── ResultRowView (×N, reused via LazyVStack)
│       │       ├── FileIconView
│       │       ├── FileNameView (attributed, highlighted)
│       │       ├── PathView (attributed, highlighted)
│       │       ├── ContentSnippetView (conditional)
│       │       └── MetadataView (recency, type, size)
│       ├── MetadataPanelView (conditional, slides in)
│       │   ├── PreviewThumbnailView
│       │   ├── FileDetailListView
│       │   ├── ContentPreviewView
│       │   └── QuickActionBarView
│       └── StatusBarView
│           ├── ResultCountLabel
│           ├── SystemStateBadge (conditional)
│           └── ShortcutHintLabel
```

### State Management

Single source of truth: `SearchViewModel` (ObservableObject). All UI state derives from this model. No state lives in view structs.

```swift
@MainActor
class SearchViewModel: ObservableObject {

    // Query state
    @Published var queryText: String = ""
    @Published var parsedFilters: [QueryFilter] = []
    @Published var activeScope: SearchScope = .allLocations

    // Result state
    @Published var displayResults: [SearchResult] = []
    @Published var queryState: QueryState = .idle
    @Published var resultCount: Int? = nil          // nil = still streaming
    @Published var selectedIndex: Int? = nil

    // System state
    @Published var systemState: SystemState = .nominal
    @Published var indexProgress: IndexProgress? = nil  // nil = not bootstrapping

    // Metadata panel
    @Published var expandedResult: SearchResult? = nil

    // Private coordination
    private var queryGeneration: UInt64 = 0
    private var debounceTask: Task<Void, Never>? = nil
    private var searchTask: Task<Void, Never>? = nil
    private var backendChannel: SearchBackendChannel

    enum QueryState {
        case idle
        case typing
        case searching
        case searchingSlow   // > 150ms with no results
        case streaming
        case complete
    }
}
```

All `@Published` properties are mutated only on `@MainActor` (the main thread). The backend channel delivers results on a background actor; they are dispatched to MainActor before updating published properties. This eliminates the class of bugs where background thread updates cause SwiftUI rendering issues.

### Async Data Handling

```swift
func onQueryChange(_ newText: String) {
    // 1. Synchronous: update query text, dim results, update generation
    queryGeneration &+= 1
    let myGeneration = queryGeneration
    dimCurrentResults()

    // 2. Cancel previous work
    debounceTask?.cancel()
    searchTask?.cancel()

    guard !newText.isEmpty else {
        queryState = .idle
        displayResults = []
        return
    }

    queryState = .typing

    // 3. Check prefix cache synchronously
    if let cached = prefixCache.results(for: newText) {
        applyResults(cached, generation: myGeneration)
    }

    // 4. Debounce, then stream
    debounceTask = Task {
        try? await Task.sleep(nanoseconds: 80_000_000) // 80ms
        guard !Task.isCancelled else { return }

        searchTask = Task {
            await runSearch(query: newText, generation: myGeneration)
        }
    }
}

func runSearch(query: String, generation: UInt64) async {
    await MainActor.run { queryState = .searching }

    // Slow search indicator after 150ms
    let slowTask = Task {
        try? await Task.sleep(nanoseconds: 150_000_000)
        guard !Task.isCancelled else { return }
        await MainActor.run {
            if queryGeneration == generation && displayResults.isEmpty {
                queryState = .searchingSlow
            }
        }
    }
    defer { slowTask.cancel() }

    let stream = backendChannel.search(query: query, filters: parsedFilters, scope: activeScope)

    for await result in stream {
        guard queryGeneration == generation else { return } // generation check
        await MainActor.run { insertResult(result) }
    }

    guard queryGeneration == generation else { return }
    await MainActor.run {
        queryState = .complete
        resultCount = displayResults.count
    }
}
```

### Backend Communication Layer

The backend is a local XPC service (`com.localsearch.engine`). The frontend communicates via a typed Swift protocol:

```swift
protocol SearchBackendProtocol {
    func search(
        query: String,
        filters: [QueryFilter],
        scope: SearchScope,
        cancellationToken: CancellationToken
    ) -> AsyncStream<SearchResult>

    func systemState() -> AsyncStream<SystemState>
    func indexProgress() -> AsyncStream<IndexProgress?>
    func prefetchPrefix(_ prefix: String) async
}
```

The `systemState()` and `indexProgress()` streams are established once at app launch and kept open for the lifetime of the window. System state changes arrive on these streams and are applied to the `SearchViewModel` immediately. No polling.

The `prefetchPrefix` call is issued speculatively when the user has typed 1–2 characters, before the debounce expires, to warm the prefix cache on the backend side.

---

## 11. Failure and Edge Case Handling

### Zero Results

```
┌─────────────────────────────────────────────────────────┐
│  [search icon]  receit                                  │
├─────────────────────────────────────────────────────────┤
│  [All] [Docs] [Images] [Code] [Folders] │ [This folder] │
├─────────────────────────────────────────────────────────┤
│                                                         │
│           No results for "receit"                       │  ← 16px Medium
│                                                         │
│           Did you mean: receipt  recite                 │  ← suggestions
│                                                         │
│           Try removing filters or broadening scope      │  ← if filters active
│                                                         │
├─────────────────────────────────────────────────────────┤
│  0 results                             Exact only · ?   │
└─────────────────────────────────────────────────────────┘
```

Spelling suggestions are generated from the BK-tree: the top 3 candidates within edit distance 2 of the query, ranked by corpus frequency. Clicking a suggestion replaces the query text. This is the zero-result state's primary recovery mechanism.

### Slow Backend (>500ms with no results)

After 500ms with no results and no stale results to show:

1. Show 3 skeleton rows (from section 5)
2. Status bar shows "Searching…" with a "This is taking longer than usual" secondary label
3. After 2000ms: show "Search is slow — backend may be under load. Results will appear when ready." The window does not close. The query does not cancel. The user is informed, not blocked.

### Partial Results (Degraded Mode)

When the backend signals `partial_results` in the result stream:

1. Each result row that comes from a partial index shows a subtle `~` prefix on the path: `~/Documents/…` becomes `~~/Documents/…` (two tildes). This is a visual flag that the result came from a potentially incomplete index.
2. Status bar shows "Partial results" badge in amber.
3. A `?` button next to the badge, when clicked, shows a popover explaining what "partial results" means and what caused it.

### Permission Errors

A result row for a file with `REVOKED` permission state:

```
┌─────────────────────────────────────────────────────────────┐
│ [icon, 60% opacity] [🔒] quarterly_report.pdf   [2h ago]   │  ← lock glyph
│                     ~/Documents/Work/    [Permission denied] │  ← inline label
└─────────────────────────────────────────────────────────────┘
```

The result is shown (the user should know the file exists) but with a lock indicator and the path row replaced with "Permission denied." Pressing `⏎` on this row shows an alert: "This file exists but LocalSearch no longer has permission to open it. Open System Settings → Privacy & Security to restore access." with a "Open System Settings" button. No silent failure.

---

## 12. Performance Constraints

### Frame Rate

The UI must maintain 60fps during all normal operations, 120fps on ProMotion displays. The constraint that makes this achievable or impossible is whether the main thread is ever blocked waiting for backend results. With the async architecture defined in Section 10, the main thread is never blocked — it only executes view updates and synchronous state mutations, both of which are bounded O(1) or O(log n) operations.

Concrete rendering budget per frame (at 60fps: 16.7ms total):

```
Operation                              Budget
──────────────────────────────────────────────────────
SwiftUI diff computation               ≤4ms
Layout recalculation                   ≤3ms
Result row render (8 rows × 0.5ms)    ≤4ms
Animation interpolation               ≤2ms
Compositing                           ≤3ms
Headroom                               0.7ms
```

The 0.5ms-per-row budget for result rows means each row must not trigger layout recalculation on every render. Result rows must have **fixed, non-computed heights**: 56px for standard rows, 76px for content-snippet rows. Heights are set as literal constants, not derived from content measurement. SwiftUI `fixedSize()` is used to prevent frame-level height negotiation.

### Rendering Limits

**Maximum simultaneous animations**: 4. More than 4 concurrent animations create visual noise and compete for GPU resources. The system queues animation triggers: when more than 4 results arrive simultaneously, batch-insert them with a single animation, not 8 individual fades.

**Maximum result rows rendered**: 20 in the list, 8 visible without scrolling. The remaining 12 are in a `LazyVStack` — they are not rendered until scrolled into view. On a typical search session where the user finds what they want in the top 3 results, rows 4–20 are never rendered. The `LazyVStack` is not a performance optimization as an afterthought — it is a foundational layout choice.

**Thumbnail generation**: QuickLook thumbnails in the metadata panel are generated asynchronously at 120×120pt. If generation takes >200ms, the panel shows the file icon (which is always available instantly). When the thumbnail arrives, it crossfades in at 150ms. Maximum one thumbnail generation in flight at a time — generating thumbnails for multiple results simultaneously causes I/O contention and thermal pressure.

### UI Process Memory Budget

The UI process (`com.localsearch.app`) has a separate memory budget from the backend engine:

```
Component                    Budget
──────────────────────────────────────────
SwiftUI view tree            ≤8MB
Result model objects (×20)   ≤4MB
Thumbnail cache (LRU, ×20)   ≤40MB (2MB per thumbnail max)
Attributed string cache      ≤6MB (pre-rendered match highlights)
Prefix cache mirror (UI-side)≤8MB (copy of top results per prefix)
Query history                ≤2MB
Total                        ≤68MB
```

The thumbnail LRU cache is capped at 20 entries — exactly matching the max result list size. When the result list changes, the thumbnail cache is cleared of entries not in the new result set. This prevents memory from accumulating across many searches in a long session.

Attributed strings (pre-rendered match highlights) are cached per `(query, doc_id)` pair. Cache hit on result re-display avoids re-running the highlight algorithm. Cache is cleared on every new query. Size is bounded by the result count (20 max × 300 bytes/entry = 6KB in practice; 6MB is a conservative ceiling).

---

## What This System Delivers

A frontend engineer reading this spec can implement the entire system from it. Every ambiguous behavior has a specified answer:

- What happens when the user types faster than the backend responds? (Section 4: generation counter, stale dimming, debounce)
- What does the UI show during the first 80ms after a keystroke? (Section 5: prefix cache, synchronous dim, reserved spinner space)
- How do results animate when new ones arrive mid-stream? (Section 4: insertion sort, opacity fade, selection tracking)
- What does a permission-denied result look like? (Section 11: lock icon, inline label, alert on open)
- How many threads touch `@Published` properties? (Section 10: MainActor only, always)
- What is the memory ceiling for the UI process? (Section 12: 68MB total)

Every decision is tied to exactly one of three things: **performance** (frame rate, latency, memory), **clarity** (scan speed, visual hierarchy, explicit state), or **user trust** (never lie, always indicate uncertainty, own failures). Nothing exists for aesthetic reasons alone.

Now let me render the two diagrams that benefit most from visualization: the query state machine and the component data flow.The generation check box in the center of the diagram is the system's correctness guarantee — every state transition that involves a result arriving from the backend goes through it. Without it, a slow query from 500ms ago can contaminate a fast query from 50ms ago, which is the most common correctness failure in streaming UIs.

The data flow from `SearchViewModel` through to the render layer:The critical path in the data flow diagram is the result return path from the search engine through the generation check back to the ViewModel. That path has two properties that make the system correct: it always passes through `@MainActor` before touching `@Published` properties (preventing threading bugs), and it always checks `queryGeneration` before applying any result (preventing stale result contamination). Every other path in the diagram — system state, index progress, prefix cache — is the same pattern: async arrival, generation or relevance check, MainActor dispatch, published property update, SwiftUI re-render. One pattern, applied consistently, means there is exactly one class of data-flow bug to think about rather than many.