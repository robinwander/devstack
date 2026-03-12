/**
 * Service/run state → style mapping.
 * All styles reference design tokens via Tailwind utility classes.
 */

export type AnyState = "running" | "ready" | "starting" | "degraded" | "failed" | "stopped";

export interface StateStyle {
  dot: string;
  iconColor: string;
  textColor: string;
}

const stateStyles: Record<AnyState, StateStyle> = {
  running: {
    dot: "bg-status-green",
    iconColor: "text-status-green-text",
    textColor: "text-status-green-text",
  },
  ready: {
    dot: "bg-status-green",
    iconColor: "text-status-green-text",
    textColor: "text-status-green-text",
  },
  starting: {
    dot: "bg-status-amber",
    iconColor: "text-status-amber-text",
    textColor: "text-status-amber-text",
  },
  degraded: {
    dot: "bg-status-amber",
    iconColor: "text-status-amber-text",
    textColor: "text-status-amber-text",
  },
  failed: {
    dot: "bg-status-red",
    iconColor: "text-status-red-text",
    textColor: "text-status-red-text",
  },
  stopped: {
    dot: "bg-ink-tertiary",
    iconColor: "text-ink-tertiary",
    textColor: "text-ink-tertiary",
  },
};

export function getStateStyle(state: string): StateStyle {
  return stateStyles[state as AnyState] ?? stateStyles.stopped;
}
