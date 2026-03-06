import React, { type ReactNode } from "react";

type Props = {
  children: ReactNode;
};

type State = {
  error: Error | null;
};

export class ErrorBoundary extends React.Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return {
      error: error instanceof Error ? error : new Error(String(error)),
    };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    // Keep a useful breadcrumb for debugging in the browser.
    // eslint-disable-next-line no-console
    console.error("[devstack-dash] uncaught render error", error, info);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="min-h-screen bg-background text-foreground p-6">
        <div className="max-w-3xl">
          <div className="text-xs font-semibold tracking-[0.08em] text-muted-foreground uppercase">
            devstack dashboard
          </div>
          <h1 className="mt-2 text-xl font-semibold">App crashed</h1>
          <p className="mt-2 text-sm text-muted-foreground/70">
            A UI render error occurred. Open DevTools and check the Console for details.
          </p>

          <div className="mt-4 border border-border bg-card/50 p-4">
            <div className="text-sm font-mono text-red-400/80 break-words">
              {error.name}: {error.message}
            </div>
            {error.stack && (
              <pre className="mt-3 text-xs font-mono text-muted-foreground/60 whitespace-pre-wrap break-words">
                {error.stack}
              </pre>
            )}
          </div>

          <div className="mt-4 flex gap-2">
            <button
              className="px-3 h-9 bg-secondary/50 border border-border hover:bg-secondary transition-colors text-sm"
              onClick={() => window.location.reload()}
            >
              Reload
            </button>
          </div>
        </div>
      </div>
    );
  }
}

