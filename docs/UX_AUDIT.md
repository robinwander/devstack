# Devstack Dashboard — Comprehensive UX Audit

**Date**: 2025-03-08
**Scope**: Full visual and code audit of `devstack-dash` web dashboard
**Method**: Code review + live browser inspection at `localhost:47832`

---

## Anti-Patterns Verdict: FAIL — This Looks AI-Generated

**If someone said "AI made this," would you believe them immediately? Yes.**

This is a textbook example of the 2024-2025 AI dark-mode dashboard aesthetic. Every single tell is present:

| AI Slop Tell | Present? | Details |
|---|---|---|
| **Dark mode with glowing accents** | ✅ YES | Teal/cyan primary on near-black background. `oklch(0.78 0.12 180)` — the exact cyan-on-dark that screams AI |
| **Glassmorphism / glow effects** | ✅ YES | `header-glow`, `status-glow-green/amber/red`, `glow-primary`, `noise-bg` texture overlay |
| **Neon accents on dark backgrounds** | ✅ YES | Emerald-400 status dots with glowing box-shadows, teal primary throughout |
| **Zero border-radius everywhere** | ✅ YES | `--radius: 0` — every element is razor-sharp rectangles. Feels like a "hacker terminal" vibe-code aesthetic |
| **Monospace typography as "developer" shorthand** | ✅ YES | Excessive monospace: search input, run IDs, facets, log content, URLs, timestamps, line numbers — nearly everything |
| **Cards without purpose** | ✅ YES | `bg-card/50`, `bg-secondary/50` used as subtle card containers where spacing alone would suffice |
| **Generic font choice** | ⚠️ PARTIAL | Outfit is a reasonable variable font but used blandly. No typographic personality or hierarchy beyond weight |
| **Same spacing everywhere** | ✅ YES | Monotonous `px-3 py-2.5` / `px-4 py-3` pattern repeated everywhere — no spatial rhythm |

**The core problem**: This dashboard has been optimized to *look like a developer tool* rather than *be a great developer tool*. It adopted a dark-mode terminal aesthetic as a substitute for actual design decisions. The teal-on-black palette, the noise texture, the glow effects, the zero border radius — these are cosmetic affectations, not functional choices.

---

## Executive Summary

**Total issues: 47** (8 Critical, 14 High, 16 Medium, 9 Low)

### Top 5 Most Critical Issues

1. **No visual identity or design direction** — The app has zero brand personality. It looks like every other AI-generated dark dashboard from 2024.
2. **Brutally poor information density in the log viewer** — The core feature of the app (viewing logs) wastes enormous amounts of horizontal space and has serious readability problems.
3. **Service panel is an awkward, under-designed sidebar** — Inconsistent spacing, unclear hierarchy, globals section is visually orphaned, action buttons are hard to discover.
4. **Color palette is monotone teal-on-black** — No color differentiation beyond status indicators. Everything blends together into an undifferentiated dark mass.
5. **Toolbar area is visually overwhelming and poorly structured** — Search bar, time range, tabs, facets toggle, and scroll controls all compete in a cramped, confusing strip.

### Quality Score: 3/10

Functionally complete but aesthetically generic and experientially mediocre. The app works — the data model, keyboard shortcuts, URL state, and real-time updates are well-engineered — but the visual design actively undermines the UX.

---

## Detailed Findings

### CRITICAL Issues

#### C1. Zero Design Identity — Generic AI Terminal Aesthetic
- **Location**: Global — `styles.css`, all components
- **Category**: Visual Design / Brand
- **Description**: The entire app is a teal-cyan-on-near-black dark mode dashboard with zero border radius, glow effects, noise textures, and monospace everywhere. It has no distinctive visual identity. It looks like every Vercel/Cursor/Raycast-inspired AI project from 2024.
- **Impact**: Users feel no connection to the product. The dashboard is forgettable and indistinguishable. For a tool developers live in all day, this matters enormously.
- **Recommendation**: Define a real design direction. This is a *developer operations tool* — it should feel precise, confident, and professional, not "hacker movie terminal." Consider: Stripe's dashboard (clean, functional, light-mode-first), Linear (restrained dark mode with real hierarchy), or Grafana (information-dense but readable). Pick a direction and commit.

