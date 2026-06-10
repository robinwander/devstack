import { StrictMode, Suspense, lazy } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Toaster } from "@/components/ui/sonner";
import { ErrorBoundary } from "@/components/error-boundary";
import "./styles.css";

const Dashboard = lazy(() =>
  import("@/components/dashboard").then((module) => ({
    default: module.Dashboard,
  })),
);
const LogAnimationTest = lazy(() =>
  import("@/components/log-animation-test").then((module) => ({
    default: module.LogAnimationTest,
  })),
);

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 1000,
      refetchOnWindowFocus: true,
    },
  },
});

const showTestHarness = window.location.hash === "#test-log-animation";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <ErrorBoundary>
        <Suspense fallback={<div className="min-h-dvh bg-surface-base" />}>
          {showTestHarness ? <LogAnimationTest /> : <Dashboard />}
        </Suspense>
        <Toaster position="bottom-right" />
      </ErrorBoundary>
    </QueryClientProvider>
  </StrictMode>,
);
