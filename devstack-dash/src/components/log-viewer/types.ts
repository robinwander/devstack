export interface ParsedLog {
  timestamp: string;
  rawTimestamp: string;
  content: string;
  service: string;
  stream: string;
  level: "info" | "warn" | "error";
  raw: string;
  json?: Record<string, unknown>;
  attributes?: Record<string, string>;
}

export type TimeRange = "live" | "5m" | "15m" | "1h" | "custom";