#### C2. Log Viewer Readability — The Core Feature Is Broken
- **Location**: `log-viewer.tsx`, `log-row.tsx`
- **Category**: Information Design / Usability
- **Description**: The log viewer — the primary reason this app exists — has critical readability issues:
  - **Line numbers are nearly invisible**: `oklch(0.28 0 0)` on `oklch(0.07 0.003 260)` background — extremely low contrast
  - **Timestamps are washed out**: `text-muted-foreground/40` (40% opacity of already-muted color) is nearly invisible
  - **INFO text is also low contrast**: `text-foreground/65` means the *most common* log content is at 65% opacity
  - **No clear visual separation between log lines** — relies on subtle row-tint alternation that's barely perceptible
  - **Horizontal overflow**: Long log messages don't have clear truncation or scroll affordance
  - **Service name column is hidden for consecutive same-service lines** — `text-transparent` trick means the column width is reserved but invisible, creating dead space
- **Impact**: Developers scan hundreds of log lines per session. Poor contrast means eye strain, missed errors, and slower debugging.
- **WCAG**: Multiple contrast ratio failures. Line numbers (~1.5:1 ratio) fail even WCAG A. Timestamps (~2:1) also fail.

#### C3. Service Panel — Unclear Hierarchy and Wasted Space
- **Location**: `service-panel.tsx`, `service-row.tsx`
- **Category**: Information Architecture / Layout
- **Description**: The left sidebar is a grab bag:
  - Run ID at top (`voice-b5a7fe4a`) in tiny mono text — what is this? Why is it the first thing I see?
  - Stop/Kill buttons are tiny icon-xs (24px) with no labels — destructive actions hidden behind cryptic icons
  - Service rows have inconsistent height: services with URLs get extra vertical space, services without don't
  - Action buttons (copy, open, restart) appear at 60% opacity by default — hard to discover
  - URL text at 11px in `muted-foreground/45` — practically invisible
  - "GLOBALS" section at the bottom is disconnected from the services above, with a different visual treatment (no action buttons, different spacing)
  - Project path at very bottom in 11px text — wasted footer space
- **Impact**: The sidebar should be a quick-reference panel — glance at status, click to filter. Instead it requires careful scanning to parse.

#### C4. Toolbar Visual Overload
- **Location**: `log-viewer.tsx` toolbar section
- **Category**: Layout / Visual Hierarchy
- **Description**: The toolbar area between the header and log content crams too much into too little space:
  - Search bar + regex toggle + time range selector (all in one row on desktop)
  - Tab bar + line count + facets toggle + scroll control (all in another row)
  - Two borders separate these (border-b border-border on toolbar, border-b border-border/50 on search row)
  - The regex toggle button is visually disconnected from the search input
  - Time range radio buttons have inconsistent visual treatment (Live gets a pulsing dot, others get a clock icon hidden on mobile)
  - Line count "500" is shown as plain text with no context — what does 500 mean?
- **Impact**: Cognitive overload. Users have to learn what each of these controls does through trial and error. The toolbar fights for attention with the actual log content.

#### C5. Color Palette — Functionally Monochrome
- **Location**: `styles.css` dark theme, all components
- **Category**: Color Design
- **Description**: The palette consists of:
  - Background: near-black (`oklch(0.07 0.003 260)`)
  - Surface: slightly-less-black (`oklch(0.09-0.12)`)
  - Text: gray (`oklch(0.92)`) with most text at 40-65% opacity
  - Accent: teal (`oklch(0.78 0.12 180)`) — used for everything: primary buttons, active tabs, focus rings, Live indicator
  - Status: green, amber, red from Tailwind defaults
  - Service colors: 8 hardcoded OKLCH values
  
  The result is that 95% of the interface is dark gray, and the remaining 5% is teal or status colors. There's no warmth, no depth, no visual interest. The service colors (sky blue, violet, amber, etc.) are a good idea but used only for 3px strips and service names — they could do so much more.
