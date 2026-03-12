import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { Search, Play, Square, RotateCcw, Copy, Filter, ArrowDown, Keyboard } from "lucide-react";
import { cn } from "@/lib/utils";

export interface CommandAction {
  id: string;
  label: string;
  description?: string;
  icon?: React.ReactNode;
  shortcut?: string;
  action: () => void;
  section?: string;
}

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  actions: CommandAction[];
}

export function CommandPalette({ open, onClose, actions }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
      setSelectedIndex(0);
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  const filtered = useMemo(() => {
    if (!query.trim()) return actions;
    const lower = query.toLowerCase();
    return actions.filter(
      (a) =>
        a.label.toLowerCase().includes(lower) ||
        a.description?.toLowerCase().includes(lower) ||
        a.section?.toLowerCase().includes(lower),
    );
  }, [actions, query]);

  const grouped = useMemo(() => {
    const groups: { section: string; items: CommandAction[] }[] = [];
    const sectionMap = new Map<string, CommandAction[]>();
    for (const action of filtered) {
      const section = action.section ?? "Actions";
      if (!sectionMap.has(section)) {
        sectionMap.set(section, []);
        groups.push({ section, items: sectionMap.get(section)! });
      }
      sectionMap.get(section)!.push(action);
    }
    return groups;
  }, [filtered]);

  const flatItems = useMemo(() => filtered, [filtered]);

  useEffect(() => {
    if (selectedIndex >= flatItems.length) {
      setSelectedIndex(Math.max(0, flatItems.length - 1));
    }
  }, [flatItems.length, selectedIndex]);

  const execute = useCallback(
    (action: CommandAction) => {
      onClose();
      requestAnimationFrame(() => action.action());
    },
    [onClose],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedIndex((i) => Math.min(i + 1, flatItems.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedIndex((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const item = flatItems[selectedIndex];
        if (item) execute(item);
      } else if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    },
    [flatItems, selectedIndex, execute, onClose],
  );

  if (!open) return null;

  let globalIndex = 0;

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[12vh] md:pt-[20vh] px-3 md:px-0">
      {/* Backdrop */}
      <div
        className="absolute inset-0 bg-surface-base/80 backdrop-blur-sm animate-in fade-in-0 duration-100"
        onClick={onClose}
      />
      {/* Palette */}
      <div
        className="relative w-full max-w-[540px] max-h-[70vh] md:max-h-[400px] bg-surface-overlay border border-line shadow-2xl rounded-lg flex flex-col animate-in fade-in-0 slide-in-from-top-2 duration-100 overflow-hidden"
        role="dialog"
        aria-label="Command palette"
      >
        {/* Search */}
        <div className="flex items-center gap-2.5 px-4 h-12 border-b border-line">
          <Search className="w-4 h-4 text-ink-tertiary shrink-0" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setSelectedIndex(0);
            }}
            onKeyDown={handleKeyDown}
            placeholder="Type a command…"
            className="flex-1 bg-transparent text-sm text-ink placeholder:text-ink-tertiary outline-none"
            spellCheck={false}
          />
          <kbd className="text-[10px] text-ink-tertiary border border-line px-1.5 py-0.5 font-mono rounded hidden md:inline">
            esc
          </kbd>
        </div>

        {/* Results */}
        <div className="overflow-auto flex-1">
          {filtered.length === 0 ? (
            <div className="px-4 py-8 text-center text-sm text-ink-tertiary">
              No matching commands
            </div>
          ) : (
            grouped.map((group) => (
              <div key={group.section}>
                <div className="px-4 pt-2.5 pb-1 text-[11px] font-semibold tracking-wider uppercase text-ink-tertiary">
                  {group.section}
                </div>
                {group.items.map((item) => {
                  const idx = globalIndex++;
                  return (
                    <button
                      key={item.id}
                      onClick={() => execute(item)}
                      onMouseEnter={() => setSelectedIndex(idx)}
                      className={cn(
                        "w-full flex items-center gap-3 px-4 py-2.5 md:py-2 text-left transition-colors",
                        idx === selectedIndex
                          ? "bg-surface-sunken text-ink"
                          : "text-ink-secondary hover:bg-surface-sunken/50",
                      )}
                    >
                      {item.icon && (
                        <span className="w-4 h-4 shrink-0 text-ink-tertiary flex items-center justify-center">
                          {item.icon}
                        </span>
                      )}
                      <span className="text-sm flex-1 min-w-0 truncate">{item.label}</span>
                      {item.description && (
                        <span className="text-xs text-ink-tertiary truncate">
                          {item.description}
                        </span>
                      )}
                      {item.shortcut && (
                        <kbd className="text-[10px] text-ink-tertiary border border-line px-1.5 py-0.5 font-mono rounded shrink-0">
                          {item.shortcut}
                        </kbd>
                      )}
                    </button>
                  );
                })}
              </div>
            ))
          )}
        </div>

        {/* Footer */}
        <div className="px-4 py-2 border-t border-line-subtle flex items-center gap-4 text-[10px] text-ink-tertiary">
          <span>↑↓ navigate</span>
          <span>↵ execute</span>
          <span>esc close</span>
        </div>
      </div>
    </div>
  );
}

