import { useEffect, useRef, useState, useMemo, useCallback } from "react";
import { useQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { toast } from "sonner";
import {
  ArrowDown,
  Search,
  X,
  AlertTriangle,
  ChevronUp,
  ChevronDown,
  Regex,
  Clock,
  ListFilter,
  Share2,
} from "lucide-react";
import { cn } from "@/lib/utils";
import {
  ApiError,
  api,
  queries,
  type FacetFilter,
  type LogFilterParams,
  type RunStatusResponse,
} from "@/lib/api";
import { patchUrlParams, readUrlParam } from "@/lib/url-state";
import {
  LogRow,
  FacetSection,
  LogTabBar,
  LogScrollControls,
  LogSkeleton,
  type ParsedLog,
  type TimeRange,
} from "./log-viewer/index";

interface LogViewerProps {
  runId: string;
  projectDir: string;
  services: string[];
  selectedService: string | null;
  onSelectService: (name: string | null) => void;
  status?: RunStatusResponse;
  isMobile?: boolean;
}

// eslint-disable-next-line no-control-regex
const ANSI_RE =
  /\x1b(?:\[[0-9;?]*[A-Za-z]|\][^\x07\x1b]*(?:\x07|\x1b\\)|\([A-B]|[=>NOMDEHcZ78])/g;

function stripAnsi(text: string): string {
  return text.indexOf("\x1b") === -1 ? text : text.replace(ANSI_RE, "");
}

function tryParseJson(text: string): Record<string, unknown> | undefined {
  const ch = text.charCodeAt(0);
  if (ch !== 123 && ch !== 91) return undefined;
  try {
    const parsed = JSON.parse(text);
    if (typeof parsed === "object" && parsed !== null) return parsed;
  } catch {
    /* not json */
  }
  return undefined;
}

function formatTimestamp(ts: string): string {
  if (ts.length >= 23 && ts.charCodeAt(10) === 84) {
    return ts.slice(11, 23);
  }
  try {
    const d = new Date(ts);
    const h = d.getHours(),
      m = d.getMinutes(),
      s = d.getSeconds(),
      ms = d.getMilliseconds();
    return `${h < 10 ? "0" : ""}${h}:${m < 10 ? "0" : ""}${m}:${s < 10 ? "0" : ""}${s}.${ms < 10 ? "00" : ms < 100 ? "0" : ""}${ms}`;
  } catch {
    return ts.slice(11, 23);
  }
}

function timeRangeToSince(range: TimeRange, customSince?: string): string | undefined {
  if (range === "live") return undefined;
  if (range === "custom") return customSince;
  const ms = { "5m": 5 * 60_000, "15m": 15 * 60_000, "1h": 60 * 60_000 }[range];
  return new Date(Date.now() - ms).toISOString();
}

function parseSinceParam(value: string | null): { range: TimeRange; customSince?: string } {
  if (value === "5m" || value === "15m" || value === "1h") {
    return { range: value };
  }
  if (value && value.trim().length > 0) {
    return { range: "custom", customSince: value };
  }
  return { range: "live" };
}

function parseLastParam(value: string | null, fallback: number): number {
  if (!value) return fallback;
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) return fallback;
  return parsed;
}

function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function escapeTantivyPhrase(s: string): string {
  return `"${s.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

function simpleTantivyQuery(input: string, facetFields: Set<string>): string {
  const terms = input.trim().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return "";
  return terms
    .map((t) => {
      const neg = t.startsWith("-");
      const raw = neg ? t.slice(1) : t;
      const colon = raw.indexOf(":");
      if (colon > 0) {
        const field = raw.slice(0, colon).toLowerCase();
        const rest = raw.slice(colon + 1);
        if (facetFields.has(field) && !rest.startsWith("//")) {
          return (neg ? "-" : "") + raw;
        }
        return escapeTantivyPhrase(t);
      }
      return /^[A-Za-z0-9_]+$/.test(t) ? t : escapeTantivyPhrase(t);
    })
    .join(" AND ");
}

function facetToken(field: string, value: string): string {
  if (/^[A-Za-z0-9_.-]+$/.test(value)) return `${field}:${value}`;
  return `${field}:${escapeTantivyPhrase(value)}`;
}

type SuggestionKind = "facet" | "facetValue";
type SearchSuggestion = {
  id: string;
  kind: SuggestionKind;
  label: string;
  description?: string;
  insertText: string;
};

function tokenAtCursor(
  text: string,
  cursor: number,
): { start: number; end: number; token: string } {
  const isWs = (c: string) => c === " " || c === "\n" || c === "\t";
  let start = cursor;
  while (start > 0 && !isWs(text[start - 1])) start--;
  let end = cursor;
  while (end < text.length && !isWs(text[end])) end++;
  return { start, end, token: text.slice(start, end) };
}

export function LogViewer({
  runId,
  projectDir,
  services,
  selectedService,
  onSelectService,
  status,
  isMobile,
}: LogViewerProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);
  const [isAtBottom, setIsAtBottom] = useState(true);
  const defaultLast = 500;
  const [searchInput, setSearchInput] = useState(() => readUrlParam("search") ?? "");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [isAdvancedQuery, setIsAdvancedQuery] = useState(false);
  const [isSearchFocused, setIsSearchFocused] = useState(false);
  const [suggestionIndex, setSuggestionIndex] = useState(0);
  const [facetsOpen, setFacetsOpen] = useState(() => {
    // Default facets closed below 1024px — sidebar + facets + logs doesn't fit
    if (typeof window !== "undefined" && window.innerWidth < 1024) return false;
    return true;
  });
  const [levelFilter, setLevelFilter] = useState(() => readUrlParam("level") ?? "all");
  const parsedSince = useMemo(() => parseSinceParam(readUrlParam("since")), []);
  const [timeRange, setTimeRange] = useState<TimeRange>(parsedSince.range);
  const [customSince] = useState<string | undefined>(parsedSince.customSince);
  const [streamFilter, setStreamFilter] = useState(() => readUrlParam("stream") ?? "all");
  const last = useMemo(() => parseLastParam(readUrlParam("last"), defaultLast), []);
  const [expandedRow, setExpandedRow] = useState<number | null>(null);
  const [activeMatchIndex, setActiveMatchIndex] = useState(0);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const [newLogCount, setNewLogCount] = useState(0);
  const prevLogCountRef = useRef(0);

  // Debounce search input → server query
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedSearch(searchInput), 150);
    return () => clearTimeout(timer);
  }, [searchInput]);

  const selectedServiceIsValid =
    selectedService === null || services.includes(selectedService);
  const activeTab =
    selectedService !== null && selectedServiceIsValid ? selectedService : "__all__";

  useEffect(() => {
    if (selectedService !== null && !selectedServiceIsValid) {
      onSelectService(null);
    }
  }, [selectedService, selectedServiceIsValid, onSelectService]);

  useEffect(() => {
    patchUrlParams({
      search: searchInput || undefined,
      level: levelFilter !== "all" ? levelFilter : undefined,
      stream: streamFilter !== "all" ? streamFilter : undefined,
      since:
        timeRange === "custom"
          ? customSince
          : timeRange !== "live"
            ? timeRange
            : undefined,
      last: last !== defaultLast ? last : undefined,
    });
  }, [searchInput, levelFilter, streamFilter, timeRange, customSince, last, defaultLast]);

  const facetFilters: Omit<LogFilterParams, "last" | "search"> = useMemo(() => {
    const p: Omit<LogFilterParams, "last" | "search"> = {};
    const since = timeRangeToSince(timeRange, customSince);
    if (since) p.since = since;
    if (activeTab !== "__all__") p.service = activeTab;
    if (levelFilter !== "all") p.level = levelFilter;
    if (streamFilter !== "all") p.stream = streamFilter;
    return p;
  }, [timeRange, customSince, activeTab, levelFilter, streamFilter]);

  const facetsQuery = useQuery({
    ...queries.runLogsFacets(runId, facetFilters),
    enabled: Boolean(runId),
    refetchInterval: (query) =>
      query.state.error instanceof ApiError && query.state.error.status === 404
        ? false
        : 5000,
  });

  const facetFieldSet = useMemo(() => {
    const fields = facetsQuery.data?.filters.map((filter) => filter.field) ?? [];
    return new Set(fields);
  }, [facetsQuery.data]);

  const serverQuery = useMemo(() => {
    if (!debouncedSearch) return undefined;
    return isAdvancedQuery
      ? debouncedSearch
      : simpleTantivyQuery(debouncedSearch, facetFieldSet);
  }, [debouncedSearch, isAdvancedQuery, facetFieldSet]);

  const filterParams: LogFilterParams = useMemo(() => {
    const p: LogFilterParams = { last };
    if (serverQuery) p.search = serverQuery;
    if (levelFilter !== "all") p.level = levelFilter;
    if (streamFilter !== "all") p.stream = streamFilter;
    const since = timeRangeToSince(timeRange, customSince);
    if (since) p.since = since;
    if (activeTab !== "__all__") p.service = activeTab;
    return p;
  }, [last, serverQuery, levelFilter, timeRange, customSince, streamFilter, activeTab]);

  const logsQuery = useQuery({
    ...queries.runLogsSearch(runId, filterParams),
    enabled: Boolean(runId),
    refetchInterval: (query) =>
      query.state.error instanceof ApiError && query.state.error.status === 404
        ? false
        : 1500,
  });

  const latestAgentSessionQuery = useQuery({
    ...queries.latestAgentSession(projectDir),
    enabled: Boolean(projectDir),
  });

  const shareCommand = useMemo(() => {
    const args = ["devstack", "show", "--run", runId];
    if (activeTab !== "__all__") {
      args.push("--service", activeTab);
    }
    if (searchInput.trim()) {
      args.push("--search", searchInput.trim());
    }
    if (levelFilter !== "all") {
      args.push("--level", levelFilter);
    }
    if (streamFilter !== "all") {
      args.push("--stream", streamFilter);
    }
    if (timeRange === "custom") {
      if (customSince?.trim()) {
        args.push("--since", customSince.trim());
      }
    } else if (timeRange !== "live") {
      args.push("--since", timeRange);
    }
    if (last !== defaultLast) {
      args.push("--last", String(last));
    }
    return args
      .map((arg) => (arg.includes(" ") ? JSON.stringify(arg) : arg))
      .join(" ");
  }, [activeTab, customSince, defaultLast, last, levelFilter, runId, searchInput, streamFilter, timeRange]);

  const canShare = Boolean(latestAgentSessionQuery.data?.session);

  const shareCurrentView = useCallback(async () => {
    if (!canShare) return;
    try {
      await api.shareToAgent(projectDir, shareCommand, "Can you take a look at this?");
      toast.success("Shared log query with active agent");
    } catch (error) {
      const message = error instanceof Error ? error.message : "Unknown error";
      toast.error(`Failed to share query: ${message}`);
    }
  }, [canShare, projectDir, shareCommand]);

  // Service → color index mapping
  const colorIndexMap = useMemo(() => {
    const map = new Map<string, number>();
    services.forEach((svc, i) => map.set(svc, i % 8));
    return map;
  }, [services]);

  const { logs, matchCount, truncated, matchedTotal } = useMemo(() => {
    const entries = logsQuery.data?.entries ?? [];
    const result: ParsedLog[] = entries.map((e) => ({
      timestamp: formatTimestamp(e.ts),
      rawTimestamp: e.ts,
      content: stripAnsi(e.message),
      service: e.service,
      stream: e.stream,
      level: (e.level as ParsedLog["level"]) || "info",
      raw: stripAnsi(e.raw),
      json: tryParseJson(e.message),
    }));
    return {
      logs: result,
      matchCount: debouncedSearch ? result.length : 0,
      truncated: logsQuery.data?.truncated ?? false,
      matchedTotal: logsQuery.data?.matched_total ?? 0,
    };
  }, [logsQuery.data, debouncedSearch]);

  // Track new logs arriving when not at bottom (14.18)
  useEffect(() => {
    if (autoScroll || isAtBottom) {
      setNewLogCount(0);
      prevLogCountRef.current = logs.length;
    } else if (logs.length > prevLogCountRef.current) {
      setNewLogCount(logs.length - prevLogCountRef.current);
    }
  }, [logs.length, autoScroll, isAtBottom]);

  const suggestions = useMemo<SearchSuggestion[]>(() => {
    if (!isSearchFocused) return [];

    const el = searchInputRef.current;
    const cursor = el?.selectionStart ?? searchInput.length;
    const { token } = tokenAtCursor(searchInput, cursor);

    const neg = token.startsWith("-");
    const raw = neg ? token.slice(1) : token;
    const lower = raw.toLowerCase();

    const out: SearchSuggestion[] = [];

    const add = (s: Omit<SearchSuggestion, "id">) => {
      out.push({ id: `${s.kind}:${s.label}:${s.insertText}`, ...s });
    };

    const filters = facetsQuery.data?.filters ?? [];
    const filterByField = new Map(filters.map((filter) => [filter.field, filter]));

    const colon = raw.indexOf(":");
    if (colon >= 0) {
      const field = raw.slice(0, colon).toLowerCase();
      const valuePrefix = raw.slice(colon + 1);
      const filter = filterByField.get(field);
      if (filter) {
        const prefixLower = valuePrefix.toLowerCase();
        const filtered = filter.values.filter((v) =>
          v.value.toLowerCase().startsWith(prefixLower),
        );
        for (const v of filtered) {
          add({
            kind: "facetValue",
            label: `${neg ? "-" : ""}${field}:${v.value}`,
            description: `Seen ${v.count}x`,
            insertText: `${neg ? "-" : ""}${field}:${v.value} `,
          });
        }
        return out.slice(0, 12);
      }
    }

    const facetKeys = filters.map((filter) => filter.field);
    const facetMatches = facetKeys.filter((field) => field.startsWith(lower));
    for (const field of facetMatches) {
      add({
        kind: "facet",
        label: `${neg ? "-" : ""}${field}:`,
        description: "Facet filter",
        insertText: `${neg ? "-" : ""}${field}:`,
      });
    }
    if (token.length === 0) {
      for (const field of facetKeys) {
        add({
          kind: "facet",
          label: `${field}:`,
          description: "Facet filter",
          insertText: `${field}:`,
        });
      }
    }

    return out.slice(0, 8);
  }, [isSearchFocused, searchInput, facetsQuery.data]);

  useEffect(() => {
    if (suggestionIndex >= suggestions.length) setSuggestionIndex(0);
  }, [suggestions, suggestionIndex]);

  // Virtualizer
  const virtualizer = useVirtualizer({
    count: logs.length,
    getScrollElement: () => containerRef.current,
    estimateSize: () => 28,
    overscan: 30,
  });

  // Clamp active match
  useEffect(() => {
    if (activeMatchIndex >= matchCount && matchCount > 0) {
      setActiveMatchIndex(matchCount - 1);
    } else if (matchCount === 0) {
      setActiveMatchIndex(0);
    }
  }, [matchCount, activeMatchIndex]);

  // Scroll to active match
  useEffect(() => {
    if (matchCount === 0 || !debouncedSearch) return;
    virtualizer.scrollToIndex(activeMatchIndex, { align: "center" });
    setAutoScroll(false);
  }, [activeMatchIndex, matchCount, debouncedSearch, virtualizer]);

  useEffect(() => {
    if (autoScroll && !debouncedSearch && logs.length > 0) {
      virtualizer.scrollToIndex(logs.length - 1, { align: "end" });
    }
  }, [logs, autoScroll, debouncedSearch, virtualizer]);

  const handleScroll = useCallback(() => {
    if (!containerRef.current) return;
    const { scrollTop, scrollHeight, clientHeight } = containerRef.current;
    const atBottom = scrollHeight - scrollTop - clientHeight < 50;
    setIsAtBottom(atBottom);
    if (atBottom && !autoScroll) setAutoScroll(true);
    else if (!atBottom && autoScroll) setAutoScroll(false);
  }, [autoScroll]);

  const scrollToBottom = useCallback(() => {
    if (logs.length > 0) {
      virtualizer.scrollToIndex(logs.length - 1, { align: "end" });
    }
    setAutoScroll(true);
    setNewLogCount(0);
    prevLogCountRef.current = logs.length;
  }, [logs.length, virtualizer]);

  const nextMatch = useCallback(() => {
    if (matchCount === 0) return;
    setActiveMatchIndex((prev) => (prev + 1) % matchCount);
  }, [matchCount]);

  const prevMatch = useCallback(() => {
    if (matchCount === 0) return;
    setActiveMatchIndex((prev) => (prev - 1 + matchCount) % matchCount);
  }, [matchCount]);

  const toggleExpand = useCallback((index: number) => {
    setExpandedRow((prev) => (prev === index ? null : index));
  }, []);

  // Keyboard shortcuts (8.1)
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const isInput =
        document.activeElement?.tagName === "INPUT" ||
        document.activeElement?.tagName === "TEXTAREA";

      if ((e.ctrlKey || e.metaKey) && e.key === "f") {
        e.preventDefault();
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
      }
      if (e.key === "Escape") {
        if (searchInput) {
          setSearchInput("");
          setExpandedRow(null);
        }
        searchInputRef.current?.blur();
      }
      if (debouncedSearch && matchCount > 0) {
        if (e.key === "Enter" || ((e.ctrlKey || e.metaKey) && e.key === "g")) {
          if (document.activeElement === searchInputRef.current || !isInput) {
            e.preventDefault();
            if (e.shiftKey) prevMatch();
            else nextMatch();
          }
        }
      }
      if (
        e.key === "/" &&
        !e.ctrlKey &&
        !e.metaKey &&
        !isInput
      ) {
        e.preventDefault();
        searchInputRef.current?.focus();
      }
      // Quick filter shortcuts (when no input focused)
      if (!isInput && !e.ctrlKey && !e.metaKey) {
        if (e.key === "e" || e.key === "E") {
          e.preventDefault();
          setLevelFilter((c) => (c === "error" ? "all" : "error"));
        }
        if (e.key === "w" || e.key === "W") {
          e.preventDefault();
          setLevelFilter((c) => (c === "warn" ? "all" : "warn"));
        }
        if (e.key === "f" && !e.shiftKey) {
          e.preventDefault();
          setFacetsOpen((v) => !v);
        }
        // Number keys to switch tabs
        if (e.key >= "1" && e.key <= "9") {
          const idx = Number(e.key) - 1;
          if (idx === 0) {
            onSelectService(null);
          } else if (idx - 1 < services.length) {
            onSelectService(services[idx - 1]);
          }
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [searchInput, debouncedSearch, matchCount, nextMatch, prevMatch, onSelectService, services]);

  const showServiceColumn = activeTab === "__all__" && services.length > 1;

  const highlighter = useMemo(() => {
    if (!debouncedSearch) return null;
    if (isAdvancedQuery) return null;
    const terms = debouncedSearch.trim().split(/\s+/).filter(Boolean);
    const highlightTerms = terms.filter((t) => !t.includes(":") && !t.startsWith("-"));
    if (highlightTerms.length === 0) return null;
    if (highlightTerms.length <= 1) return highlightTerms[0];
    const pattern = highlightTerms.map(escapeRegex).join("|");
    try {
      return new RegExp(pattern);
    } catch {
      return highlightTerms[0];
    }
  }, [debouncedSearch, isAdvancedQuery]);

  const applySuggestion = useCallback(
    (s: SearchSuggestion) => {
      const el = searchInputRef.current;
      const cursor = el?.selectionStart ?? searchInput.length;
      const { start, end } = tokenAtCursor(searchInput, cursor);
      const next = `${searchInput.slice(0, start)}${s.insertText}${searchInput.slice(end)}`;
      setSearchInput(next);
      setActiveMatchIndex(0);
      setSuggestionIndex(0);
      requestAnimationFrame(() => {
        const pos = start + s.insertText.length;
        el?.focus();
        el?.setSelectionRange(pos, pos);
      });
    },
    [searchInput],
  );

  const tokenParts = useMemo(
    () => searchInput.trim().split(/\s+/).filter(Boolean),
    [searchInput],
  );

  const isFacetValueActive = useCallback(
    (field: string, value: string) => {
      if (field === "service") return activeTab === value;
      if (field === "level") return levelFilter === value;
      if (field === "stream") return streamFilter === value;
      return tokenParts.includes(facetToken(field, value));
    },
    [activeTab, levelFilter, streamFilter, tokenParts],
  );

  const toggleFacet = useCallback(
    (field: string, value: string) => {
      if (field === "service") {
        if (activeTab === value) {
          onSelectService(null);
        } else {
          onSelectService(value);
        }
        return;
      }

      if (field === "level") {
        setLevelFilter((current) => (current === value ? "all" : value));
        return;
      }

      if (field === "stream") {
        setStreamFilter((current) => (current === value ? "all" : value));
        return;
      }

      const token = facetToken(field, value);
      const parts = searchInput.trim().split(/\s+/).filter(Boolean);
      const idx = parts.indexOf(token);
      if (idx >= 0) {
        parts.splice(idx, 1);
        const next = parts.join(" ");
        setSearchInput(next);
        setActiveMatchIndex(0);
        requestAnimationFrame(() => searchInputRef.current?.focus());
        return;
      }

      applySuggestion({
        id: `facet:${token}`,
        kind: "facetValue",
        label: token,
        insertText: `${token} `,
      });
    },
    [activeTab, applySuggestion, onSelectService, searchInput],
  );

  // Track whether we've ever received data (to distinguish initial load from filter changes)
  const hasEverLoadedRef = useRef(false);
  if (logsQuery.data) hasEverLoadedRef.current = true;
  const isInitialLoad = logsQuery.isLoading && !logsQuery.data && !hasEverLoadedRef.current;

  return (
    <div className="flex-1 flex flex-col min-h-0 relative min-w-0">
      {/* Toolbar */}
      <div className="flex flex-col border-b border-border shrink-0 bg-card/50 min-w-0 overflow-hidden">
        {/* Search bar — stacks on mobile */}
        <div className="flex flex-col md:flex-row md:items-center gap-1.5 px-2 md:px-3 py-1.5 md:py-2 border-b border-border/50 min-w-0 w-full overflow-hidden">
          <div className="flex items-center gap-1.5 flex-1 min-w-0 w-full overflow-hidden">
            <div
              className={cn(
                "relative flex items-center gap-2 flex-1 bg-background/50 border px-2 md:px-3 h-10 md:h-9 transition-colors",
                "border-border/60 focus-within:border-primary/30 focus-within:bg-background",
              )}
            >
              <Search className="w-3.5 h-3.5 text-muted-foreground/45 shrink-0" />
              <input
                ref={searchInputRef}
                type="text"
                value={searchInput}
                onChange={(e) => {
                  setSearchInput(e.target.value);
                  setActiveMatchIndex(0);
                }}
                onFocus={() => setIsSearchFocused(true)}
                onBlur={() => {
                  setTimeout(() => setIsSearchFocused(false), 100);
                }}
                onKeyDown={(e) => {
                  if (!isSearchFocused || suggestions.length === 0) return;
                  if (e.key === "ArrowDown") {
                    e.preventDefault();
                    e.stopPropagation();
                    setSuggestionIndex((i) => Math.min(i + 1, suggestions.length - 1));
                  } else if (e.key === "ArrowUp") {
                    e.preventDefault();
                    e.stopPropagation();
                    setSuggestionIndex((i) => Math.max(i - 1, 0));
                  } else if (e.key === "Enter" || e.key === "Tab") {
                    e.preventDefault();
                    e.stopPropagation();
                    const s = suggestions[suggestionIndex];
                    if (s) applySuggestion(s);
                  } else if (e.key === "Escape") {
                    e.stopPropagation();
                    setIsSearchFocused(false);
                  }
                }}
                placeholder={
                  isMobile
                    ? "Search logs…"
                    : isAdvancedQuery
                      ? "Search logs…  (advanced query)"
                      : "Search logs…  / to focus"
                }
                className="bg-transparent text-sm md:text-xs text-foreground placeholder:text-muted-foreground/35 outline-none flex-1 min-w-0 font-mono"
                aria-label="Search log lines"
                spellCheck={false}
              />
              {searchInput && (
                <>
                  <div className="flex items-center gap-0.5 shrink-0">
                    <span className="text-[11px] text-muted-foreground/40 tabular-nums min-w-[7ch] text-right mr-0.5">
                      {matchCount > 0
                        ? `${activeMatchIndex + 1}/${matchCount}`
                        : "0"}
                      {truncated && matchedTotal > matchCount
                        ? ` of ${matchedTotal}`
                        : ""}
                    </span>
                    <button
                      onClick={prevMatch}
                      disabled={matchCount === 0}
                      className="w-8 h-8 md:w-6 md:h-6 flex items-center justify-center text-muted-foreground/40 hover:text-foreground disabled:opacity-20 transition-colors"
                      aria-label="Previous match"
                    >
                      <ChevronUp className="w-3.5 h-3.5 md:w-3 md:h-3" />
                    </button>
                    <button
                      onClick={nextMatch}
                      disabled={matchCount === 0}
                      className="w-8 h-8 md:w-6 md:h-6 flex items-center justify-center text-muted-foreground/40 hover:text-foreground disabled:opacity-20 transition-colors"
                      aria-label="Next match"
                    >
                      <ChevronDown className="w-3.5 h-3.5 md:w-3 md:h-3" />
                    </button>
                  </div>
                  <button
                    onClick={() => {
                      setSearchInput("");
                      setActiveMatchIndex(0);
                    }}
                    className="w-8 h-8 md:w-6 md:h-6 flex items-center justify-center text-muted-foreground/35 hover:text-foreground transition-colors shrink-0"
                    aria-label="Clear search"
                  >
                    <X className="w-3.5 h-3.5 md:w-3 md:h-3" />
                  </button>
                </>
              )}
              {isSearchFocused && suggestions.length > 0 && (
                <div
                  className={cn(
                    "absolute left-0 right-0 top-full mt-0.5 bg-popover border border-border/60 shadow-xl z-50",
                    "max-h-56 overflow-auto",
                  )}
                  onMouseDown={(e) => e.preventDefault()}
                  role="listbox"
                  aria-label="Search suggestions"
                >
                  {suggestions.map((s, i) => (
                    <button
                      key={s.id}
                      type="button"
                      onClick={() => applySuggestion(s)}
                      className={cn(
                        "w-full text-left px-3 py-2 md:py-1.5 flex items-center justify-between gap-4",
                        "text-xs font-mono transition-colors",
                        i === suggestionIndex
                          ? "bg-secondary/70 text-foreground"
                          : "hover:bg-secondary/40 text-foreground/70",
                      )}
                      role="option"
                      aria-selected={i === suggestionIndex}
                    >
                      <span className="truncate">{s.label}</span>
                      {s.description && (
                        <span className="text-[11px] text-muted-foreground/40 truncate">
                          {s.description}
                        </span>
                      )}
                    </button>
                  ))}
                  {!isMobile && (
                    <div className="px-3 py-1.5 text-[10px] text-muted-foreground/35 border-t border-border/30">
                      ↑/↓ to navigate · Enter to select
                    </div>
                  )}
                </div>
              )}
            </div>

            <button
              onClick={() => setIsAdvancedQuery(!isAdvancedQuery)}
              className={cn(
                "w-10 h-10 md:w-9 md:h-9 flex items-center justify-center transition-colors shrink-0",
                isAdvancedQuery
                  ? "bg-primary/10 text-primary border border-primary/20"
                  : "text-muted-foreground/35 hover:text-foreground hover:bg-secondary/50 border border-transparent",
              )}
              aria-pressed={isAdvancedQuery}
              title="Toggle advanced query"
            >
              <Regex className="w-3.5 h-3.5" />
            </button>
          </div>

          <div
            className="flex items-center border border-border overflow-hidden shrink-0 md:shrink-0 w-full md:w-auto"
            role="radiogroup"
            aria-label="Time range"
          >
            {(
              [
                { key: "live" as const, label: "Live" },
                { key: "5m" as const, label: "5m" },
                { key: "15m" as const, label: "15m" },
                { key: "1h" as const, label: "1h" },
              ] as const
            ).map(({ key, label }) => (
              <button
                key={key}
                role="radio"
                aria-checked={timeRange === key}
                onClick={() => setTimeRange(key)}
                className={cn(
                  "px-1.5 md:px-2.5 h-10 md:h-9 text-xs font-medium transition-colors flex items-center gap-1 flex-1 md:flex-initial justify-center md:justify-start",
                  timeRange === key
                    ? key === "live"
                      ? "bg-primary/10 text-primary"
                      : "bg-secondary text-foreground"
                    : "text-muted-foreground/35 hover:text-foreground hover:bg-secondary/50",
                )}
              >
                {key === "live" && (
                  <span
                    className={cn(
                      "w-1.5 h-1.5 rounded-full",
                      timeRange === "live"
                        ? "bg-emerald-400 pulse-dot"
                        : "bg-muted-foreground/35",
                    )}
                  />
                )}
                {key !== "live" && <Clock className="w-3 h-3 hidden md:block" />}
                {label}
              </button>
            ))}
          </div>
        </div>

        {/* Tab bar + filters */}
        <LogTabBar
          services={services}
          activeTab={activeTab}
          status={status}
          onSelectService={onSelectService}
        >
          <div className="flex items-center gap-1 md:gap-1.5 shrink-0">
            {canShare && (
              <button
                onClick={() => {
                  void shareCurrentView();
                }}
                className="h-10 md:h-8 px-2 md:px-2.5 flex items-center gap-1.5 border border-border text-muted-foreground/60 hover:text-foreground hover:bg-secondary/50 transition-colors"
                aria-label="Share query with agent"
                title="Share this log query with the active agent"
              >
                <Share2 className="w-3.5 h-3.5" />
                <span className="text-xs hidden md:inline">Share</span>
              </button>
            )}

            <span
              className="text-xs text-muted-foreground/50 tabular-nums px-1 hidden md:inline"
              aria-label={`${logs.length} lines`}
            >
              {logs.length}
            </span>

            <button
              onClick={() => setFacetsOpen((v) => !v)}
              className={cn(
                "h-10 md:h-8 px-2 md:px-2.5 flex items-center gap-1.5 border transition-colors",
                facetsOpen
                  ? "bg-secondary text-foreground border-border"
                  : "text-muted-foreground/50 hover:text-foreground hover:bg-secondary/50 border-transparent",
              )}
              aria-pressed={facetsOpen}
              title={facetsOpen ? "Hide facets (F)" : "Show facets (F)"}
            >
              <ListFilter className="w-3.5 h-3.5" />
            </button>

            <button
              onClick={scrollToBottom}
              className={cn(
                "w-10 h-10 md:w-8 md:h-8 flex items-center justify-center transition-colors",
                autoScroll
                  ? "text-primary"
                  : "text-muted-foreground/40 hover:text-foreground hover:bg-secondary/50",
              )}
              aria-label={autoScroll ? "Auto-scroll active" : "Scroll to latest"}
            >
              <ArrowDown className="w-4 h-4" />
            </button>
          </div>
        </LogTabBar>
      </div>

      {/* Log content */}
      <div className="flex-1 min-h-0 flex">
        {facetsOpen && (
          <aside
            className={cn(
              "border-r border-border/30 bg-card/50 shrink-0 overflow-auto",
              isMobile ? "w-full absolute inset-0 z-20 border-r-0" : "w-64",
            )}
            role="complementary"
            aria-label="Log facets"
          >
            <div className="px-3 py-2.5 border-b border-border/20">
              <div className="flex items-center justify-between">
                <span className="text-[11px] font-semibold tracking-wider uppercase text-muted-foreground/50">
                  Facets
                </span>
                <div className="flex items-center gap-2">
                  <span className="text-[11px] text-muted-foreground/45 tabular-nums">
                    {facetsQuery.data
                      ? facetsQuery.data.total
                      : facetsQuery.isLoading
                        ? "…"
                        : 0}
                  </span>
                  {isMobile && (
                    <button
                      onClick={() => setFacetsOpen(false)}
                      className="w-8 h-8 flex items-center justify-center text-muted-foreground/50 hover:text-foreground transition-colors"
                      aria-label="Close facets"
                    >
                      <X className="w-4 h-4" />
                    </button>
                  )}
                </div>
              </div>
              {facetsQuery.isError && (
                <div className="mt-1.5 text-[11px] text-red-400/50">
                  Facets unavailable
                </div>
              )}
            </div>

            {(facetsQuery.data?.filters ?? []).map((filter: FacetFilter) => (
              <FacetSection
                key={filter.field}
                filter={filter}
                loading={facetsQuery.isLoading && !facetsQuery.data}
                onPick={(value: string) => toggleFacet(filter.field, value)}
                isActive={(value: string) => isFacetValueActive(filter.field, value)}
              />
            ))}
          </aside>
        )}

        <div
          ref={containerRef}
          onScroll={handleScroll}
          className="flex-1 overflow-auto font-mono text-[13px] leading-relaxed min-w-0"
          role="log"
          aria-label="Service logs"
          aria-live="polite"
        >
          {/* Loading skeleton (13.1) */}
          {isInitialLoad ? (
            <LogSkeleton />
          ) : logsQuery.isError &&
            logsQuery.error instanceof ApiError &&
            logsQuery.error.status === 404 ? (
            <div className="flex flex-col items-center justify-center h-full text-muted-foreground gap-3 px-8">
              <div className="w-12 h-12 bg-zinc-500/5 border border-zinc-500/10 flex items-center justify-center mb-1">
                <AlertTriangle className="w-5 h-5 text-muted-foreground/30" />
              </div>
              <span className="text-sm text-foreground/60">Run stopped</span>
              <p className="text-xs text-muted-foreground/50">
                Logs are no longer available for this run.
              </p>
            </div>
          ) : logsQuery.isError ? (
            <div className="flex flex-col items-center justify-center h-full text-muted-foreground gap-3 px-8">
              <div className="w-12 h-12 bg-red-500/5 border border-red-500/10 flex items-center justify-center mb-1">
                <AlertTriangle className="w-5 h-5 text-red-400/40" />
              </div>
              <span className="text-sm text-foreground/60">Log search failed</span>
              <pre className="max-w-[600px] w-full whitespace-pre-wrap break-words text-xs text-muted-foreground/50 bg-background/40 border border-border/50 p-3 font-mono">
                {logsQuery.error instanceof Error
                  ? logsQuery.error.message
                  : "Unknown error"}
              </pre>
              <div className="flex items-center gap-3">
                <button
                  onClick={() => {
                    setSearchInput("");
                    setActiveMatchIndex(0);
                  }}
                  className="text-xs text-primary hover:underline px-3 py-1.5"
                >
                  Clear search
                </button>
                {!isAdvancedQuery && (
                  <button
                    onClick={() => setIsAdvancedQuery(true)}
                    className="text-xs text-muted-foreground/50 hover:text-primary flex items-center gap-1.5 transition-colors px-3 py-1.5"
                  >
                    <Regex className="w-3 h-3" />
                    Advanced query
                  </button>
                )}
              </div>
            </div>
          ) : logs.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full text-muted-foreground gap-3">
              {levelFilter !== "all" ? (
                <>
                  <div className="w-12 h-12 bg-secondary/50 border border-border/50 flex items-center justify-center mb-1">
                    <AlertTriangle className="w-5 h-5 text-muted-foreground/30" />
                  </div>
                  <span className="text-sm text-foreground/50">
                    No {levelFilter === "error" ? "errors" : "warnings"}
                  </span>
                  <button
                    onClick={() => setLevelFilter("all")}
                    className="text-xs text-primary hover:underline px-3 py-1.5"
                  >
                    Show all logs
                  </button>
                </>
              ) : debouncedSearch ? (
                <div className="flex flex-col items-center gap-2">
                  <div className="w-12 h-12 bg-secondary/50 border border-border/50 flex items-center justify-center mb-1">
                    <Search className="w-5 h-5 text-muted-foreground/20" />
                  </div>
                  <span className="text-sm text-foreground/50">
                    No matches for{" "}
                    <span className="font-mono text-foreground/60">
                      {debouncedSearch}
                    </span>
                  </span>
                  {!isAdvancedQuery && (
                    <button
                      onClick={() => setIsAdvancedQuery(true)}
                      className="text-xs text-muted-foreground/40 hover:text-primary flex items-center gap-1.5 transition-colors mt-1"
                    >
                      <Regex className="w-3 h-3" />
                      Try advanced query
                    </button>
                  )}
                </div>
              ) : (
                <div className="flex flex-col items-center gap-2">
                  <div className="flex items-center gap-1">
                    <span
                      className="w-1 h-1 rounded-full bg-muted-foreground/40 animate-pulse"
                      style={{ animationDelay: "0ms" }}
                    />
                    <span
                      className="w-1 h-1 rounded-full bg-muted-foreground/40 animate-pulse"
                      style={{ animationDelay: "300ms" }}
                    />
                    <span
                      className="w-1 h-1 rounded-full bg-muted-foreground/40 animate-pulse"
                      style={{ animationDelay: "600ms" }}
                    />
                  </div>
                  <span className="text-sm text-muted-foreground/60">
                    Waiting for output
                  </span>
                </div>
              )}
            </div>
          ) : (
            <div
              style={{
                height: virtualizer.getTotalSize(),
                width: "100%",
                position: "relative",
              }}
            >
              {virtualizer.getVirtualItems().map((virtualRow) => {
                const i = virtualRow.index;
                const log = logs[i];
                const prevService = i > 0 ? logs[i - 1].service : null;
                const showLabel = showServiceColumn && log.service !== prevService;
                const svcColorIndex = colorIndexMap.get(log.service) ?? 0;

                return (
                  <LogRow
                    key={virtualRow.key}
                    virtualRow={virtualRow}
                    measureElement={virtualizer.measureElement}
                    log={log}
                    index={i}
                    lineNumber={i + 1}
                    showLabel={showLabel}
                    showServiceColumn={showServiceColumn}
                    svcColorIndex={svcColorIndex}
                    highlighter={highlighter}
                    isActiveMatch={!!debouncedSearch && i === activeMatchIndex}
                    isExpanded={expandedRow === i}
                    onToggleExpand={toggleExpand}
                    hasBorderTop={showLabel && i > 0}
                  />
                );
              })}
            </div>
          )}
        </div>
      </div>

      {/* New logs toast with count (14.18) */}
      <LogScrollControls
        newLogCount={newLogCount}
        isAtBottom={isAtBottom}
        hasSearch={!!debouncedSearch}
        onScrollToBottom={scrollToBottom}
      />

      {/* Keyboard shortcuts hint — hidden on mobile (no physical keyboard) */}
      <div className="desktop-only-hints absolute bottom-2.5 left-3 flex items-center gap-3 text-[11px] text-muted-foreground/25 pointer-events-none select-none">
        <span className="flex items-center gap-1">
          <Kbd>/</Kbd> search
        </span>
        <span className="flex items-center gap-1">
          <Kbd>E</Kbd> errors
        </span>
        <span className="flex items-center gap-1">
          <Kbd>W</Kbd> warns
        </span>
        <span className="flex items-center gap-1">
          <Kbd>F</Kbd> facets
        </span>
        <span className="flex items-center gap-1">
          <Kbd>⌘G</Kbd> next
        </span>
        <span className="flex items-center gap-1">
          <Kbd>Esc</Kbd> clear
        </span>
      </div>
    </div>
  );
}

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="inline-flex items-center justify-center h-4 min-w-[16px] px-1 bg-secondary/50 border border-border/30 text-[10px] font-mono">
      {children}
    </kbd>
  );
}
