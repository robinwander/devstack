import { useState, useEffect } from "react";

/**
 * Simple media query hook. Returns true when the query matches.
 * Uses matchMedia for efficiency (no resize listener).
 * Falls back to false in test environments where matchMedia is unavailable.
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
    return window.matchMedia(query).matches;
  });

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mql = window.matchMedia(query);
    const handler = (e: MediaQueryListEvent) => setMatches(e.matches);
    mql.addEventListener("change", handler);
    // Sync in case it changed between render and effect
    setMatches(mql.matches);
    return () => mql.removeEventListener("change", handler);
  }, [query]);

  return matches;
}

/** True when viewport is < 768px (mobile) */
export function useIsMobile(): boolean {
  return useMediaQuery("(max-width: 767px)");
}