- **Impact**: Everything looks the same. There's no visual differentiation between primary and secondary information. Users can't quickly parse the interface at a glance.

#### C6. Empty State — Uninspiring and Unhelpful
- **Location**: `empty-dashboard.tsx`
- **Category**: Onboarding / Empty States
- **Description**: When no stacks are running:
  - Shows a nested-rectangle "stack illustration" that's just three `bg-primary/5` divs — lazy, meaningless decoration
  - "No active stacks" heading + "Start a stack from below, or run `devstack up`" — fine but bland
  - Stack buttons are identical cards with a play icon + stack name + project name
  - No visual differentiation between projects
  - No information about what each stack contains (services, last run time, etc.)
  - The motion animation (whileHover x: 2, whileTap scale: 0.99) is barely perceptible
- **Impact**: First impressions matter. This empty state doesn't teach users anything about devstack or make them excited to use it.

#### C7. Hard-coded Colors Outside Design Tokens
- **Location**: Multiple components
- **Category**: Theming / Consistency
- **Description**: Despite having a design token system, dozens of colors are hard-coded:
  - `text-emerald-400`, `text-emerald-500`, `text-amber-400`, `text-amber-500`, `text-red-400`, `text-red-500`, `text-orange-500` in status-dot.tsx and service-row.tsx
  - `bg-emerald-400`, `bg-amber-400`, `bg-red-400`, `bg-zinc-500`, `bg-zinc-600` in status.ts
  - `bg-red-500/5`, `border-red-500/10` in error states
  - `oklch(0.28 0 0)` for line numbers directly in CSS
  - `oklch(0.42 0 0 / 20%)` for facet bars
  - All status glow colors are raw OKLCH in CSS classes
  
  These bypass the token system entirely, making theme changes or light mode impossible.
- **Impact**: The app is locked into dark mode forever. The light mode tokens in `:root` are defined but would produce a broken UI because half the colors are hard-coded for dark backgrounds.

#### C8. No Light Mode Support
- **Location**: `index.html` (`class="dark"` hardcoded), `styles.css`
- **Category**: Theming / Accessibility
- **Description**: The HTML has `class="dark"` hardcoded. There is no theme toggle. The light mode tokens exist in `:root` but are untested and would break with all the hard-coded dark colors. Many users work in bright environments or prefer light mode for readability.
- **Impact**: Excludes users who prefer or need light mode. The WCAG requirement for user color preferences is not met.

---

### HIGH-Severity Issues

#### H1. Facets Panel Takes 256px of Horizontal Space
- **Location**: `log-viewer.tsx` facets aside (`w-64`)
- **Description**: The facets panel is a fixed 256px column that permanently steals horizontal space from log content. With a 260px service panel + 256px facets panel, only ~44% of a 1920px screen shows actual logs. On a 1440px screen, it's even worse.
- **Impact**: The log viewer — the core feature — is squeezed into a narrow strip. Users must constantly toggle facets on/off to read logs comfortably.
- **Recommendation**: Move facets to a popover/dropdown or collapsible section above the logs. Reclaim horizontal space for the primary content.

#### H2. Service Row Action Buttons — Discoverability Problem
- **Location**: `service-row.tsx`
- **Description**: Copy URL, Open in Browser, and Restart buttons start at 60% opacity and only reach 100% on hover. On a dark interface, 60% opacity of `muted-foreground/50` is nearly invisible. These are core actions — copy URL and open service are used constantly.
- **Impact**: Users don't know these actions exist until they accidentally hover over a service row.
- **Recommendation**: Make action buttons always visible at full opacity for the selected service. Consider a persistent action bar or right-click context menu.

#### H3. Tab Bar Scrolling on Many Services
- **Location**: `log-tab-bar.tsx`
- **Description**: The tab bar uses `overflow-x-auto scrollbar-none` — it scrolls horizontally but hides the scrollbar. With 5+ services, tabs overflow off-screen with no visible indicator.
- **Impact**: Users don't know there are more service tabs. Hidden scrollbars violate scrollable region discoverability.
- **Recommendation**: Add scroll arrows/fade indicators, or use a different pattern (dropdown, wrap) for many services.

