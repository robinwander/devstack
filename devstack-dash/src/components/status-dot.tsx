import {
  CheckCircle2,
  Loader2,
  AlertCircle,
  XCircle,
  MinusCircle,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { getStateStyle } from "@/lib/status";

export function StatusDot({ state, size = "sm" }: { state: string; size?: "sm" | "md" }) {
  const style = getStateStyle(state);
  const px = size === "md" ? "w-2.5 h-2.5" : "w-2 h-2";
  return (
    <span
      className={cn(
        "rounded-full shrink-0",
        px,
        style.dot,
        style.glow,
        // Only pulse on "starting" — ready is steady (15.2)
        state === "starting" && "pulse-dot",
      )}
      aria-hidden="true"
    />
  );
}

export function StatusIcon({ state }: { state: string }) {
  const style = getStateStyle(state);

  switch (state) {
    case "running":
    case "ready":
      return <CheckCircle2 className={cn("w-4 h-4 shrink-0", style.iconColor)} aria-label="Ready" />;
    case "starting":
      return <Loader2 className={cn("w-4 h-4 animate-spin shrink-0", style.iconColor)} aria-label="Starting" />;
    case "degraded":
      return <AlertCircle className={cn("w-4 h-4 shrink-0", style.iconColor)} aria-label="Degraded" />;
    case "failed":
      return <XCircle className={cn("w-4 h-4 shrink-0", style.iconColor)} aria-label="Failed" />;
    case "stopped":
    default:
      return <MinusCircle className={cn("w-4 h-4 shrink-0", style.iconColor)} aria-label="Stopped" />;
  }
}
