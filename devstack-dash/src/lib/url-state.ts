import { useCallback, useMemo, useSyncExternalStore } from 'react'

type UrlParamValue = string | number | null | undefined

// --- Centralized URL state store ---
const listeners = new Set<() => void>()

function getSnapshot(): string {
  return window.location.search
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function notify() {
  for (const fn of listeners) fn()
}

// Legacy API — still works but now also notifies subscribers
export function readUrlParam(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name)
}

export function patchUrlParams(patch: Record<string, UrlParamValue>): void {
  const url = new URL(window.location.href)
  let changed = false
  const normalizedPatch = { ...patch }

  if (
    normalizedPatch.source !== undefined &&
    normalizedPatch.source !== null &&
    normalizedPatch.source !== ''
  ) {
    normalizedPatch.run = null
  } else if (
    normalizedPatch.run !== undefined &&
    normalizedPatch.run !== null &&
    normalizedPatch.run !== ''
  ) {
    normalizedPatch.source = null
  }

  for (const [key, value] of Object.entries(normalizedPatch)) {
    if (value === undefined || value === null || value === '') {
      if (url.searchParams.has(key)) {
        url.searchParams.delete(key)
        changed = true
      }
      continue
    }

    const next = String(value)
    if (url.searchParams.get(key) !== next) {
      url.searchParams.set(key, next)
      changed = true
    }
  }

  if (!changed) return

  const search = url.searchParams.toString()
  const nextUrl = `${url.pathname}${search ? `?${search}` : ''}${url.hash}`
  window.history.replaceState({}, '', nextUrl)
  notify()
}

// --- Reactive hook: subscribe to a URL parameter with useSyncExternalStore ---
export function useUrlParam(
  name: string,
): [string | null, (v: string | null) => void] {
  const search = useSyncExternalStore(subscribe, getSnapshot)
  const value = useMemo(
    () => new URLSearchParams(search).get(name),
    [search, name],
  )
  const setValue = useCallback(
    (v: string | null) => patchUrlParams({ [name]: v }),
    [name],
  )
  return [value, setValue]
}
