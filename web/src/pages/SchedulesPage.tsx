import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { getRouteApi } from '@tanstack/react-router'
import { CalendarClock, Pencil, Play, Plus, Trash2, X } from 'lucide-react'
import { useState } from 'react'

import {
  changeSchedule,
  createSchedule,
  removeSchedule,
  runSchedule,
  type Schedule,
  type ScheduleSettings,
  type TaskAction,
} from '../api/client'
import { Button, Confirm, Field, Problem, Select } from '../components/ui'
import { when } from '../format'
import { schedulesQuery } from '../servers'

const route = getRouteApi('/shell/servers/$serverId')

/** How often the list is asked for again, in milliseconds: a schedule runs by itself, and then has something new to say. */
const EVERY = 10_000

const ACTIONS: { action: TaskAction; label: string; short: string }[] = [
  { action: 'command', label: 'Type a command', short: 'type' },
  { action: 'backup', label: 'Make a backup', short: 'make a backup' },
  { action: 'restart', label: 'Restart the server', short: 'restart' },
  { action: 'start', label: 'Start the server', short: 'start' },
  { action: 'stop', label: 'Stop the server', short: 'stop' },
  { action: 'kill', label: 'Kill the server', short: 'kill' },
]

/** Times most schedules run at, for whoever would rather not write cron. */
const TIMES = [
  { label: 'Every night at 4', cron: '0 4 * * *' },
  { label: 'Every six hours', cron: '0 */6 * * *' },
  { label: 'Every half hour', cron: '*/30 * * * *' },
  { label: 'Sundays at 5', cron: '0 5 * * 0' },
]

/** One step as the form holds it: every field there, whatever the step is. */
type Step = { action: TaskAction; command: string; wait: number }
/** A schedule being made, or changed. `id` is null for one that is not made yet. */
type Draft = { id: number | null; name: string; cron: string; enabled: boolean; onlyRunning: boolean; steps: Step[] }

const NEW: Draft = { id: null, name: '', cron: '0 4 * * *', enabled: true, onlyRunning: false, steps: [{ action: 'backup', command: '', wait: 0 }] }

/** How far this device's clock is from UTC, in minutes east: the clock a schedule's times are read on. */
function clock(): { offset: number; label: string } {
  const offset = -new Date().getTimezoneOffset()
  const [hours, minutes] = [Math.floor(Math.abs(offset) / 60), Math.abs(offset) % 60]
  return { offset, label: `UTC${offset < 0 ? '−' : '+'}${hours}${minutes ? `:${String(minutes).padStart(2, '0')}` : ''}` }
}

/** What a schedule does, in a line: "type save-all, then make a backup". */
function told(schedule: Schedule): string {
  return schedule.tasks
    .map((task) => {
      const { short } = ACTIONS.find((one) => one.action === task.action) ?? { short: task.action }
      return task.action === 'command' ? `${short} ${task.command ?? ''}` : short
    })
    .join(', then ')
}

