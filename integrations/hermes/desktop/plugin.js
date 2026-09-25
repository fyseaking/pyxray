/**
 * pyxray — the desktop half.
 *
 * A pane under the conversation that shows what the Python Hermes ran was
 * going to do: one row per snippet, newest first, coloured by band, with the
 * full card for whichever row you click. Everything comes through the plugin's
 * own backend (../dashboard/plugin_api.py) via ctx.rest, so nothing here
 * touches the filesystem or the feed directly.
 *
 * Plain ESM, loaded uncompiled — jsx()/jsxs() only, no JSX syntax, and only
 * the three importable specifiers.
 */

import {
  Badge,
  Button,
  EmptyState,
  Tip,
  haptic,
  useQuery,
  useQueryClient
} from '@hermes/plugin-sdk'
import { jsx, jsxs } from 'react/jsx-runtime'
import { useState } from 'react'

const ID = 'pyxray'
const POLL_MS = 2500

// Band → theme variable. Never a literal colour; the pane reskins with the app.
const BAND = {
  inert: { word: 'inert', color: 'var(--ui-text-quaternary)' },
  routine: { word: 'routine', color: 'var(--ui-success, var(--ui-accent))' },
  check: { word: 'check it', color: 'var(--ui-warning, var(--ui-accent))' },
  read: { word: 'read it first', color: 'var(--ui-danger, var(--ui-accent))' }
}

function bandOf (event) {
  return BAND[event && event.band] || BAND.inert
}

