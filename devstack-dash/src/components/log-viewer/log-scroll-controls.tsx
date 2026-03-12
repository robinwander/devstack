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
      className="absolute bottom-3 right-3 flex items-center gap-2 px-3 h-8 bg-surface-raised border border-line text-xs font-medium text-ink shadow-lg hover:bg-surface-sunken rounded-md transition-colors new-logs-toast"
    >
      <ArrowDown className="w-3.5 h-3.5" />
      ↓ {newLogCount} new lines
    </button>
  );
}
