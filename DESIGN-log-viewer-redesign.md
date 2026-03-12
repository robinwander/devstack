# Log Viewer Redesign — Implementation Spec

## Overview

The log viewer evolves from a "tail with search" into a structured log explorer (Datadog-style). Core changes:
- **Table with dynamic columns** replaces flat monospace list
- **Rich search bar** with inline filter tokens and intellisense
- **Persistent facets sidebar** replaces popover overlay
- **Newest-first default sort**
- **Every field value is actionable** — click to filter/exclude/isolate
- **Enhanced sharing** — share specific logs + multi-select rows

## Architecture Principle

**The search string is the single source of truth for all filters.** Facets, detail actions, column clicks — they ALL modify the search input string. The search string is parsed to extract `field:value` tokens which drive API params. This keeps everything in sync and makes the URL shareable. The CLI already supports the same query syntax so CLI/UI parity is preserved.

## Current Codebase

Key files:
- `devstack-dash/src/components/log-viewer.tsx` — Main component (~1340 lines, owns all state)
- `devstack-dash/src/components/log-viewer/log-row.tsx` — Individual log row
- `devstack-dash/src/components/log-viewer/log-detail.tsx` — Expanded JSON detail view
- `devstack-dash/src/components/log-viewer/facet-section.tsx` — Facet filter section
- `devstack-dash/src/components/log-viewer/log-tab-bar.tsx` — Service tabs
- `devstack-dash/src/components/log-viewer/types.ts` — ParsedLog type
- `devstack-dash/src/lib/api.ts` — API client + types
- `devstack-dash/src/styles.css` — Design tokens + CSS

The app uses: React 19, TanStack Query, TanStack Virtual, Tailwind CSS v4, Radix UI, Framer Motion, vanilla-jsoneditor, Lucide icons.

## Feature Specifications

### 1. Search Parser (`lib/search-parser.ts`)

Create a parser that takes a search string and extracts:
- `field:value` tokens (including quoted values like `field:"multi word"`)
- `-field:value` exclusion tokens  
- Free-text segments (everything else)

```typescript
interface ParsedSearch {
  tokens: SearchToken[]
  freeText: string  // non-field text segments joined
}

interface SearchToken {
  field: string
  value: string
  negated: boolean
  raw: string       // original text in search string
  start: number     // position in search string
  end: number
}

function parseSearch(input: string): ParsedSearch
function serializeSearch(parsed: ParsedSearch): string
function addToken(input: string, field: string, value: string): string
function removeToken(input: string, raw: string): string
function replaceAllTokens(field: string, value: string): string
```

The existing `simpleTantivyQuery()` function already partially does this — reuse/extend it. The parser also feeds the intellisense.

### 2. Column Detection (`lib/column-detection.ts`)

Auto-detect useful columns from log entry attributes.

```typescript
interface ColumnConfig {
  field: string          // attribute key or built-in field name
  label: string          // display label
  width: number          // pixel width
  visible: boolean
  builtIn: boolean       // timestamp, service, level, message
}

// Well-known field names that get priority (from logging libraries):
const WELL_KNOWN_FIELDS = [
  'event', 'type', 'action', 'kind', 'name',
  'pid', 'hostname', 'host',
  'request_id', 'trace_id', 'span_id', 'correlation_id',
  'method', 'path', 'url', 'status', 'status_code', 'duration',
  'error', 'err', 'exception',
  'module', 'component', 'logger', 'caller', 'source',
  'toolname', 'sessionid',  // devstack-specific
]

function detectColumns(entries: LogEntry[]): ColumnConfig[]
function loadColumnConfig(storageKey: string): ColumnConfig[] | null
function saveColumnConfig(storageKey: string, config: ColumnConfig[]): void
```

