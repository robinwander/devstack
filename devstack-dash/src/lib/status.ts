/**
 * Single source of truth for service/run state → color mapping.
 * Used by StatusDot and any other component displaying state.
 */

export type AnyState = "running" | "ready" | "starting" | "degraded" | "failed" | "stopped";

export interface StateStyle {
  /** Tailwind bg- class for the dot */
  dot: string;
  /** CSS glow class name */
  glow: string;
  /** Tailwind text- class for the icon */
  iconColor: string;
}

const stateStyles: Record<AnyState, StateStyle> = {
  running: {
    dot: "bg-emerald-400",
    glow: "status-glow-green",
    iconColor: "text-emerald-500",
  },
  ready: {
    dot: "bg-emerald-400",
    glow: "status-glow-green",
    iconColor: "text-emerald-500",
  },
  starting: {
    dot: "bg-amber-400",
    glow: "status-glow-amber",
    iconColor: "text-amber-500",
  },
  degraded: {
    dot: "bg-orange-400",
    glow: "status-glow-orange",
    iconColor: "text-orange-500",
  },
  failed: {
    dot: "bg-red-400",
    glow: "status-glow-red",
    iconColor: "text-red-500",
  },
  stopped: {
    dot: "bg-zinc-500",
    glow: "",
    iconColor: "text-zinc-500",
  },
};

export function getStateStyle(state: string): StateStyle {
  return stateStyles[state as AnyState] ?? stateStyles.stopped;
}
