export interface CustomTimeRangeDraft {
  fromInput: string
  toInput: string
}

export interface ResolvedCustomTimeRange {
  fromInput: string
  toInput: string
  fromIso?: string
  toIso?: string
  fromError?: string
  toError?: string
  rangeError?: string
  hasValue: boolean
}

const RELATIVE_TIME_RE = /^(\d+)(s|m|h|d)\s+ago$/i

const TIME_UNITS_MS: Record<string, number> = {
  s: 1000,
  m: 60_000,
  h: 60 * 60_000,
  d: 24 * 60 * 60_000,
}

export function parseTimeExpression(
  input: string,
  now: Date = new Date(),
): string | null {
  const trimmed = input.trim()
  if (!trimmed) return null

  const relativeMatch = RELATIVE_TIME_RE.exec(trimmed)
  if (relativeMatch) {
    const amount = Number.parseInt(relativeMatch[1], 10)
    const unit = relativeMatch[2].toLowerCase()
    const durationMs = TIME_UNITS_MS[unit]
    if (!Number.isFinite(amount) || !durationMs) return null
    return new Date(now.getTime() - amount * durationMs).toISOString()
  }

  const parsed = new Date(trimmed)
  if (Number.isNaN(parsed.getTime())) return null
  return parsed.toISOString()
}

export function resolveCustomTimeRange(
  draft: CustomTimeRangeDraft,
  now: Date = new Date(),
): ResolvedCustomTimeRange {
  const fromInput = draft.fromInput.trim()
  const toInput = draft.toInput.trim()
  const fromIso = parseTimeExpression(fromInput, now) ?? undefined
  const toIso = parseTimeExpression(toInput, now) ?? undefined

  const resolved: ResolvedCustomTimeRange = {
    fromInput,
    toInput,
    fromIso,
    toIso,
    hasValue: fromInput.length > 0 || toInput.length > 0,
  }

  if (fromInput && !fromIso) {
    resolved.fromError =
      'Use RFC3339, datetime-local, or a relative time like 2h ago'
  }
  if (toInput && !toIso) {
    resolved.toError =
      'Use RFC3339, datetime-local, or a relative time like 30m ago'
  }
  if (fromIso && toIso && new Date(fromIso).getTime() > new Date(toIso).getTime()) {
    resolved.rangeError = 'From must be before To'
  }

  return resolved
}

function pad(value: number): string {
  return String(value).padStart(2, '0')
}

export function isoToDateTimeLocalValue(input?: string): string {
  if (!input) return ''
  const parsed = new Date(input)
  if (Number.isNaN(parsed.getTime())) return ''

  return [
    parsed.getFullYear(),
    pad(parsed.getMonth() + 1),
    pad(parsed.getDate()),
  ].join('-') + `T${pad(parsed.getHours())}:${pad(parsed.getMinutes())}`
}
