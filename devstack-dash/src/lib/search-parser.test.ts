import { describe, expect, it } from 'vitest'
import {
  addToken,
  parseSearch,
  removeToken,
  replaceAllTokens,
  serializeSearch,
} from './search-parser'

describe('parseSearch', () => {
  it('extracts field tokens and free text from mixed queries', () => {
    const parsed = parseSearch('event:extract_tool_result level:error free text')

    expect(parsed.tokens).toEqual([
      {
        field: 'event',
        value: 'extract_tool_result',
        negated: false,
        raw: 'event:extract_tool_result',
        start: 0,
        end: 25,
      },
      {
        field: 'level',
        value: 'error',
        negated: false,
        raw: 'level:error',
        start: 26,
        end: 37,
      },
    ])
    expect(parsed.freeText).toBe('free text')
  })

  it('parses negated field tokens', () => {
    const parsed = parseSearch('-service:worker timeout -stream:stderr')

    expect(parsed.tokens).toEqual([
      {
        field: 'service',
        value: 'worker',
        negated: true,
        raw: '-service:worker',
        start: 0,
        end: 15,
      },
      {
        field: 'stream',
        value: 'stderr',
        negated: true,
        raw: '-stream:stderr',
        start: 24,
        end: 38,
      },
    ])
    expect(parsed.freeText).toBe('timeout')
  })

  it('parses quoted token values and quoted free text', () => {
    const parsed = parseSearch(
      'event:"extract tool result" message:"quoted \\\"value\\\"" "free text"',
    )

    expect(parsed.tokens).toEqual([
      {
        field: 'event',
        value: 'extract tool result',
        negated: false,
        raw: 'event:"extract tool result"',
        start: 0,
        end: 27,
      },
      {
        field: 'message',
        value: 'quoted "value"',
        negated: false,
        raw: 'message:"quoted \\\"value\\\""',
        start: 28,
        end: 54,
      },
    ])
    expect(parsed.freeText).toBe('free text')
  })

  it('treats URLs and malformed tokens as free text', () => {
    const parsed = parseSearch(
      'https://example.test field:"unterminated field: plain:',
    )

    expect(parsed.tokens).toEqual([])
    expect(parsed.freeText).toBe(
      'https://example.test field:"unterminated field: plain:',
    )
  })

  it('normalizes field names to lowercase', () => {
    const parsed = parseSearch('LEVEL:error Event:extract_tool_result')

    expect(parsed.tokens.map((token) => token.field)).toEqual([
      'level',
      'event',
    ])
  })
})

describe('search parser helpers', () => {
  it('serializes parsed search into a normalized query string', () => {
    const serialized = serializeSearch(
      parseSearch('level:error event:"extract tool result" free text'),
    )

    expect(serialized).toBe('level:error event:"extract tool result" free text')
  })

  it('adds tokens with quoting when needed', () => {
    expect(addToken('free text', 'event', 'extract tool result')).toBe(
      'free text event:"extract tool result"',
    )
  })

  it('removes only the requested token instance', () => {
    expect(removeToken('level:error level:error timeout', 'level:error')).toBe(
      'level:error timeout',
    )
  })

  it('replaces all tokens for a field while keeping free text and other filters', () => {
    expect(
      replaceAllTokens(
        'level:warn level:error stream:stderr free text',
        'level',
        'info',
      ),
    ).toBe('stream:stderr level:info free text')
  })
})
