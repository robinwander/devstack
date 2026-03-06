export function LogoMark() {
  return (
    <div className="w-7 h-7 relative flex items-center justify-center">
      <div className="absolute inset-0 bg-primary/8 border border-primary/15" />
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none" className="relative">
        <rect x="1" y="2" width="5" height="5" rx="0.5" fill="currentColor" className="text-primary" opacity="0.9" />
        <rect x="8" y="2" width="5" height="5" rx="0.5" fill="currentColor" className="text-primary" opacity="0.5" />
        <rect x="1" y="9" width="5" height="5" rx="0.5" fill="currentColor" className="text-primary" opacity="0.5" />
        <rect x="8" y="9" width="5" height="5" rx="0.5" fill="currentColor" className="text-primary" opacity="0.3" />
        <line x1="6.5" y1="4.5" x2="7.5" y2="4.5" stroke="currentColor" className="text-primary" strokeWidth="0.75" opacity="0.4" />
        <line x1="3.5" y1="7.5" x2="3.5" y2="8.5" stroke="currentColor" className="text-primary" strokeWidth="0.75" opacity="0.4" />
      </svg>
    </div>
  );
}
