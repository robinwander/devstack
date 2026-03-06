import type { ReactNode } from "react";

export function highlightAll(text: string, matcher: string | RegExp): ReactNode {
  if (typeof matcher === "string") return highlightSubstring(text, matcher);
  return highlightRegex(text, matcher);
}

function highlightSubstring(text: string, query: string): ReactNode {
  const lower = text.toLowerCase();
  const qLower = query.toLowerCase();
  const parts: ReactNode[] = [];
  let lastEnd = 0;
  let idx = lower.indexOf(qLower);
  let key = 0;
  while (idx !== -1) {
    if (idx > lastEnd) parts.push(text.slice(lastEnd, idx));
    parts.push(
      <mark key={key++} className="bg-primary/20 text-primary px-0.5">
        {text.slice(idx, idx + query.length)}
      </mark>,
    );
    lastEnd = idx + query.length;
    idx = lower.indexOf(qLower, lastEnd);
  }
  if (lastEnd < text.length) parts.push(text.slice(lastEnd));
  return <>{parts}</>;
}

function highlightRegex(text: string, regex: RegExp): ReactNode {
  const parts: ReactNode[] = [];
  let lastEnd = 0;
  let key = 0;
  const re = new RegExp(regex.source, "gi");
  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    if (match[0].length === 0) {
      re.lastIndex++;
      continue;
    }
    if (match.index > lastEnd) parts.push(text.slice(lastEnd, match.index));
    parts.push(
      <mark key={key++} className="bg-primary/20 text-primary px-0.5">
        {match[0]}
      </mark>,
    );
    lastEnd = match.index + match[0].length;
  }
  if (lastEnd < text.length) parts.push(text.slice(lastEnd));
  return <>{parts}</>;
}
