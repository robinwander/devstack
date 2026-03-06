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

  // Reset on open
  useEffect(() => {
    if (open) {
      setQuery("");
      setSelectedIndex(0);
      // Wait for DOM to settle then focus
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  // Filter actions by query
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

  // Group by section
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

  // Flatten for index-based navigation
  const flatItems = useMemo(() => filtered, [filtered]);

  // Clamp selection
  useEffect(() => {
    if (selectedIndex >= flatItems.length) {
      setSelectedIndex(Math.max(0, flatItems.length - 1));
    }
  }, [flatItems.length, selectedIndex]);

  const execute = useCallback(
    (action: CommandAction) => {
      onClose();
      // Defer to let the palette close first
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
        className="absolute inset-0 bg-background/80 backdrop-blur-sm animate-in fade-in-0 duration-100"
        onClick={onClose}
      />
      {/* Palette */}
      <div
        className="relative w-full max-w-[560px] max-h-[70vh] md:max-h-[400px] bg-popover border border-border shadow-2xl flex flex-col animate-in fade-in-0 slide-in-from-top-2 duration-100"
        role="dialog"
        aria-label="Command palette"
      >
        {/* Search */}
        <div className="flex items-center gap-2.5 px-4 h-12 md:h-12 border-b border-border">
          <Search className="w-4 h-4 text-muted-foreground/40 shrink-0" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setSelectedIndex(0);
            }}
            onKeyDown={handleKeyDown}
            placeholder="Type a command…"
            className="flex-1 bg-transparent text-base md:text-sm text-foreground placeholder:text-muted-foreground/35 outline-none"
            spellCheck={false}
          />
          <kbd className="text-[10px] text-muted-foreground/30 border border-border/40 px-1.5 py-0.5 font-mono hidden md:inline">
            esc
          </kbd>
        </div>

        {/* Results */}
        <div className="overflow-auto flex-1">
          {filtered.length === 0 ? (
            <div className="px-4 py-8 text-center text-sm text-muted-foreground/40">
              No matching commands
            </div>
          ) : (
            grouped.map((group) => (
              <div key={group.section}>
                <div className="px-4 pt-2.5 pb-1 text-[11px] font-semibold tracking-wider uppercase text-muted-foreground/40">
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
                        "w-full flex items-center gap-3 px-4 py-3 md:py-2 text-left transition-colors",
                        idx === selectedIndex
                          ? "bg-secondary/70 text-foreground"
                          : "text-foreground/70 hover:bg-secondary/40",
                      )}
                    >
                      {item.icon && (
                        <span className="w-4 h-4 shrink-0 text-muted-foreground/50 flex items-center justify-center">
                          {item.icon}
                        </span>
                      )}
                      <span className="text-sm flex-1 min-w-0 truncate">{item.label}</span>
                      {item.description && (
                        <span className="text-xs text-muted-foreground/40 truncate">
                          {item.description}
                        </span>
                      )}
                      {item.shortcut && (
                        <kbd className="text-[10px] text-muted-foreground/30 border border-border/30 px-1.5 py-0.5 font-mono shrink-0">
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

        {/* Footer hint */}
        <div className="px-4 py-2 border-t border-border/30 flex items-center gap-4 text-[10px] text-muted-foreground/30">
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
      {
        id: "search",
        label: "Focus search",
        shortcut: "/",
        icon: <Search className="w-3.5 h-3.5" />,
        action: onFocusSearch,
        section: "Navigation",
      },
      {
        id: "filter-errors",
        label: "Toggle error filter",
        shortcut: "E",
        icon: <Filter className="w-3.5 h-3.5" />,
        action: onToggleErrors,
        section: "Filters",
      },
      {
        id: "filter-warns",
        label: "Toggle warn filter",
        shortcut: "W",
        icon: <Filter className="w-3.5 h-3.5" />,
        action: onToggleWarns,
        section: "Filters",
      },
      {
        id: "toggle-facets",
        label: "Toggle facets panel",
        shortcut: "F",
        icon: <Filter className="w-3.5 h-3.5" />,
        action: onToggleFacets,
        section: "Filters",
      },
      {
        id: "all-services",
        label: "Show all services",
        shortcut: "1",
        icon: <ArrowDown className="w-3.5 h-3.5" />,
        action: () => onSelectService(null),
        section: "Services",
      },
      ...services.map((svc, i) => ({
        id: `service-${svc}`,
        label: `Filter to ${svc}`,
        shortcut: `${i + 2}`,
        icon: <Play className="w-3.5 h-3.5" />,
        action: () => onSelectService(svc),
        section: "Services",
      })),
      {
        id: "scroll-bottom",
        label: "Scroll to latest",
        icon: <ArrowDown className="w-3.5 h-3.5" />,
        action: onScrollToBottom,
        section: "Navigation",
      },
      {
        id: "copy-url",
        label: "Copy current URL",
        icon: <Copy className="w-3.5 h-3.5" />,
        action: onCopyUrl,
        section: "Actions",
      },
      {
        id: "shortcuts",
        label: "Keyboard shortcuts",
        shortcut: "?",
        icon: <Keyboard className="w-3.5 h-3.5" />,
        action: () => {
          /* TODO: show shortcuts overlay */
        },
        section: "Help",
      },
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
  }, [
    services,
    onSelectService,
    onFocusSearch,
    onToggleErrors,
    onToggleWarns,
    onToggleFacets,
    onScrollToBottom,
    onCopyUrl,
    onRestartService,
  ]);
}

// Re-export icon components used by callers
export { Play, Square, RotateCcw, Copy, Filter, ArrowDown, Keyboard };
