import { useState } from 'react'

import { rate } from '../format'

/** What went one way and the other in a second, and when: in milliseconds, as the clock counts. */
export type Sample = { at: number; received: number; sent: number }

/** The least the scale reaches to. Under it, a few stray packets would fill the whole height. */
const LEAST = 4096
const HEIGHT = 100

const CLOCK = new Intl.DateTimeFormat(undefined, { timeStyle: 'medium' })

/**
 * Traffic as it is now and as it was a little while ago: two lines over one
 * scale, the newest at the right. The first is the accent's, with its area
 * filled; the second is neutral and dashed, so that the two are told apart by
 * more than colour (DESIGN.md: one accent, and colour means state).
 *
 * `slots` is how many samples the width holds. Fewer than that leave the left
 * of it empty, as a chart that has only just begun to be drawn should.
 */
export function TrafficChart({
  samples,
  slots,
  labels,
  compact = false,
}: {
  samples: Sample[]
  slots: number
  labels: [string, string]
  compact?: boolean
}) {
  const [pointed, setPointed] = useState<number | null>(null)
  const shown = samples.slice(-slots)
  const first = slots - shown.length
  const most = shown.reduce((most, sample) => Math.max(most, sample.received, sample.sent), 0)
  let top = LEAST
  while (top < most) top *= 2

  const x = (index: number) => first + index
  const y = (value: number) => HEIGHT - (value / top) * HEIGHT
  const line = (of: (sample: Sample) => number) =>
    shown.map((sample, index) => `${index === 0 ? 'M' : 'L'}${x(index)} ${y(of(sample)).toFixed(2)}`).join('')
  const received = line((sample) => sample.received)
  const sent = line((sample) => sample.sent)
  const last = shown.at(-1)
  const at = pointed === null ? undefined : shown[pointed]
  const told = at ?? last

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
        {[
          { label: labels[0], value: told?.received, dashed: false },
          { label: labels[1], value: told?.sent, dashed: true },
        ].map(({ label, value, dashed }) => (
          <span key={label} className="flex items-baseline gap-1.5">
            <svg aria-hidden viewBox="0 0 14 4" className={`h-1 w-3.5 self-center ${dashed ? 'text-ink-subtle' : 'text-accent'}`}>
              <line x1="0" y1="2" x2="14" y2="2" stroke="currentColor" strokeWidth="2" strokeDasharray={dashed ? '4 2' : undefined} />
            </svg>
            <span className="text-small text-ink-subtle">{label}</span>
            <span className="font-medium text-ink tabular-nums">{value === undefined ? '—' : rate(value)}</span>
          </span>
        ))}
        {/* Whose moment the figures are, while one is pointed at. */}
        {at && <span className="ml-auto text-small text-ink-subtle tabular-nums">{CLOCK.format(at.at)}</span>}
      </div>
      <div
        role="img"
        aria-label={
          last ? `${labels[0]} ${rate(last.received)}, ${labels[1]} ${rate(last.sent)}, now` : 'Nothing measured yet'
        }
        className={`relative ${compact ? 'h-12' : 'h-40'}`}
        onPointerMove={(event) => {
          const box = event.currentTarget.getBoundingClientRect()
          const index = Math.round(((event.clientX - box.left) / box.width) * (slots - 1)) - first
          setPointed(index >= 0 && index < shown.length ? index : null)
        }}
        onPointerLeave={() => setPointed(null)}
      >
        <svg viewBox={`0 0 ${slots - 1} ${HEIGHT}`} preserveAspectRatio="none" className="absolute inset-0 size-full overflow-visible">
          {/* The scale: its top, its middle and the ground it stands on. */}
          {(compact ? [HEIGHT] : [0, HEIGHT / 2, HEIGHT]).map((level) => (
            <line
              key={level}
              x1="0"
              x2={slots - 1}
              y1={level}
              y2={level}
              className="stroke-hairline"
              strokeWidth="1"
              vectorEffect="non-scaling-stroke"
            />
          ))}
          {shown.length > 1 && (
            <>
              <path d={`${received}L${x(shown.length - 1)} ${HEIGHT}L${x(0)} ${HEIGHT}Z`} className="fill-accent-soft" />
              <path
                d={sent}
                fill="none"
                className="stroke-ink-subtle"
                strokeWidth={compact ? 1.5 : 2}
                strokeDasharray="4 3"
                strokeLinejoin="round"
                vectorEffect="non-scaling-stroke"
              />
              <path
                d={received}
                fill="none"
                className="stroke-accent"
                strokeWidth={compact ? 1.5 : 2}
                strokeLinejoin="round"
                vectorEffect="non-scaling-stroke"
              />
            </>
          )}
          {pointed !== null && (
            <line
              x1={x(pointed)}
              x2={x(pointed)}
              y1="0"
              y2={HEIGHT}
              className="stroke-hairline-strong"
              strokeWidth="1"
              vectorEffect="non-scaling-stroke"
            />
          )}
        </svg>
        {!compact && (
          <>
            <span className="absolute top-0.5 left-0 text-caption text-ink-subtle tabular-nums">{rate(top)}</span>
            <span className="absolute top-1/2 left-0 mt-0.5 text-caption text-ink-subtle tabular-nums">{rate(top / 2)}</span>
          </>
        )}
      </div>
    </div>
  )
}
