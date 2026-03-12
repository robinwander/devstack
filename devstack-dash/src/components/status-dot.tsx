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
  const px = size === "md" ? "w-2 h-2" : "w-1.5 h-1.5";
  return (
    <span
      className={cn("rounded-full shrink-0", px, style.dot, state === "starting" && "pulse-dot")}
      aria-hidden="true"
    />
  );
}

export function StatusIcon({ state }: { state: string }) {
  const style = getStateStyle(state);
  const base = "w-4 h-4 shrink-0";

  switch (state) {
    case "running":
    case "ready":
      return <CheckCircle2 className={cn(base, style.iconColor)} aria-label="Ready" />;
    case "starting":
      return <Loader2 className={cn(base, "animate-spin", style.iconColor)} aria-label="Starting" />;
    case "degraded":
      return <AlertCircle className={cn(base, style.iconColor)} aria-label="Degraded" />;
    case "failed":
      return <XCircle className={cn(base, style.iconColor)} aria-label="Failed" />;
    case "stopped":
    default:
      return <MinusCircle className={cn(base, style.iconColor)} aria-label="Stopped" />;
  }
}