#### H4. Run Selector Dropdown — Insufficient Information
- **Location**: `header.tsx` dropdown
- **Description**: The run dropdown shows: StatusDot + stack name + project name + run ID fragment. But it doesn't show:
  - Number of services or their health
  - How long the run has been active
  - Which services are in the stack
  - The run's creation time (only shown for stopped runs as "Xm ago")
- **Impact**: When multiple runs are active, users can't make informed decisions about which to switch to.

#### H5. Keyboard Shortcut Hints Overlap Log Content
- **Location**: `log-viewer.tsx` bottom-left hints
- **Description**: The keyboard hints (`/ search`, `E errors`, etc.) are positioned `absolute bottom-2.5 left-3` with `pointer-events-none`. They sit on top of the last visible log line(s), creating visual clutter.
- **Impact**: These hints are useful for new users but permanently obstruct content for experienced users. There's no way to dismiss them.
- **Recommendation**: Show hints only on first visit or in a discoverable help menu. Or position them in the toolbar area.

#### H6. Excessive Opacity Layering Creates Muddy Contrast
- **Location**: Throughout all components
- **Description**: The codebase is saturated with fractional opacity: `/40`, `/50`, `/60`, `/65`, `/70`. Examples:
  - `text-muted-foreground/40` (timestamps)
  - `text-muted-foreground/45` (URLs)
  - `text-muted-foreground/50` (various labels)
  - `text-muted-foreground/60` (daemon status)
  - `text-foreground/65` (info-level logs)
  - `text-foreground/70` (service names)
  - `text-foreground/80` (headings in error states)
  
  There are at least 12 distinct opacity levels used on text. This creates a muddy, indistinct hierarchy where nothing feels confident or readable.
- **Impact**: Everything looks tentatively visible rather than deliberately placed. The hierarchy is communicated through opacity rather than through type size, weight, color, or spatial position.
- **Recommendation**: Collapse to 3-4 distinct text colors (foreground, secondary, tertiary, disabled) and use size/weight for hierarchy instead of opacity.

#### H7. Zero Border Radius — Hostile Visual Tone
- **Location**: `styles.css` `--radius: 0`
- **Description**: Every single element — buttons, inputs, cards, badges, modals, dropdowns, tabs — has perfectly sharp corners. The radius scale (`radius-sm` through `radius-4xl`) all compute to 0 or negative values. Combined with the dark palette and monospace text, this creates a cold, intimidating aesthetic.
- **Impact**: The interface feels hostile rather than welcoming. Sharp corners increase visual tension and make the interface feel unfinished or intentionally brutalist without the skill to pull off brutalism.
- **Recommendation**: Use a small, consistent radius (4-6px) for interactive elements and containers. Brutalist zero-radius can work but requires exceptional spatial design to compensate.

#### H8. Stop/Kill Stack — Dangerous Actions with Tiny Unlabeled Buttons
- **Location**: `service-panel.tsx` stack controls
- **Description**: The Stop (Square icon) and Kill (XCircle icon) stack actions are `icon-xs` (24px) buttons with no text labels. Kill is styled with `text-destructive` but both are tiny and adjacent. There is no confirmation dialog for Kill.
- **Impact**: Accidentally killing a stack with 5+ services is a real risk. The icons are ambiguous (Square could mean "stop" or "minimize").
- **Recommendation**: Add text labels ("Stop", "Kill"), use a confirmation flow for Kill, and increase button size.

#### H9. Command Palette Missing Restart Actions
- **Location**: `command-palette.tsx`, `dashboard.tsx`
- **Description**: `useDashboardCommands` accepts `onRestartService` but `Dashboard` never passes it. The command palette lists services for filtering but cannot restart them — a core workflow gap.
- **Impact**: Users who discover the command palette expect full functionality but find it incomplete.