/** Build common command actions for the dashboard */
export function useDashboardCommands({
  services,
  onSelectService,
  onFocusSearch,
  onToggleErrors,
  onToggleWarns,
  onToggleFacets,
  onScrollToBottom,
  onCopyUrl,
  onRestartService,
}: {
  services: string[];
  onSelectService: (name: string | null) => void;
  onFocusSearch: () => void;
  onToggleErrors: () => void;
  onToggleWarns: () => void;
  onToggleFacets: () => void;
  onScrollToBottom: () => void;
  onCopyUrl: () => void;
  onRestartService?: (name: string) => void;
}): CommandAction[] {
  return useMemo(() => {
    const actions: CommandAction[] = [
      { id: "search", label: "Focus search", shortcut: "/", icon: <Search className="w-3.5 h-3.5" />, action: onFocusSearch, section: "Navigation" },
      { id: "filter-errors", label: "Toggle error filter", shortcut: "E", icon: <Filter className="w-3.5 h-3.5" />, action: onToggleErrors, section: "Filters" },
      { id: "filter-warns", label: "Toggle warn filter", shortcut: "W", icon: <Filter className="w-3.5 h-3.5" />, action: onToggleWarns, section: "Filters" },
      { id: "toggle-facets", label: "Toggle facets panel", shortcut: "F", icon: <Filter className="w-3.5 h-3.5" />, action: onToggleFacets, section: "Filters" },
      { id: "all-services", label: "Show all services", shortcut: "1", icon: <ArrowDown className="w-3.5 h-3.5" />, action: () => onSelectService(null), section: "Services" },
      ...services.map((svc, i) => ({
        id: `service-${svc}`,
        label: `Filter to ${svc}`,
        shortcut: `${i + 2}`,
        icon: <Play className="w-3.5 h-3.5" />,
        action: () => onSelectService(svc),
        section: "Services",
      })),
      { id: "scroll-bottom", label: "Scroll to latest", icon: <ArrowDown className="w-3.5 h-3.5" />, action: onScrollToBottom, section: "Navigation" },
      { id: "copy-url", label: "Copy current URL", icon: <Copy className="w-3.5 h-3.5" />, action: onCopyUrl, section: "Actions" },
      { id: "shortcuts", label: "Keyboard shortcuts", shortcut: "?", icon: <Keyboard className="w-3.5 h-3.5" />, action: () => {}, section: "Help" },
    ];

    if (onRestartService) {
      for (const svc of services) {
        actions.push({
          id: `restart-${svc}`,
          label: `Restart ${svc}`,
          icon: <RotateCcw className="w-3.5 h-3.5" />,
          action: () => onRestartService(svc),
          section: "Actions",
        });
      }
    }

    return actions;
  }, [services, onSelectService, onFocusSearch, onToggleErrors, onToggleWarns, onToggleFacets, onScrollToBottom, onCopyUrl, onRestartService]);
}

export { Play, Square, RotateCcw, Copy, Filter, ArrowDown, Keyboard };