function clock (ts) {
  if (!ts) return ''
  const d = new Date(ts)
  const p = n => String(n).padStart(2, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

function Dot ({ color }) {
  return jsx('span', {
    'aria-hidden': true,
    style: {
      display: 'inline-block',
      width: 7,
      height: 7,
      borderRadius: 999,
      background: color,
      flexShrink: 0
    }
  })
}

function ObservedChip ({ items }) {
  if (!items || !items.length) return null
  const label = items
    .slice(0, 8)
    .map(i => `${i.verb} ${i.target || ''}`.trim() + (i.count > 1 ? ` x${i.count}` : ''))
    .join('\n')
  return jsx(Tip, {
    label: `what the run actually did:\n${label}`,
    children: jsx('span', {
      className: 'shrink-0 rounded px-1 text-[0.625rem]',
      style: { color: 'var(--ui-text-secondary)', border: '1px solid var(--ui-stroke-secondary)' },
      children: `observed ${items.length}`
    })
  })
}

function Row ({ row, active, onPick, observed }) {
  const ev = row.event || {}
  const band = bandOf(ev)
  const notes = Array.isArray(ev.notes) ? ev.notes : []
  const title = notes
    .slice(0, 4)
    .map(n => [n.verb, n.target].filter(Boolean).join(' ') + (n.line ? ` (line ${n.line})` : ''))
    .join('\n')
  return jsx(Tip, {
    label: title || ev.synopsis || row.label || '',
    children: jsxs('button', {
      type: 'button',
      onClick: () => { haptic('tap'); onPick(row) },
      className: 'flex w-full items-center gap-2 rounded px-1.5 py-1 text-left',
      style: {
        background: active ? 'var(--ui-canvas-elevated)' : 'transparent',
        color: 'var(--ui-text-primary)'
      },
      children: [
        jsx(Dot, { color: band.color }),
        jsx('span', {
          className: 'shrink-0 tabular-nums text-[0.6875rem]',
          style: { color: 'var(--ui-text-tertiary)', minWidth: 54 },
          children: clock(row.ts || ev.ts)
        }),
        jsx('span', {
          className: 'shrink-0 text-[0.6875rem]',
          style: { color: band.color, minWidth: 74 },
          children: band.word
        }),
        jsx('span', {
          className: 'shrink-0 tabular-nums text-[0.6875rem]',
          style: { color: 'var(--ui-text-secondary)', minWidth: 22, textAlign: 'right' },
          children: String(ev.risk == null ? '' : ev.risk)
        }),
        jsx('span', {
          className: 'min-w-0 flex-1 truncate text-[0.75rem]',
          children: ev.synopsis || '—'
        }),
        jsx(ObservedChip, { items: ev.run && observed ? observed[ev.run] : null }),
        row.tool
          ? jsx('span', {
              className: 'shrink-0 text-[0.625rem]',
              style: { color: 'var(--ui-text-quaternary)' },
              children: row.tool
            })
          : null
      ]
    })
  })
}

function Card ({ ctx, row }) {
  const q = useQuery({
    queryKey: [ctx.source, 'card', row && row.card],
    enabled: !!(row && row.card),
    queryFn: () => ctx.rest(`/card/${encodeURIComponent(row.card)}`)
  })
  if (!row || !row.card) return null
  const html = (q.data && q.data.html) || ''
  if (q.isLoading) {
    return jsx('div', {
      className: 'p-2 text-[0.6875rem]',
      style: { color: 'var(--ui-text-tertiary)' },
      children: 'rendering…'
    })
  }
  if (!html) {
    return jsx('div', {
      className: 'p-2 text-[0.6875rem]',
      style: { color: 'var(--ui-text-tertiary)' },
      children: 'card not available (the backend half may be off)'
    })
  }
  return jsx('iframe', {
    title: `pyxray card ${row.label || ''}`,
    sandbox: '',
    srcDoc: html,
    style: {
      width: '100%',
      height: 260,
      border: '1px solid var(--ui-stroke-secondary)',
      borderRadius: 6,
      background: 'transparent'
    }
  })
}

function GateChip ({ ctx, status }) {
  const qc = useQueryClient()
  const gate = status && status.gate
  const envGate = status && status.env_gate
  const label = gate == null ? 'gate off' : `gate ${gate} → ${status.mode}`
  const cycle = async () => {
    haptic('tap')
    // off → 40 → 60 → off. The precise number lives in config.yaml; this is a
    // quick toggle, not a settings page.
    const next = gate == null ? 40 : gate < 60 ? 60 : 'off'
    try {
      await ctx.rest('/gate', { method: 'POST', body: { gate: next } })
    } catch (_) { /* the backend half is off; the chip stays as it was */ }
    qc.invalidateQueries({ queryKey: [ctx.source, 'status'] })
  }
  return jsx(Tip, {
    label: envGate != null
      ? `PYXRAY_GATE=${envGate} in the environment overrides the config value`
      : 'Anything over the gate goes to the approval prompt. Click to cycle off → 40 → 60.',
    children: jsx(Button, { variant: 'secondary', size: 'sm', onClick: cycle, children: label })
  })
}

function Pane ({ ctx }) {
  const [picked, setPicked] = useState(null)
  const [scope, setScope] = useState(() => ctx.storage.get('scope', 'hermes'))
  const status = useQuery({
    queryKey: [ctx.source, 'status'],
    queryFn: () => ctx.rest('/status'),
    refetchInterval: POLL_MS * 4
  })
  const rows = useQuery({
    queryKey: [ctx.source, 'rows', scope],
    queryFn: () => scope === 'hermes'
      ? ctx.rest('/recent?limit=60')
      : ctx.rest('/events?limit=80'),
    refetchInterval: POLL_MS
  })
  // Audited runs (the shim with PYXRAY_AUDIT=1) in the whole-feed view.
  const observedQ = useQuery({
    queryKey: [ctx.source, 'observed'],
    queryFn: () => ctx.rest('/observed'),
    enabled: scope !== 'hermes',
    refetchInterval: POLL_MS * 2
  })
  const observed = (observedQ.data && observedQ.data.runs) || {}
  const list = (rows.data && rows.data.rows) || []
  // Feed rows are events themselves; recent rows wrap one under `event`.
  const items = scope === 'hermes'
    ? list
    : list.map(ev => ({ ts: ev.ts, tool: ev.source, label: ev.name, event: ev }))
  const engine = status.data && status.data.engine
  const flip = () => {
    const next = scope === 'hermes' ? 'all' : 'hermes'
    ctx.storage.set('scope', next)
    setScope(next)
    setPicked(null)
  }

  return jsxs('div', {
    className: 'flex h-full flex-col gap-1 p-1.5',
    children: [
      jsxs('div', {
        className: 'flex items-center gap-1.5',
        children: [
          jsx('span', {
            className: 'text-[0.75rem] font-medium',
            children: 'pyxray'
          }),
          engine && !engine.ok
            ? jsx(Badge, { variant: 'destructive', children: 'engine missing' })
            : engine
              ? jsx(Badge, { variant: 'secondary', children: engine.backend })
              : null,
          jsx('span', { className: 'flex-1' }),
          jsx(Tip, {
            label: scope === 'hermes'
              ? 'Showing Hermes calls (with cards). Click for the whole feed — Claude Code, the shim, everything.'
              : 'Showing the whole pyxray feed. Click for Hermes calls only.',
            children: jsx(Button, {
              variant: 'ghost',
              size: 'sm',
              onClick: flip,
              children: scope === 'hermes' ? 'hermes' : 'all'
            })
          }),
          status.data ? jsx(GateChip, { ctx, status: status.data }) : null
        ]
      }),
      engine && !engine.ok
        ? jsx('div', {
            className: 'rounded px-2 py-1 text-[0.6875rem]',
            style: { color: 'var(--ui-text-secondary)', background: 'var(--ui-canvas-elevated)' },
            children: engine.error
          })
        : null,
      jsx('div', {
        className: 'min-h-0 flex-1 overflow-auto',
        children: items.length
          ? items.map((row, i) => jsx(Row, {
              row,
              active: picked === row,
              observed,
              onPick: r => setPicked(picked === r ? null : r)
            }, `${row.ts}-${i}`))
          : jsx(EmptyState, {
              title: rows.isLoading ? 'loading…' : 'nothing yet',
              description: rows.isError
                ? 'the backend half is not answering — is pyxray in plugins.enabled?'
                : 'the next terminal or execute_code call with Python in it lands here'
            })
      }),
      picked && scope === 'hermes' ? jsx(Card, { ctx, row: picked }) : null
    ]
  })
}

export default {
  id: ID,
  name: 'pyxray',
  defaultEnabled: true,
  register (ctx) {
    ctx.register({
      id: 'pane',
      area: 'panes',
      title: 'pyxray',
      data: {
        placement: 'bottom',
        dock: { pane: 'workspace', pos: 'bottom' },
        height: '220px'
      },
      render: () => jsx(Pane, { ctx })
    })
  }
}
