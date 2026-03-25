import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Toaster } from "@/components/ui/sonner";
import { Dashboard } from "@/components/dashboard";
import { ErrorBoundary } from "@/components/error-boundary";
import { LogAnimationTest } from "@/components/log-animation-test";
import "./styles.css";

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
        {showTestHarness ? <LogAnimationTest /> : <Dashboard />}
        <Toaster position="bottom-right" />
      </ErrorBoundary>
    </QueryClientProvider>
  </StrictMode>
);
