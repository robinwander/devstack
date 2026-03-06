/** Loading skeleton shown when switching runs (13.1) */
export function LogSkeleton() {
  const widths = [75, 60, 90, 45, 80, 55, 70, 85, 50, 65, 40, 72];
  return (
    <div className="flex-1 p-4 space-y-2">
      {widths.map((w, i) => (
        <div key={i} className="flex items-center gap-3">
          <div className="w-12 h-4 skeleton-shimmer shrink-0" />
          <div className="h-4 skeleton-shimmer" style={{ width: `${w}%` }} />
        </div>
      ))}
    </div>
  );
}
