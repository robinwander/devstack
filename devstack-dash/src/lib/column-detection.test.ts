// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from 'vitest'
import {
  detectColumns,
  loadColumnConfig,
  mergeColumnConfig,
  saveColumnConfig,
  type ColumnConfig,
} from './column-detection'

const entries: Array<{ attributes: Record<string, string> }> = [
  {
    attributes: {
      event: 'request_started',
      method: 'GET',
      status_code: '200',
      env: 'prod',
      request_id: '3f5d8d70-6c95-4d2e-8d29-51903334c101',
      trace_id: '3b7ad9bcf9d64cb1bd18f3c0c6d8630d',
    },
  },
  {
    attributes: {
      event: 'request_finished',
      method: 'POST',
      status_code: '500',
      env: 'prod',
      request_id: '49b6bf97-32ff-4f91-a611-f1417774931e',
      trace_id: '7497d7cfed4046cfa8b9a0dc186423a7',
    },
  },
  {
    attributes: {
      event: 'request_finished',
      method: 'GET',
      status_code: '200',
      env: 'prod',
      request_id: '1966ff30-92a4-40fe-b27c-70d54b3e3f6a',
      trace_id: '1b1cc573464143df906a2f64c2ae57dc',
      user: 'dana',
    },
  },
]

describe('detectColumns', () => {
  beforeEach(() => {
    window.localStorage.clear()
  })

  it('returns built-in columns plus ranked dynamic columns', () => {
    const columns = detectColumns(entries)
    const dynamicColumns = columns.filter((column) => !column.builtIn)

    expect(columns.slice(0, 4).map((column) => column.field)).toEqual([
      'timestamp',
      'service',
      'level',
      'message',
    ])
    expect(dynamicColumns.map((column) => column.field).slice(0, 4)).toEqual([
      'event',
      'method',
      'status_code',
      'request_id',
    ])
    expect(dynamicColumns[0]?.label).toBe('event')
    expect(dynamicColumns[2]?.label).toBe('status code')
  })

  it('auto-selects the top three useful dynamic columns and hides single-value fields', () => {
    const columns = detectColumns(entries)
    const byField = new Map(columns.map((column) => [column.field, column]))

    expect(byField.get('event')?.visible).toBe(true)
    expect(byField.get('method')?.visible).toBe(true)
    expect(byField.get('status_code')?.visible).toBe(true)
    expect(byField.get('env')?.visible).toBe(false)
  })

  it('deprioritizes id-like fields behind more readable columns', () => {
    const columns = detectColumns(entries)
    const dynamicFields = columns
      .filter((column) => !column.builtIn)
      .map((column) => column.field)

    expect(dynamicFields.indexOf('request_id')).toBeGreaterThan(
      dynamicFields.indexOf('status_code'),
    )
    expect(dynamicFields.indexOf('trace_id')).toBeGreaterThan(
      dynamicFields.indexOf('method'),
    )
  })

  it('merges saved config so user visibility and width take precedence', () => {
    const detected = detectColumns(entries)
    const saved: ColumnConfig[] = [
      {
        field: 'method',
        label: 'HTTP method',
        width: 180,
        visible: false,
        builtIn: false,
      },
      {
        field: 'timestamp',
        label: 'Time',
        width: 140,
        visible: true,
        builtIn: true,
      },
    ]

    const merged = mergeColumnConfig(detected, saved)
    const byField = new Map(merged.map((column) => [column.field, column]))

    expect(merged.slice(0, 2).map((column) => column.field)).toEqual([
      'method',
      'timestamp',
    ])
    expect(byField.get('method')).toMatchObject({
      label: 'HTTP method',
      width: 180,
      visible: false,
      builtIn: false,
    })
    expect(byField.get('timestamp')).toMatchObject({
      label: 'Time',
      width: 140,
      visible: true,
      builtIn: true,
    })
    expect(byField.has('status_code')).toBe(true)
  })
})

describe('column config persistence', () => {
  beforeEach(() => {
    window.localStorage.clear()
  })

  it('saves and loads config from localStorage', () => {
    const config = detectColumns(entries)

    saveColumnConfig('devstack:columns:test-project', config)

    expect(loadColumnConfig('devstack:columns:test-project')).toEqual(config)
  })

  it('returns null when storage is missing or malformed', () => {
    expect(loadColumnConfig('devstack:columns:missing')).toBeNull()

    window.localStorage.setItem('devstack:columns:broken', '{not-json')
    expect(loadColumnConfig('devstack:columns:broken')).toBeNull()

    window.localStorage.setItem(
      'devstack:columns:wrong-shape',
      JSON.stringify([{ field: 'event' }]),
    )
    expect(loadColumnConfig('devstack:columns:wrong-shape')).toBeNull()
  })
})