#### H10. Search Suggestions Dropdown — Poor Positioning and Interaction
- **Location**: `log-viewer.tsx` suggestions dropdown
- **Description**: The suggestions dropdown opens `top-full mt-0.5` (directly below the search input), uses `onMouseDown preventDefault` to keep focus. But:
  - The dropdown has no maximum width constraint — suggestions with long facet values can overflow
  - No loading state when facets are being fetched
  - `setTimeout(() => setIsSearchFocused(false), 100)` on blur is a race condition hack
  - Keyboard selection wraps awkwardly (ArrowDown stops at end instead of wrapping)
- **Impact**: Autocomplete feels janky and unreliable. The 100ms timeout can cause the dropdown to close before a click registers.

#### H11. Globals Section — Second-Class Treatment
- **Location**: `service-panel.tsx` globals section
- **Description**: Global services (mysql, cache, db) are shown in a separate section with:
  - No status icons (just StatusDot, no CheckCircle2 like regular services)
  - No action buttons (no copy URL, no open, no restart)
  - Different spacing and text styling
  - Port shown as `:3311` with no URL
  - Duplicate "cache" entries shown (from different projects) with no disambiguation
- **Impact**: Globals feel like an afterthought. Users can't interact with them (no copy, open, restart). Duplicate names cause confusion.

#### H12. Mobile Facets — Full-Screen Overlay Blocks Everything
- **Location**: `log-viewer.tsx` facets aside on mobile
- **Description**: On mobile, the facets panel becomes `w-full absolute inset-0 z-20` — a full-screen overlay. This completely obscures the log content, requiring the user to close facets before they can see the effect of their filter changes.
- **Impact**: Users can't see logs while adjusting filters on mobile, defeating the purpose of faceted filtering.

#### H13. Log Detail Panel Expansion — Disrupts Virtual List
- **Location**: `log-row.tsx`, `log-detail.tsx`
- **Description**: When clicking a log line, the detail panel renders *inside* the virtualized row. This causes the virtualizer to re-measure and shift all visible rows. The detail panel uses `animate-in fade-in-0 slide-in-from-top-1 duration-150` which can cause visual jank with the virtualizer's absolute positioning.
- **Impact**: Expanding a log line while auto-scrolling causes content to jump. The expansion animation fights with the virtualizer's layout.

#### H14. Noise Texture — Pure Decoration
- **Location**: `styles.css` `.noise-bg::before`
- **Description**: A noise SVG texture overlay at 2% opacity covers the entire dashboard background. It adds zero functional value and consumes a compositing layer.
- **Impact**: Purely decorative performance cost. At 2% opacity it's barely perceptible — if you can't see it, remove it.

---

### MEDIUM-Severity Issues

#### M1. Inconsistent Font Usage — Mono Where Sans Would Be Better
- **Location**: Search input, facet values, run IDs, URLs, timestamps
- **Description**: The monospace font (`JetBrains Mono`) is used for: search input, facet value buttons, run IDs, URLs, status text, timestamps, line numbers, log content, and project paths. This means ~80% of the text in the UI is monospace. Only headings, button labels, and section headers use the sans font.
- **Recommendation**: Reserve monospace for log content, code snippets, and run IDs. Use the sans font for UI elements (search input, facet labels, button text).

#### M2. Stagger Animation on Service List — Unnecessary
- **Location**: `service-panel.tsx` `stagger-in` class
- **Description**: Every time the service panel renders, all service rows animate in with a stagger (40ms per item, up to 360ms total for 10 items). On a real-time dashboard that re-renders every 2 seconds, this creates unnecessary motion.
- **Recommendation**: Only stagger-animate on initial mount, not on re-renders. The CSS `stagger-in` class replays whenever the DOM is regenerated.

#### M3. Time Range Buttons — Cramped and Unclear
- **Location**: `log-viewer.tsx` time range radio group
- **Description**: "Live", "5m", "15m", "1h" are crammed into tiny buttons. "5m" and "15m" are ambiguous — 5 minutes of what? Last 5 minutes? Since 5 minutes ago? The "Live" label with its pulsing dot implies streaming, but the others imply historical ranges.
- **Recommendation**: Use clearer labels: "Live", "Last 5m", "Last 15m", "Last 1h". Or use a single time picker with presets.