/** What a server does by the clock (DESIGN.md, Inside a server): commands, restarts and backups at set times. */
export function SchedulesTab() {
  const { serverId } = route.useParams()
  const id = Number(serverId)
  const queryClient = useQueryClient()
  const { data: schedules } = useSuspenseQuery({ ...schedulesQuery(id), refetchInterval: EVERY })
  const [draft, setDraft] = useState<Draft | null>(null)
  const [doomed, setDoomed] = useState<Schedule | null>(null)
  const again = () => queryClient.invalidateQueries({ queryKey: schedulesQuery(id).queryKey })

  const settingsOf = (from: Draft): ScheduleSettings => ({
    name: from.name,
    cron: from.cron,
    utc_offset: clock().offset,
    enabled: from.enabled,
    only_running: from.onlyRunning,
    tasks: from.steps.map((step) => ({
      action: step.action,
      command: step.action === 'command' ? step.command : '',
      wait_seconds: step.wait,
    })),
  })
  const draftOf = (schedule: Schedule): Draft => ({
    id: schedule.id,
    name: schedule.name,
    cron: schedule.cron,
    enabled: schedule.enabled,
    onlyRunning: schedule.only_running,
    steps: schedule.tasks.map((task) => ({ action: task.action, command: task.command ?? '', wait: task.wait_seconds ?? 0 })),
  })

  const saving = useMutation({
    mutationFn: (from: Draft) => (from.id === null ? createSchedule(id, settingsOf(from)) : changeSchedule(id, from.id, settingsOf(from))),
    onSuccess: async () => {
      await again()
      setDraft(null)
    },
  })
  const running = useMutation({
    mutationFn: (schedule: number) => runSchedule(id, schedule),
    // It is set off, not yet done: what it came to is there a moment later.
    onSuccess: () => void window.setTimeout(() => void again(), 1_500),
  })
  const removing = useMutation({
    mutationFn: (schedule: number) => removeSchedule(id, schedule),
    onSuccess: async () => {
      await again()
      setDoomed(null)
    },
  })

  return (
    <>
      {draft ? (
        <ScheduleForm
          draft={draft}
          onChange={setDraft}
          busy={saving.isPending}
          problem={saving.error?.message}
          onSave={() => saving.mutate(draft)}
          onCancel={() => {
            setDraft(null)
            saving.reset()
          }}
        />
      ) : (
        <div>
          <Button variant={schedules.length === 0 ? 'primary' : 'secondary'} onClick={() => setDraft(NEW)}>
            <Plus aria-hidden size={16} />
            New schedule
          </Button>
        </div>
      )}
      {running.error && <Problem>{running.error.message}</Problem>}

      {schedules.length === 0 ? (
        !draft && (
          <div className="flex flex-1 flex-col items-center justify-center rounded-lg border border-dashed border-hairline-strong py-16 text-center">
            <CalendarClock aria-hidden size={32} className="mb-3 text-ink-faint" />
            <h2 className="text-section text-ink">Nothing is scheduled.</h2>
            <p className="mt-1">Have the server back itself up every night, or restart every morning.</p>
          </div>
        )
      ) : (
        <ul className="flex flex-col gap-4">
          {schedules.map((schedule) => (
            <li key={schedule.id} className="rounded-lg border border-hairline bg-surface-1 p-4">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <h2 className="min-w-0 truncate text-body font-medium text-ink">{schedule.name}</h2>
                <code className="rounded-sm bg-surface-2 px-1.5 font-mono text-mono text-ink-muted">{schedule.cron}</code>
                {!schedule.enabled && <span className="text-caption text-ink-subtle">Off</span>}
                <span className="ml-auto flex gap-1">
                  <Button variant="ghost" title="Run now" disabled={running.isPending} onClick={() => running.mutate(schedule.id)}>
                    <Play aria-hidden size={16} />
                    <span className="sr-only wide:not-sr-only">Run now</span>
                  </Button>
                  <Button variant="ghost" title="Change" onClick={() => setDraft(draftOf(schedule))}>
                    <Pencil aria-hidden size={16} />
                    <span className="sr-only wide:not-sr-only">Change</span>
                  </Button>
                  <Button variant="ghost" title="Remove" onClick={() => setDoomed(schedule)}>
                    <Trash2 aria-hidden size={16} />
                    <span className="sr-only wide:not-sr-only">Remove</span>
                  </Button>
                </span>
              </div>
              <p className="mt-1 text-small first-letter:uppercase">{told(schedule)}.</p>
              <p className="mt-1 text-small text-ink-subtle">
                {typeof schedule.next_run_at === 'number' ? `Next ${when(schedule.next_run_at)}` : 'It runs only when set off by hand'}
                {schedule.only_running && ' · only while the server is running'}
                {typeof schedule.last_run_at === 'number' && ` · last ${when(schedule.last_run_at)}: ${schedule.last_result ?? ''}`}
              </p>
            </li>
          ))}
        </ul>
      )}

      <Confirm
        open={doomed !== null}
        onClose={() => {
          setDoomed(null)
          removing.reset()
        }}
        title={`Remove ${doomed?.name ?? 'this schedule'}?`}
        action={
          <Button variant="danger" busy={removing.isPending} onClick={() => doomed && removing.mutate(doomed.id)}>
            Remove
          </Button>
        }
      >
        <p>The server no longer does this by itself. What the schedule did before stays done.</p>
        {removing.error && (
          <div className="mt-3">
            <Problem>{removing.error.message}</Problem>
          </div>
        )}
      </Confirm>
    </>
  )
}

