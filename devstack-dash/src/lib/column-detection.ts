import type { LogEntry } from './api'

export interface ColumnConfig {
  field: string
  label: string
  width: number
  visible: boolean
  builtIn: boolean
}

interface ColumnStats {
  field: string
  count: number
  uniqueValues: Set<string>
  maxValueLength: number
  idLikeCount: number
}

export const WELL_KNOWN_FIELDS = [
  'event',
  'type',
  'action',
  'kind',
  'name',
  'pid',
  'hostname',
  'host',
  'request_id',
  'trace_id',
  'span_id',
  'correlation_id',
  'method',
  'path',
  'url',
  'status',
  'status_code',
  'duration',
  'error',
  'err',
  'exception',
  'module',
  'component',
  'logger',
  'caller',
  'source',
  'toolname',
  'sessionid',
] as const

const BUILT_IN_COLUMNS: ColumnConfig[] = [
  {
    field: 'timestamp',
    label: 'timestamp',
    width: 108,
    visible: true,
    builtIn: true,
  },
  {
    field: 'service',
    label: 'service',
    width: 120,
    visible: true,
    builtIn: true,
  },
  {
    field: 'level',
    label: 'level',
    width: 72,
    visible: true,
    builtIn: true,
  },
  {
    field: 'message',
    label: 'message',
    width: 480,
    visible: true,
    builtIn: true,
  },
]

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value))
}

function humanizeField(field: string): string {
  return field.replace(/[_-]+/g, ' ')
}

function estimateColumnWidth(field: string, stats: ColumnStats): number {
  const contentLength = Math.max(field.length, stats.maxValueLength)
  return clamp(contentLength * 8 + 28, 96, 280)
}

function isIdLikeValue(value: string): boolean {
  if (value.length < 12 || /\s/u.test(value)) return false

  return (
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu.test(
      value,
    ) ||
    /^[0-9a-f]{16,}$/iu.test(value) ||
    /^[A-Za-z0-9_-]{20,}$/u.test(value)
  )
}

function getDynamicFieldScore(stats: ColumnStats, totalEntries: number): number {
  const normalizedField = stats.field.toLowerCase()
  const presenceRatio = totalEntries === 0 ? 0 : stats.count / totalEntries
  const uniqueCount = stats.uniqueValues.size
  const wellKnownIndex = WELL_KNOWN_FIELDS.indexOf(
    normalizedField as (typeof WELL_KNOWN_FIELDS)[number],
  )
  const wellKnownScore = wellKnownIndex === -1 ? 0 : 1000 - wellKnownIndex * 20
  const cardinalityScore =
    uniqueCount >= 2 && uniqueCount <= 50 ? 180 - Math.abs(uniqueCount - 6) : -80
  const frequencyScore = presenceRatio > 0.5 ? 120 + presenceRatio * 100 : presenceRatio * 40
  const idLikePenalty =
    uniqueCount > 1 && stats.idLikeCount / Math.max(stats.count, 1) >= 0.6 ? 260 : 0
  const singleValuePenalty = uniqueCount <= 1 ? 2000 : 0

  return wellKnownScore + cardinalityScore + frequencyScore - idLikePenalty - singleValuePenalty
}

export function mergeColumnConfig(
  detected: ColumnConfig[],
  saved: ColumnConfig[] | null | undefined,
): ColumnConfig[] {
  if (!saved || saved.length === 0) return detected

  const detectedMap = new Map(detected.map((column) => [column.field, column]))
  const merged: ColumnConfig[] = []

  for (const savedColumn of saved) {
    const detectedColumn = detectedMap.get(savedColumn.field)
    merged.push(
      detectedColumn
        ? {
            ...detectedColumn,
            label: savedColumn.label,
            width: savedColumn.width,
            visible: savedColumn.visible,
          }
        : savedColumn,
    )
    detectedMap.delete(savedColumn.field)
  }

  for (const detectedColumn of detected) {
    if (!detectedMap.has(detectedColumn.field)) continue
    merged.push(detectedColumn)
    detectedMap.delete(detectedColumn.field)
  }

  return merged
}

export function detectColumns(
  entries: Pick<LogEntry, 'attributes'>[],
  savedConfig?: ColumnConfig[] | null,
): ColumnConfig[] {
  const statsByField = new Map<string, ColumnStats>()

  for (const entry of entries) {
    for (const [field, value] of Object.entries(entry.attributes ?? {})) {
      const stats =
        statsByField.get(field) ??
        {
          field,
          count: 0,
          uniqueValues: new Set<string>(),
          maxValueLength: 0,
          idLikeCount: 0,
        }
      stats.count += 1
      stats.uniqueValues.add(value)
      stats.maxValueLength = Math.max(stats.maxValueLength, value.length)
      if (isIdLikeValue(value)) stats.idLikeCount += 1
      statsByField.set(field, stats)
    }
  }

  const rankedDynamicColumns = Array.from(statsByField.values())
    .sort((a, b) => {
      const scoreDelta =
        getDynamicFieldScore(b, entries.length) -
        getDynamicFieldScore(a, entries.length)
      if (scoreDelta !== 0) return scoreDelta

      const frequencyDelta = b.count - a.count
      if (frequencyDelta !== 0) return frequencyDelta

      return a.field.localeCompare(b.field)
    })
    .map<ColumnConfig>((stats, index) => ({
      field: stats.field,
      label: humanizeField(stats.field),
      width: estimateColumnWidth(stats.field, stats),
      visible: stats.uniqueValues.size > 1 && index < 3,
      builtIn: false,
    }))

  return mergeColumnConfig([...BUILT_IN_COLUMNS, ...rankedDynamicColumns], savedConfig)
}

function isColumnConfig(value: unknown): value is ColumnConfig {
  if (!value || typeof value !== 'object') return false

  const column = value as Record<string, unknown>
  return (
    typeof column.field === 'string' &&
    typeof column.label === 'string' &&
    typeof column.width === 'number' &&
    typeof column.visible === 'boolean' &&
    typeof column.builtIn === 'boolean'
  )
}

export function loadColumnConfig(storageKey: string): ColumnConfig[] | null {
  if (typeof window === 'undefined' || !window.localStorage) return null

  try {
    const raw = window.localStorage.getItem(storageKey)
    if (!raw) return null

    const parsed = JSON.parse(raw)
    return Array.isArray(parsed) && parsed.every(isColumnConfig) ? parsed : null
  } catch {
    return null
  }
}

export function saveColumnConfig(
  storageKey: string,
  config: ColumnConfig[],
): void {
  if (typeof window === 'undefined' || !window.localStorage) return
  window.localStorage.setItem(storageKey, JSON.stringify(config))
}