#### M4. Health Bar Visualization — Cryptic
- **Location**: `health-summary.tsx`
- **Description**: The health bars are tiny colored rectangles (1.5px wide × 20px tall) with staggered scaleY animation. On compact mode (mobile), they shrink to 1px × 14px. At that size, they're nearly invisible.
- **Impact**: The visual metaphor (bars = services) isn't immediately clear. "5/5 ready" text does the actual communication; the bars are decoration.

#### M5. Error Boundary — Unstyled and Unhelpful
- **Location**: `error-boundary.tsx`
- **Description**: The crash screen shows a red monospace error with a "Reload" button. No recovery suggestions, no way to report the issue, no context about what went wrong.
- **Recommendation**: Add "Copy error details", "Return to dashboard", and link to docs/issues.

#### M6. JSON Editor — External Theme Dependency
- **Location**: `json-editor.tsx`, `styles.css` overrides
- **Description**: The log detail JSON view uses `vanilla-jsoneditor` with CSS overrides for dark theme. This brings in an entire JSON editor library for read-only viewing. The result looks inconsistent with the rest of the UI.
- **Recommendation**: Use a simple syntax-highlighted `<pre>` for read-only JSON display. `vanilla-jsoneditor` is overkill.

#### M7. Duplicate Cache Globals
- **Location**: `service-panel.tsx` globals rendering
- **Description**: The globals section shows "cache :41793" and "cache :41789" as separate entries from different projects. No project context is shown, making them indistinguishable.
- **Recommendation**: Show project name or key alongside global name for disambiguation.

#### M8. Focus Ring — Insufficient Contrast
- **Location**: `styles.css` `:focus-visible`
- **Description**: Focus ring uses `var(--primary)` which is teal on dark mode. Teal on dark background meets 3:1 but is subtle. The `outline-offset: 2px` creates a gap that can make the ring hard to see against dark backgrounds.
- **Recommendation**: Use a higher-contrast focus ring or double-ring technique (inner dark + outer light).

#### M9. Animation on Route/Run Transitions — Opacity-Only Is Jarring
- **Location**: `dashboard.tsx` AnimatePresence
- **Description**: Run transitions use only `opacity: 0 → 1` over 150ms. The entire content area disappears and reappears, including the sidebar. This creates a visible flash.
- **Recommendation**: Keep the sidebar stable during run transitions. Only animate the log viewer content.

#### M10. Log Line Hover — Too Subtle
- **Location**: `styles.css` `.log-line:hover`
- **Description**: Log line hover is `oklch(1 0 0 / 2.5%)` — a 2.5% white overlay. This is imperceptible on most monitors.
- **Recommendation**: Increase to ~5-8% or use a distinct background color change.

#### M11. Missing `aria-label` on Project Start Buttons
- **Location**: `empty-dashboard.tsx`
- **Description**: Stack start buttons don't have aria-labels. Screen readers would announce "dev request-viewer" which is the button's visible text, but the full context ("Start the 'dev' stack for request-viewer project") is lost.

#### M12. Search Input Placeholder — Low Contrast
- **Location**: `log-viewer.tsx` search input
- **Description**: `placeholder:text-muted-foreground/35` — 35% opacity of the already-muted foreground color. This fails WCAG placeholder contrast requirements (4.5:1 for body text).

#### M13. Service Color Assignment — Not Deterministic Across Runs
- **Location**: `log-viewer.tsx` color index map
- **Description**: Service colors are assigned based on array index (`services.forEach((svc, i) => map.set(svc, i % 8))`). If services are returned in a different order across runs, colors shift. "api" could be blue in one run and violet in another.
- **Recommendation**: Hash service name to a deterministic color index.

#### M14. No Loading State for Run Switching
- **Location**: `dashboard.tsx`
- **Description**: When switching runs via the dropdown, there's no loading indicator. The old run's data stays visible until the new run's status query returns. This can take 1-2 seconds, during which the header shows the new run but the sidebar and logs show the old run's data.

