import { ArrowDown } from "lucide-react";

interface LogScrollControlsProps {
  newLogCount: number;
  isAtBottom: boolean;
  hasSearch: boolean;
  onScrollToBottom: () => void;
}

export function LogScrollControls({
  newLogCount,
  isAtBottom,
  hasSearch,
  onScrollToBottom,
}: LogScrollControlsProps) {
  if (isAtBottom || hasSearch || newLogCount === 0) return null;

  return (
    <button
      onClick={onScrollToBottom}
      className="absolute bottom-4 right-4 flex items-center gap-2 px-3 h-8 bg-card/90 backdrop-blur-sm border border-border text-xs font-medium text-foreground shadow-lg hover:bg-secondary transition-all hover:shadow-xl new-logs-toast"
    >
      <ArrowDown className="w-3.5 h-3.5" />
      {/* Show count of new lines (14.18) */}
      {newLogCount > 0 ? `↓ ${newLogCount} new lines` : "New logs"}
    </button>
  );
}
