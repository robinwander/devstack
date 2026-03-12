import { describe, expect, it } from 'vitest'
import {
  isoToDateTimeLocalValue,
  parseTimeExpression,
  resolveCustomTimeRange,
} from './time-range'

describe('parseTimeExpression', () => {
  const now = new Date('2025-01-01T12:00:00.000Z')

  it('parses relative expressions like 2h ago', () => {
    expect(parseTimeExpression('2h ago', now)).toBe('2025-01-01T10:00:00.000Z')
    expect(parseTimeExpression('30m ago', now)).toBe('2025-01-01T11:30:00.000Z')
  })

  it('parses ISO and datetime-local timestamps', () => {
    expect(parseTimeExpression('2025-01-01T09:15:00Z', now)).toBe(
      '2025-01-01T09:15:00.000Z',
    )
    expect(parseTimeExpression('2025-01-01T09:15', now)).toMatch(
      /^2025-01-01T\d{2}:15:00.000Z$/,
    )
  })

  it('returns null for invalid expressions', () => {
    expect(parseTimeExpression('yesterday', now)).toBeNull()
    expect(parseTimeExpression('', now)).toBeNull()
  })
})

describe('resolveCustomTimeRange', () => {
  const now = new Date('2025-01-01T12:00:00.000Z')

  it('validates custom bounds and reports ordering errors', () => {
    const resolved = resolveCustomTimeRange(
      { fromInput: '30m ago', toInput: '2h ago' },
      now,
    )

    expect(resolved.fromIso).toBe('2025-01-01T11:30:00.000Z')
    expect(resolved.toIso).toBe('2025-01-01T10:00:00.000Z')
    expect(resolved.rangeError).toBe('From must be before To')
  })

  it('reports field-level validation errors for invalid inputs', () => {
    const resolved = resolveCustomTimeRange(
      { fromInput: 'soon', toInput: 'later' },
      now,
    )

    expect(resolved.fromError).toContain('RFC3339')
    expect(resolved.toError).toContain('relative time')
    expect(resolved.hasValue).toBe(true)
  })
})

describe('isoToDateTimeLocalValue', () => {
  it('formats timestamps for datetime-local inputs', () => {
    expect(isoToDateTimeLocalValue('2025-01-01T12:34:56.000Z')).toMatch(
      /^2025-01-01T\d{2}:34$/,
    )
  })
})
