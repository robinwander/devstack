/**
 * Log Viewer Redesign Chain
 *
 * Transforms the log viewer from a flat tail into a structured log explorer.
 * Design spec: DESIGN-log-viewer-redesign.md
 *
 * Model assignment:
 *   - GPT-5.4: backend work, foundational libraries, detail actions
 *   - Opus design: complex frontend UI (columns, search bar, facets)
 *   - QA (t2 + browser): parallel browser + CLI testing
 */
export default ({ sequence, loop, parallel, coding, design, qa, review, gate }) => {
  const gpt5 = coding({ provider: "openai-codex", model: "gpt-5.4" });
  const opus = design();
  const qaAgent = qa();
  const opusGate = gate({ provider: "anthropic", model: "claude-opus-4-6" });
  const codexGate = gate({ provider: "openai-codex", model: "gpt-5.4" });

  return sequence([
    // ─── Phase 1: Foundation libs + sort + line wrap ───
    // GPT-5.4 — pure logic, no design
    {
      preset: gpt5,
      task: `Build the foundation for the log viewer redesign described in DESIGN-log-viewer-redesign.md.

Create:
1. A search parser library (see "Search Parser" section of the design doc) — parses search strings into structured field:value tokens, handles negation, quoted values, and free text. Write thorough tests covering edge cases.
2. A column detection library (see "Column Detection" section) — auto-detects useful columns from log entry attributes, ranks by usefulness, persists config to localStorage. Write tests.
3. Sort direction toggle in log-viewer.tsx — newest-first default, persisted in localStorage, with a toolbar button.
4. Line wrap toggle in log-viewer.tsx — controls white-space on message content, persisted in localStorage, with a toolbar button.

Read the design doc for the full spec on each. Read the existing log-viewer.tsx and log-row.tsx to understand the current architecture.

<verification_loop>
- cd devstack-dash && pnpm exec tsc --noEmit
- cd devstack-dash && pnpm test
- Verify the parser handles: "event:extract_tool_result level:error free text", negation, quoted values
</verification_loop>`,
      output: "progress-foundation.md",
      artifacts: [{ key: "foundation", from: "progress-foundation.md", required: true }],
      maxEndCheckRetries: 2,
    },

    // ─── Phase 2: Dynamic column table layout ───
    // Opus design — complex frontend architecture
    {
      preset: opus,
      task: `Refactor the log viewer into a table with dynamic configurable columns. Read DESIGN-log-viewer-redesign.md sections "Dynamic Columns" and "Column Detection" for the full spec.

Read the progress artifact from phase 1 to understand the foundation libs that were built.

The log viewer currently renders a flat monospace list. Transform it into a proper data table where attribute columns are auto-detected from structured log data, users can add/remove/reorder columns, and settings persist per-project.

Key goals:
- Column header bar with built-in columns (timestamp, service, level) + dynamic attribute columns + message (always last, flex-grows)
- Column picker UI (+ button) showing available attributes with cardinality
- Level column as a colored badge, not text
- Column state persisted in localStorage
- Preserve: virtualized scrolling, expand-on-click, service colors, error/warn tinting, search highlighting, all keyboard shortcuts

After changes, verify by running typecheck and viewing http://localhost:47832/?source=pi in the browser to confirm columns render for structured logs.

Copy changed source files to ~/.local/share/devstack/dashboard/src/ so the live dashboard picks them up.`,
      consumes: ["foundation"],
      output: "progress-columns.md",
      artifacts: [{ key: "columns", from: "progress-columns.md", required: true }],
      maxEndCheckRetries: 2,
    },

    // ─── Phase 3: Rich search bar + facets sidebar ───
    // Opus design — most complex UI piece
    {
      preset: opus,
      task: `Build the rich search bar with intellisense and convert facets from a popover to a persistent sidebar. Read DESIGN-log-viewer-redesign.md sections "Search Bar with Intellisense" and "Facets Sidebar" for the full spec.

Read progress artifacts from previous phases to understand what's been built.

Search bar goals:
- Inline pill/badge treatment for field:value tokens in the search input (the Datadog pattern)
- Contextual intellisense: suggest field names on focus, suggest values after "field:", with counts from facet data
- The search string is the single source of truth for ALL filters — remove separate levelFilter/streamFilter state and FilterChip component
- E/W shortcuts toggle level:error / level:warn tokens in the search string

Facets sidebar goals:
- Desktop: persistent collapsible left sidebar (~250px) alongside log content, toggled with F
- Mobile: falls back to overlay (reuse existing FacetSection component)
- Clicking a facet value adds/removes a token in the search bar

After changes, verify by running typecheck and testing the search + facets in the browser at http://localhost:47832/?source=pi.

Copy changed source files to ~/.local/share/devstack/dashboard/src/.`,
      consumes: ["foundation", "columns"],
      output: "progress-search.md",
      artifacts: [{ key: "search", from: "progress-search.md", required: true }],
      maxEndCheckRetries: 2,
    },

    // ─── Phase 4: Detail actions + sharing + custom time range ───
    // GPT-5.4 — well-defined features needing persistence
    {
      preset: gpt5,
      task: `Add filter actions to the detail panel, enhance sharing, add row selection, and add custom time ranges. Read DESIGN-log-viewer-redesign.md sections "Filter-from-Detail", "Enhanced Sharing", and "Custom Time Range" for the full spec.

Read progress artifacts from previous phases.

Goals:
1. Detail panel: actionable attributes with filter-to (+), exclude (−), and only-this (=) buttons that modify the search bar
2. Share from detail: "Share with Agent" button sends specific log entry to active agent
3. Row selection: Shift+Click for range select, floating action bar with Copy/Share/Clear
4. Custom time range: extend time picker with a Custom option supporting relative expressions like "2h ago"

After all changes, copy source files to the installed dashboard:
  rsync -a devstack-dash/src/ ~/.local/share/devstack/dashboard/src/ --exclude node_modules

<verification_loop>
- cd devstack-dash && pnpm exec tsc --noEmit
- cd devstack-dash && pnpm test
- Verify filter actions modify the search bar correctly
</verification_loop>`,
      consumes: ["foundation", "columns", "search"],
      output: "progress-detail.md",
      artifacts: [{ key: "detail", from: "progress-detail.md", required: true }],
      maxEndCheckRetries: 2,
    },

    // ─── Phase 5: QA + Review loop ───
    loop([
      // Parallel QA: browser + CLI
      parallel([
        {
          preset: qaAgent,
          task: `QA the log viewer redesign — browser testing. The dashboard is live at http://localhost:47832/ (do NOT try to start the server, it's already running).

Read DESIGN-log-viewer-redesign.md to understand what was built.

Test these features using the browser:

1. **Dynamic columns** — Open http://localhost:47832/?source=pi. Verify column headers, attribute columns render, "+" picker works, columns persist on reload.
2. **Search intellisense** — Click search bar, verify field suggestions appear. Type "event:" and verify value suggestions with counts. Select one, verify results filter.
3. **Facets sidebar** — Press F. Verify it's a sidebar (not a popover overlay). Click a value, verify it adds to search bar. Both sidebar and logs visible simultaneously.
4. **Newest-first sort** — Verify newest entries are at the top by default. Toggle sort direction.
5. **Line wrap** — Find a long line, toggle wrap, verify it wraps.
6. **Detail actions** — Click a row to expand. Verify filter action buttons appear on attributes. Click + to filter.

For each issue: describe the problem, rate P0/P1/P2/P3.
If a feature works, explicitly confirm it passes.`,
          output: "qa-browser.md",
          artifacts: [{ key: "qa-browser", from: "qa-browser.md", required: true }],
        },
        {
          preset: qaAgent,
          task: `QA the log viewer redesign — detail features + CLI testing. The dashboard is live at http://localhost:47832/ (do NOT try to start the server, it's already running).

Read DESIGN-log-viewer-redesign.md to understand what was built.

Browser tests:

1. **Row selection** — Shift+Click to select a range. Verify floating action bar with Copy/Share/Clear. Test Ctrl+Click to toggle individual rows.
2. **Share from detail** — Expand a log row. Verify "Share with Agent" button exists alongside Copy.
3. **Custom time range** — Check for a Custom option in the time picker. Try relative expressions like "2h ago".
4. **Keyboard shortcuts** — Verify: / focuses search, E toggles level:error in search, W toggles level:warn, F toggles facets sidebar, 1-9 for services.
5. **Run view** — Navigate to http://localhost:47832/?run=dev-2ee83e7d. Verify columns, search, facets all work for run-based logs too.

CLI tests (do NOT restart daemon or stop runs):
- Run: devstack logs --help — verify options exist
- Run: devstack logs --search "event:extract_tool_result" --last 5 — verify structured query works

For each issue: describe the problem, rate P0/P1/P2/P3.`,
          output: "qa-detail.md",
          artifacts: [{ key: "qa-detail", from: "qa-detail.md", required: true }],
        },
      ], { concurrency: 2 }),

      // Gate — Opus evaluates
      {
        preset: opusGate,
        task: `Evaluate both QA reports to decide if the redesign is ready.

Also verify yourself:
- cd devstack-dash && pnpm exec tsc --noEmit
- cd devstack-dash && pnpm test

Pass if: typecheck clean, tests pass, no P0 issues, core features work (columns render, search intellisense functions, facets is a sidebar not popover, newest-first sort works). A few P1s are acceptable.

Fail if: typecheck errors, test failures, any P0, or core features fundamentally broken. List exactly what needs fixing.`,
        consumes: ["qa-browser", "qa-detail"],
        gate: true,
      },

      // Fix issues
      {
        preset: opus,
        task: `Read both QA reports and fix all P0 and P1 issues. The QA reports describe what's broken and which features are affected.

Read the relevant source files based on what the QA reports flag. Fix each issue, run typecheck after each fix, and commit separately.

After fixes, copy to installed dashboard:
  rsync -a devstack-dash/src/ ~/.local/share/devstack/dashboard/src/ --exclude node_modules`,
        consumes: ["qa-browser", "qa-detail"],
        maxEndCheckRetries: 2,
      },
    ], { maxIterations: 3 }),
  ]);
};