#### M15. Facet Bar Widths — Misleading Proportions
- **Location**: `facet-section.tsx`
- **Description**: Facet bars are proportional to max count, not total. If one service has 19882 lines and another has 8, the first gets 100% width and the second gets a ~0.04% sliver that's invisible. The visual gives no useful information at that scale.

#### M16. Missing Responsive Breakpoints for Medium Screens
- **Location**: Various components
- **Description**: The app has two modes: mobile (<768px with slide-over panel) and desktop. There's no tablet/medium screen adaptation. On a 1024px screen, the 260px sidebar + 256px facets panel leaves only 508px for logs — barely usable.

---

### LOW-Severity Issues

#### L1. Bouncy Spring in Motion Library (Unused but Present)
- **Location**: `lib/motion.ts` — `bouncy: { stiffness: 600, damping: 15, mass: 1 }`
- **Description**: A "bouncy" spring variant is defined but thankfully unused. It should be removed to prevent future use — bounce easing is dated and tacky per design guidelines.

#### L2. Console.error in Error Boundary
- **Location**: `error-boundary.tsx` line 17
- **Description**: `console.error` is deliberately kept for debugging but should use a proper error reporting mechanism in production.

#### L3. `void onCloseMobilePanel` — Suppressed Lint Warning
- **Location**: `service-panel.tsx` line 29
- **Description**: `void onCloseMobilePanel;` is used to suppress an unused variable warning. This is a code smell — the prop should be used or removed.

#### L4. Status Dot Rounded — Inconsistent with Zero-Radius System
- **Location**: `status-dot.tsx`
- **Description**: Status dots use `rounded-full` while everything else in the UI is sharp-cornered. This inconsistency is technically a design token violation.

#### L5. `useIsMobile` Uses Fixed 768px Breakpoint
- **Location**: `lib/use-media-query.ts`
- **Description**: The mobile breakpoint is hardcoded to 768px. This should be a design token or at least a named constant.

#### L6. Facets Query Refetch Interval — 5 Seconds
- **Location**: `lib/api.ts` queries
- **Description**: Facets refetch every 5 seconds even when facets panel is closed. Wasted network requests.

#### L7. `relativeTime` Function — Naive Implementation
- **Location**: `header.tsx`
- **Description**: The relative time function only handles minutes, hours, and days. No "just now" threshold for <30s, no "seconds ago" option.

#### L8. Tab Title Never Updates
- **Location**: `index.html`
- **Description**: The page title is always "devstack". It should reflect the current run/stack for tab differentiation when multiple windows are open.

#### L9. `whileHover: { x: 2 }` — Imperceptible Motion
- **Location**: `empty-dashboard.tsx`
- **Description**: 2px horizontal shift on hover is literally unnoticeable. Either make it meaningful (4-6px) or remove it.

---

## Patterns & Systemic Issues

### 1. Opacity-as-Hierarchy Anti-Pattern
The entire UI communicates hierarchy through opacity (`/35`, `/40`, `/45`, `/50`, `/60`, `/65`, `/70`, `/80`, `/90`). There are at least 12 distinct opacity values used for text alone. This creates a muddy, indistinct visual field where nothing feels confident. **Replace with 3-4 named text color tokens** and use size/weight/spacing for hierarchy.

### 2. Dark Mode Hardcoding
Light mode tokens exist but the app is hardcoded to dark mode. All hard-coded colors (status dots, glow effects, log level tints, line numbers, facet bars) would need to be refactored to support theming.

### 3. Monospace Overuse
~80% of text in the UI uses monospace. This makes the interface feel like a code editor rather than a dashboard. Reserve monospace for actual code/data content.

### 4. Decoration Over Function
Noise texture, header glow, status dot glow, stagger animations — these are decorative effects that add no functional value and contribute to the "AI-generated" aesthetic.

### 5. Information Density Problems
The three-column layout (sidebar + facets + logs) wastes space on smaller screens. The facets panel is a fixed 256px column that should be a popover. The sidebar shows run IDs and project paths that could be in a details view.

