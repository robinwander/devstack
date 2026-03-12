/**
 * Deterministic service color assignment via name hashing.
 * "api" is always the same color, across runs, sessions, and machines.
 */

const SERVICE_COLOR_COUNT = 8;

function hashString(str: string): number {
  let hash = 5381;
  for (let i = 0; i < str.length; i++) {
    hash = ((hash << 5) + hash + str.charCodeAt(i)) >>> 0;
  }
  return hash;
}

export function getServiceColorIndex(serviceName: string): number {
  return hashString(serviceName) % SERVICE_COLOR_COUNT;
}

/**
 * Build a color index map for a list of service names.
 * Each service always gets the same index regardless of order.
 */
export function buildColorIndexMap(services: string[]): Map<string, number> {
  const map = new Map<string, number>();
  for (const svc of services) {
    map.set(svc, getServiceColorIndex(svc));
  }
  return map;
}