type FormProps = {
  draft: Draft
  onChange: (draft: Draft) => void
  busy: boolean
  problem: string | undefined
  onSave: () => void
  onCancel: () => void
}

/** A schedule's name, its times and what it does, step by step. */
function ScheduleForm({ draft, onChange, busy, problem, onSave, onCancel }: FormProps) {
  const step = (index: number, to: Partial<Step>) =>
    onChange({ ...draft, steps: draft.steps.map((had, at) => (at === index ? { ...had, ...to } : had)) })

  return (
    <form
      className="flex flex-col gap-4 rounded-lg border border-hairline bg-surface-1 p-4"
      onSubmit={(event) => {
        event.preventDefault()
        onSave()
      }}
    >
      <div className="grid gap-4 md:grid-cols-2">
        <Field
          label="Name"
          value={draft.name}
          onChange={(event) => onChange({ ...draft, name: event.currentTarget.value })}
          required
          maxLength={60}
          autoFocus
          autoComplete="off"
        />
        <Field
          label="When"
          mono
          value={draft.cron}
          onChange={(event) => onChange({ ...draft, cron: event.currentTarget.value })}
          required
          autoComplete="off"
          spellCheck={false}
          hint={`Minute, hour, day, month and weekday, as cron writes them, on this device's clock (${clock().label}).`}
        />
      </div>
      <div className="flex flex-wrap gap-2">
        {TIMES.map((time) => (
          <Button key={time.cron} variant="ghost" onClick={() => onChange({ ...draft, cron: time.cron })}>
            {time.label}
          </Button>
        ))}
      </div>

      <fieldset className="flex flex-col gap-3">
        <legend className="mb-3 text-caption text-ink-subtle">What it does, in this order</legend>
        {draft.steps.map((one, index) => (
          <div key={index} className="flex flex-wrap items-end gap-2">
            <Select label={`Step ${index + 1}`} value={one.action} onChange={(event) => step(index, { action: event.currentTarget.value as TaskAction })}>
              {ACTIONS.map(({ action, label }) => (
                <option key={action} value={action}>
                  {label}
                </option>
              ))}
            </Select>
            {one.action === 'command' && (
              <div className="min-w-0 flex-1 md:max-w-96">
                <Field
                  label="Command"
                  mono
                  value={one.command}
                  onChange={(event) => step(index, { command: event.currentTarget.value })}
                  required
                  maxLength={1000}
                  autoComplete="off"
                  spellCheck={false}
                />
              </div>
            )}
            <div className="w-32">
              <Field
                label="Wait first, seconds"
                type="number"
                min={0}
                max={3600}
                value={one.wait}
                onChange={(event) => step(index, { wait: Math.max(0, Math.round(Number(event.currentTarget.value) || 0)) })}
              />
            </div>
            {draft.steps.length > 1 && (
              <Button
                variant="ghost"
                title={`Take step ${index + 1} out`}
                onClick={() => onChange({ ...draft, steps: draft.steps.filter((_, at) => at !== index) })}
              >
                <X aria-hidden size={16} />
                <span className="sr-only">Take step {index + 1} out</span>
              </Button>
            )}
          </div>
        ))}
        {draft.steps.length < 10 && (
          <div>
            <Button variant="ghost" onClick={() => onChange({ ...draft, steps: [...draft.steps, { action: 'command', command: '', wait: 0 }] })}>
              <Plus aria-hidden size={16} />
              Add a step
            </Button>
          </div>
        )}
      </fieldset>

      <label className="flex items-center gap-2.5">
        <input
          type="checkbox"
          checked={draft.onlyRunning}
          onChange={(event) => onChange({ ...draft, onlyRunning: event.currentTarget.checked })}
          className="size-4 accent-accent"
        />
        Pass over a time at which the server is not running
      </label>
      <label className="flex items-center gap-2.5">
        <input
          type="checkbox"
          checked={draft.enabled}
          onChange={(event) => onChange({ ...draft, enabled: event.currentTarget.checked })}
          className="size-4 accent-accent"
        />
        Run by the clock. Without this it runs only when set off by hand.
      </label>

      {problem && <Problem>{problem}</Problem>}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" busy={busy}>
          {draft.id === null ? 'Create schedule' : 'Save'}
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  )
}