---

## Positive Findings

### P1. Excellent Keyboard Navigation
The keyboard shortcut system is well-designed: `/` to search, `E`/`W` for errors/warnings, `F` for facets, number keys for tabs, `⌘K` for command palette. The shortcuts cover all common workflows.

### P2. URL State Synchronization
Every view state (run, service, search, level, stream, since) is reflected in URL params. This makes views shareable and bookmarkable — a genuinely great feature.

### P3. Search Autocomplete with Facet Suggestions
The search input shows facet suggestions as you type, with counts. This is a powerful discovery mechanism that helps users build complex queries.

### P4. Real-Time Updates Without Jank
The polling + virtualizer combination works well for real-time log streaming. Auto-scroll behavior with "new logs" toast is a thoughtful pattern.

### P5. Accessible ARIA Structure
The ARIA landmark structure is solid: `role="log"`, `role="tablist"`, `role="complementary"`, proper `aria-label` on most interactive elements, `aria-pressed` states on toggle buttons.

### P6. Thoughtful Error States
Empty states for no matches, no logs, run stopped, and search errors are all handled with distinct messaging and recovery actions.

### P7. Status System is Well-Modeled
The `status.ts` module provides a single source of truth for state → color mapping. The five states (ready, starting, degraded, failed, stopped) cover all lifecycle phases.

---

## Recommendations by Priority

### Immediate — Before Any Feature Work

1. **Define a real design direction.** Choose: clean professional (like Linear/Stripe) or information-dense operational (like Grafana/Datadog). Commit fully. Kill the teal-on-black-glow-noise aesthetic.

2. **Fix log viewer contrast.** Line numbers, timestamps, and info-level text must meet WCAG AA (4.5:1). This is the #1 usability improvement.

3. **Reduce opacity chaos.** Define 4 text color tokens: `--text-primary`, `--text-secondary`, `--text-tertiary`, `--text-disabled`. Map all existing opacity values to one of these.

4. **Add border radius.** Set `--radius` to `6px` (or similar). The zero-radius aesthetic is not working.

### Short-Term — This Sprint

5. **Redesign the service panel.** Clear hierarchy: service name + status as primary, URL + actions as secondary. Remove run ID from the top — move it to a details section. Make action buttons always visible.

6. **Redesign the toolbar.** Reduce visual weight. Combine search + filters into a more cohesive bar. Move facets to a popover instead of a fixed column.

7. **Add Stop/Kill confirmation.** At minimum, Kill should require confirmation.

8. **Fix service color determinism.** Hash service names to consistent colors.

9. **Remove decorative effects.** Kill noise texture, header glow, status glow box-shadows. Replace with functional visual treatments.

### Medium-Term — Next Sprint

10. **Support light mode.** Refactor all hard-coded colors to use design tokens. Add a theme toggle.

11. **Redesign the empty state.** Show useful information: project health, last run time, service count per stack.

12. **Improve facets panel.** Make it a collapsible popover or accordion, not a fixed column.

13. **Add run switching indicator.** Show loading state during run transitions.

14. **Redesign the health bar.** Replace with text-only status summary or a more readable visualization.

### Long-Term — Future

15. **Build a proper design system.** Extract tokens for spacing, color, typography, and animation into a documented system.

16. **Add responsive adaptation for tablets.** The current mobile/desktop split misses medium screens.

17. **Improve the command palette.** Add restart actions, recent searches, and run switching commands.

18. **Add user preferences.** Theme, default facets state, log density, auto-scroll preference.

---

## Summary

The devstack dashboard is **functionally complete and well-engineered** — the data model, real-time updates, keyboard shortcuts, URL state, and accessibility markup are all solid. But the **visual design is generic AI-generated dark-mode terminal slop** that actively undermines the UX.

The path forward is not incremental polish — it's a design reset. Define a clear visual direction, establish a real color and typography system, and redesign the core surfaces (service panel, toolbar, log viewer) with information hierarchy as the primary goal. The engineering is good; the design needs to catch up.
