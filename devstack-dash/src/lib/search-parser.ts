export interface ParsedSearch {
  tokens: SearchToken[]
  freeText: string
}

export interface SearchToken {
  field: string
  value: string
  negated: boolean
  raw: string
  start: number
  end: number
}

const FIELD_RE = /^[A-Za-z_][A-Za-z0-9_.-]*$/

function isWhitespace(char: string): boolean {
  return char === ' ' || char === '\n' || char === '\t'
}

function parseQuotedValue(raw: string): string | null {
  if (!raw.startsWith('"')) return null

  let value = ''
  let escaped = false
  for (let i = 1; i < raw.length; i++) {
    const char = raw[i]
    if (escaped) {
      value += char
      escaped = false
      continue
    }
    if (char === '\\') {
      escaped = true
      continue
    }
    if (char === '"') {
      return i === raw.length - 1 ? value : null
    }
    value += char
  }

  return null
}

function normalizeTokenValue(rawValue: string): string | null {
  if (rawValue.startsWith('"')) {
    return parseQuotedValue(rawValue)
  }

  return rawValue.length > 0 ? rawValue : null
}

function parseToken(raw: string, start: number, end: number): SearchToken | null {
  const negated = raw.startsWith('-')
  const body = negated ? raw.slice(1) : raw
  const colonIndex = body.indexOf(':')
  if (colonIndex <= 0) return null

  const field = body.slice(0, colonIndex).toLowerCase()
  if (!FIELD_RE.test(field)) return null

  const rawValue = body.slice(colonIndex + 1)
  if (rawValue.startsWith('//')) return null

  const value = normalizeTokenValue(rawValue)
  if (value === null) return null

  return {
    field,
    value,
    negated,
    raw,
    start,
    end,
  }
}

function decodeFreeTextSegment(raw: string): string {
  const quoted = parseQuotedValue(raw)
  return quoted === null ? raw : quoted
}

function serializeValue(value: string): string {
  if (value === '' || /[\s"]/u.test(value)) {
    return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
  }
  return value
}

function formatToken(field: string, value: string, negated: boolean): string {
  return `${negated ? '-' : ''}${field}:${serializeValue(value)}`
}

function buildSearch(tokens: SearchToken[], freeText: string): string {
  const parts = tokens.map((token) =>
    formatToken(token.field, token.value, token.negated),
  )
  const trimmedFreeText = freeText.trim()
  if (trimmedFreeText) parts.push(trimmedFreeText)
  return parts.join(' ')
}

export function parseSearch(input: string): ParsedSearch {
  const tokens: SearchToken[] = []
  const freeTextSegments: string[] = []

  let index = 0
  while (index < input.length) {
    while (index < input.length && isWhitespace(input[index])) index++
    if (index >= input.length) break

    const start = index
    let inQuotes = false
    let escaped = false

    while (index < input.length) {
      const char = input[index]
      if (escaped) {
        escaped = false
        index++
        continue
      }
      if (char === '\\') {
        escaped = true
        index++
        continue
      }
      if (char === '"') {
        inQuotes = !inQuotes
        index++
        continue
      }
      if (!inQuotes && isWhitespace(char)) break
      index++
    }

    const end = index
    const raw = input.slice(start, end)
    const token = parseToken(raw, start, end)
    if (token) tokens.push(token)
    else freeTextSegments.push(decodeFreeTextSegment(raw))
  }

  return {
    tokens,
    freeText: freeTextSegments.join(' ').trim(),
  }
}

export function serializeSearch(parsed: ParsedSearch): string {
  return buildSearch(parsed.tokens, parsed.freeText)
}

export function addToken(input: string, field: string, value: string): string {
  const nextToken = formatToken(field.toLowerCase(), value, false)
  const trimmed = input.trim()
  return trimmed ? `${trimmed} ${nextToken}` : nextToken
}

export function removeToken(input: string, raw: string): string {
  const parsed = parseSearch(input)
  const nextTokens = [...parsed.tokens]
  const tokenIndex = nextTokens.findIndex((token) => token.raw === raw)
  if (tokenIndex === -1) return input.trim()
  nextTokens.splice(tokenIndex, 1)
  return buildSearch(nextTokens, parsed.freeText)
}

export function replaceAllTokens(
  input: string,
  field: string,
  value: string,
): string {
  const normalizedField = field.toLowerCase()
  const parsed = parseSearch(input)
  const nextTokens = parsed.tokens.filter(
    (token) => token.field !== normalizedField,
  )
  nextTokens.push({
    field: normalizedField,
    value,
    negated: false,
    raw: formatToken(normalizedField, value, false),
    start: 0,
    end: 0,
  })
  return buildSearch(nextTokens, parsed.freeText)
}