Auto-detection logic:
- Scan all attribute keys across entries
- Rank by: (a) presence in WELL_KNOWN_FIELDS, (b) cardinality between 2-50, (c) frequency >50% of entries
- Fields with only 1 unique value → hidden
- ID-like fields (UUIDs, long hashes) → deprioritized
- Auto-select top 2-3 as visible columns
- Merge with localStorage saved config (user's choices take precedence)
- Storage key: `devstack:columns:${projectDir || sourceName}`

### 3. Dynamic Columns in Log Table

Refactor `log-row.tsx` to render dynamic columns.

**Built-in columns** (always available, some toggleable):
- Line number (always shown, not configurable)
- Timestamp (always shown)
- Service (shown when multiple services)
- Level (shown as an icon/badge in column, not text)
- Message (always last, flex-grows)

**Dynamic columns** (from attributes):
- Rendered between level and message columns
- Show value in monospace text
- Empty cells show `—` in tertiary color
- Column header with name, sortable (click to sort)

**Column header bar:**
- Sticky at top of the virtual scroll container
- Shows column names with resize handles between them
- `+` button at the end to add columns
- Columns are resizable by dragging borders
- Right-click column header → remove column

**Column picker dropdown** (from `+` button):
- Lists all available attributes from the current log data
- Shows cardinality count for each
- Checkboxes for visibility
- Filter/search within the picker
- Grouped: "Suggested" (auto-detected) and "All Fields"

### 4. Rich Search Bar (`log-viewer/search-bar.tsx`)

A new component that replaces the current inline search input.

**Visual Design:**
- Same position in the toolbar as current search
- `field:value` tokens render as inline pill badges within the input area
- Pills show: field name in secondary color, `:`, value in primary color
- Each pill has a tiny `×` to remove
- Free text between pills is editable normally
- Cursor can navigate between pills and text segments

**Technical approach — "pill overlay" pattern:**
Since building a true tokenized editor is complex, use this proven approach:
1. Keep a real `<input>` (or `contenteditable` div) as the underlying element
2. Position pill-styled `<span>` elements inline using CSS
3. The actual input value is always the raw text string (e.g., `event:extract_tool_result level:error some search text`)
4. Parse the input to identify field:value tokens and their positions
5. Render those tokens with pill styling using positioned overlays
6. Focus/cursor management stays native

Alternative simpler approach: Use a `<div contenteditable>` where field:value tokens are `<span>` elements with pill styling. But this is harder to manage with React state.

**Pragmatic recommendation**: Start with the simpler approach of styled text segments in the input. The visual distinction comes from: the input uses a monospace font, and the suggestion dropdown provides the intellisense. Later, evolve to true pills if needed.

**Intellisense dropdown:**
- Opens on focus (showing top suggested fields)
- As user types: if typing matches a field name, show field suggestions
- After typing `field:` → show available values for that field with counts
- After typing `-` → show field names for exclusion
- Arrow keys navigate, Enter/Tab selects
- Clicking a facet value in the sidebar adds it as `field:value` to the search
- Suggestion data comes from facetsQuery

**Remove:** 
- The separate FilterChip UI
- The levelFilter/streamFilter state (these become just tokens in the search string)
- The levelFilter/streamFilter dedicated state management

Wait — actually keep dedicated API params for level and stream since the backend handles them specially. But in the UI, `level:error` in the search bar should be THE way to set them. When parsing search, extract `level:X` and `stream:X` tokens to drive the API params.

### 5. Facets Sidebar

Replace the popover overlay with a persistent left sidebar.

**Desktop layout:**
- Appears to the left of the log table
- Width: ~250px, resizable
- Toggle with `F` shortcut (same as today)
- Shows the same FacetSection components
- Clicking a facet value adds `field:value` to the search bar

**Reuse:** The existing `FacetSection` component is excellent — reuse it as-is.

**Mobile:** Falls back to the current overlay sheet pattern. Detect mobile and render overlay instead of sidebar.

**Remove:** 
- The click-outside backdrop div
- The absolute-positioned aside
- Replace with a flex layout where sidebar is a sibling of the log content

### 6. Newest-First Sort

**Default:** newest-first (descending timestamp).

**Backend:** The API already returns results sorted by `ts_nanos DESC` then reverses client-side. To support newest-first, simply skip the client-side reverse. Add a `sort` parameter concept, but initially just client-side.

**Live mode:** New entries appear at the top. The auto-scroll concept inverts — we're already at the top. The "N new logs" indicator changes to appear at the top.

**Toggle:** A sort direction toggle button in the toolbar (↑↓ icon), or click timestamp column header. Persisted in localStorage.

### 7. Detail Panel Filter Actions

When expanding a log row, each field value in the JSON tree gets hover actions.

**On hover over any value in the JSON editor:**
Show three small icon buttons:
- `+` (Filter to) — adds `field:value` to search
- `−` (Exclude) — adds `-field:value` to search
- `=` (Only this) — clears search, sets to just `field:value`

**Technical:** The vanilla-jsoneditor is read-only. We'd need to either:
(a) Overlay action buttons using CSS positioning on hover
(b) Replace with a custom JSON renderer that includes action buttons
(c) Add actions to the metadata header section only (simpler)

Recommendation: Start with (c) — add a horizontal list of the key attributes below the metadata header, each with filter actions. The full JSON tree stays for viewing, and the actionable summary is above it.

### 8. Enhanced Sharing

**Share from detail:** Add a "Share with Agent" button in the detail panel alongside "Copy JSON". Sends the specific log entry + filter context.

**Multi-select:** 
- Click on line number or use Shift+Click to select row ranges
- Selected rows get a highlight
- Floating action bar: "N selected · [Copy] [Share] [Clear]"
- Copy: JSON array or raw lines
- Share: sends selection to agent

### 9. Line Wrap Toggle

A button in the toolbar (↩ icon) that toggles:
- **Nowrap** (current): `white-space: nowrap`, `overflow: hidden`, `text-overflow: ellipsis` on message column
- **Wrap**: `white-space: pre-wrap`, `word-break: break-word` on message column

The virtualizer uses `measureElement` so variable heights work. Toggle persisted in localStorage.

### 10. Custom Time Range

Extend time picker with a "Custom" option:
- Opens a small popover with two inputs: "From" and "To"
- Inputs accept ISO timestamps or relative expressions ("2h ago", "30m ago")
- Parse with simple regex: `/^(\d+)(s|m|h|d) ago$/`
- For full datetime, use native `<input type="datetime-local">`

## Implementation Order

1. **Foundation**: `search-parser.ts`, `column-detection.ts`, sort direction toggle, line wrap toggle
2. **Table layout**: Refactor log-row to table with dynamic column rendering, column header bar, column picker
3. **Search bar**: Rich search component with intellisense, replace filter chips
4. **Facets sidebar**: Move from popover to persistent sidebar
5. **Detail actions**: Filter actions on field values
6. **Sharing**: Detail share + multi-select + bulk share

## Important Constraints

- The installed dashboard runs from `~/.local/share/devstack/dashboard/` with Vite dev server (HMR disabled)
- After making changes in `devstack-dash/`, copy changed files to the installed location for testing
- TypeScript strict mode — no `any` types
- Tailwind CSS v4 with custom design tokens defined in `styles.css`
- The virtualizer expects absolute-positioned rows — maintain this pattern
- Mobile support is important — test responsive behavior
- Keyboard shortcuts must continue to work
- URL state must be preserved for all filter state
- The CLI produces equivalent queries — maintain CLI/UI parity
