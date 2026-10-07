import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Pencil, Plus, Send, Trash2, Webhook as WebhookIcon } from 'lucide-react'
import { useState } from 'react'

import { webhooksQuery } from '../accounts'
import {
  changeWebhook,
  createWebhook,
  removeWebhook,
  testWebhook,
  type Happening,
  type Webhook,
  type WebhookSettings,
} from '../api/client'
import { Button, Confirm, Field, PageBar, Problem } from '../components/ui'
import { when } from '../format'
import { WORDS, groupOf } from '../happened'

/** A webhook being made, or changed. `id` is null for one that is not made yet. */
type Draft = { id: number | null; name: string; url: string; everything: boolean; events: string[] }

/** What can be told, under the headings it is listed under, in the order it is given in. */
function grouped(events: Happening[]): { group: string; names: string[] }[] {
  const groups: { group: string; names: string[] }[] = []
  for (const { name } of events) {
    const group = groupOf(name)
    const had = groups.find((one) => one.group === group)
    if (had) had.names.push(name)
    else groups.push({ group, names: [name] })
  }
  return groups
}

/** What a webhook is told of, in a line: "Servers 4 · VPS 2". */
function told(webhook: Webhook): string {
  if (webhook.everything) return 'Told of everything on the Activity page.'
  const counts = new Map<string, number>()
  for (const name of webhook.events) counts.set(groupOf(name), (counts.get(groupOf(name)) ?? 0) + 1)
  return `Told of ${[...counts].map(([group, count]) => `${group} ${count}`).join(' · ')}.`
}

/**
 * Where Homewarp tells what happens to it: addresses that take a message, as a
 * Discord or a Slack webhook does, each told of what was chosen for it. An
 * address is a secret of the site's making, so what is kept is not shown whole.
 */
export function WebhooksPage() {
  const queryClient = useQueryClient()
  const {
    data: { webhooks, events },
  } = useSuspenseQuery(webhooksQuery)
  const [draft, setDraft] = useState<Draft | null>(null)
  const [doomed, setDoomed] = useState<Webhook | null>(null)
  const again = () => queryClient.invalidateQueries({ queryKey: webhooksQuery.queryKey })

  // A new one starts from what a webhook is most often wanted for: what happens with nobody here.
  const fresh = (): Draft => ({
    id: null,
    name: '',
    url: '',
    everything: false,
    events: events.filter((one) => one.by_itself).map((one) => one.name),
  })
  const settingsOf = ({ name, url, everything, events }: Draft): WebhookSettings => ({ name, url, everything, events })

  const saving = useMutation({
    mutationFn: (from: Draft) => (from.id === null ? createWebhook(settingsOf(from)) : changeWebhook(from.id, settingsOf(from))),
    onSuccess: async () => {
      await again()
      setDraft(null)
    },
  })
  // How it went is kept either way, and shown beside the webhook.
  const testing = useMutation({ mutationFn: testWebhook, onSettled: again })
  const removing = useMutation({
    mutationFn: removeWebhook,
    onSuccess: async () => {
      await again()
      setDoomed(null)
    },
  })

  return (
    <>
      <PageBar title="Webhooks">
        {!draft && (
          <Button variant="primary" onClick={() => setDraft(fresh())}>
            <Plus aria-hidden size={16} />
            New webhook
          </Button>
        )}
      </PageBar>
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <p className="max-w-140 text-small text-ink-subtle">
          Homewarp can tell you what happens while you are not here: a server that crashed, one that was put to sleep or woken
          by a player, what a schedule did, a VPS that stopped answering. Give it the address of a webhook, as Discord, Slack
          and their like make them for a channel, and choose what that address is told of.
        </p>

        {draft && (
          <WebhookForm
            draft={draft}
            had={webhooks.find((one) => one.id === draft.id)}
            events={events}
            onChange={setDraft}
            busy={saving.isPending}
            problem={saving.error?.message}
            onSave={() => saving.mutate(draft)}
            onCancel={() => {
              setDraft(null)
              saving.reset()
            }}
          />
        )}

        {webhooks.length === 0 ? (
          !draft && (
            <div className="flex flex-1 flex-col items-center justify-center rounded-lg border border-dashed border-hairline-strong py-16 text-center">
              <WebhookIcon aria-hidden size={32} className="mb-3 text-ink-faint" />
              <h2 className="text-section text-ink">Nobody is told anything.</h2>
              <p className="mt-1">Have a channel of yours told when a server crashes, or a VPS stops answering.</p>
            </div>
          )
        ) : (
          <ul className="flex flex-col gap-4">
            {webhooks.map((webhook) => (
              <li key={webhook.id} className="rounded-lg border border-hairline bg-surface-1 p-4">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <h2 className="min-w-0 truncate text-body font-medium text-ink">{webhook.name}</h2>
                  <code className="rounded-sm bg-surface-2 px-1.5 font-mono text-mono text-ink-muted">{webhook.address}</code>
                  <span className="ml-auto flex gap-1">
                    <Button
                      variant="ghost"
                      title="Send one now"
                      busy={testing.isPending && testing.variables === webhook.id}
                      disabled={testing.isPending}
                      onClick={() => testing.mutate(webhook.id)}
                    >
                      <Send aria-hidden size={16} />
                      <span className="sr-only wide:not-sr-only">Send one now</span>
                    </Button>
                    <Button
                      variant="ghost"
                      title="Change"
                      onClick={() => {
                        saving.reset()
                        setDraft({ id: webhook.id, name: webhook.name, url: '', everything: webhook.everything, events: webhook.events })
                      }}
                    >
                      <Pencil aria-hidden size={16} />
                      <span className="sr-only wide:not-sr-only">Change</span>
                    </Button>
                    <Button variant="ghost" title="Remove" onClick={() => setDoomed(webhook)}>
                      <Trash2 aria-hidden size={16} />
                      <span className="sr-only wide:not-sr-only">Remove</span>
                    </Button>
                  </span>
                </div>
                <p className="mt-1 text-small">{told(webhook)}</p>
                <p role="status" className="mt-1 text-small text-ink-subtle">
                  {!webhook.last
                    ? 'Nothing has been sent to it yet.'
                    : webhook.last.problem
                      ? `The last one, ${when(webhook.last.at)}, was not taken: ${webhook.last.problem}`
                      : `The last one was taken ${when(webhook.last.at)}.`}
                </p>
              </li>
            ))}
          </ul>
        )}
      </main>

      <Confirm
        open={doomed !== null}
        onClose={() => {
          setDoomed(null)
          removing.reset()
        }}
        title={`Remove ${doomed?.name ?? 'this webhook'}?`}
        action={
          <Button variant="danger" busy={removing.isPending} onClick={() => doomed && removing.mutate(doomed.id)}>
            Remove
          </Button>
        }
      >
        <p>Homewarp tells that address nothing more. What it was told before stays where it was sent.</p>
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
  /** The webhook being changed, as it is kept. None for one that is being made. */
  had: Webhook | undefined
  events: Happening[]
  onChange: (draft: Draft) => void
  busy: boolean
  problem: string | undefined
  onSave: () => void
  onCancel: () => void
}

/** A webhook's name, the address it is told at, and what it is told of. */
function WebhookForm({ draft, had, events, onChange, busy, problem, onSave, onCancel }: FormProps) {
  const chosen = new Set(draft.events)
  // In the order they are listed in, whatever order they were ticked in.
  const choose = (names: string[], on: boolean) =>
    onChange({ ...draft, events: events.map((one) => one.name).filter((name) => (names.includes(name) ? on : chosen.has(name))) })

  return (
    <form
      className="flex flex-col gap-4 rounded-lg border border-hairline bg-surface-1 p-4"
      onSubmit={(event) => {
        event.preventDefault()
        onSave()
      }}
    >
      <div className="flex max-w-140 flex-col gap-4">
        <Field
          label="Name"
          value={draft.name}
          onChange={(event) => onChange({ ...draft, name: event.currentTarget.value })}
          required
          maxLength={60}
          autoFocus
          autoComplete="off"
          placeholder="Discord"
        />
        <Field
          label={had ? 'Another address' : 'Address to tell'}
          value={draft.url}
          onChange={(event) => onChange({ ...draft, url: event.currentTarget.value })}
          mono
          required={!had}
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          placeholder="https://discord.com/api/webhooks/…"
          hint={
            had
              ? `Left empty, it stays ${had.address}. Whoever has an address can post there, so it is kept and not shown again.`
              : 'It has to begin with https:// and be on the internet: Homewarp sends nothing into a home network. Whoever has it can post there, so it is kept and not shown again.'
          }
        />
        <label className="flex items-start gap-2">
          <input
            type="checkbox"
            checked={draft.everything}
            onChange={(event) => onChange({ ...draft, everything: event.currentTarget.checked })}
            className="mt-0.5 size-4 accent-accent"
          />
          <span>
            Everything on the Activity page
            <span className="block text-small text-ink-subtle">
              Every sign-in, start, stop and change, as it is done, and whatever a later Homewarp writes down there.
            </span>
          </span>
        </label>
      </div>

      {!draft.everything && (
        <fieldset className="flex flex-col gap-4">
          <legend className="mb-3 text-caption text-ink-subtle">What it is told of</legend>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="ghost"
              onClick={() => onChange({ ...draft, events: events.filter((one) => one.by_itself).map((one) => one.name) })}
            >
              What happens by itself
            </Button>
            <Button variant="ghost" onClick={() => onChange({ ...draft, events: events.map((one) => one.name) })}>
              All of it
            </Button>
            <Button variant="ghost" onClick={() => onChange({ ...draft, events: [] })}>
              None of it
            </Button>
          </div>
          {grouped(events).map(({ group, names }) => {
            const all = names.every((name) => chosen.has(name))
            return (
              <div key={group} className="flex flex-col gap-2">
                <div className="flex items-center gap-2">
                  <h3 className="text-body font-medium text-ink">{group}</h3>
                  <Button variant="ghost" aria-label={`${all ? 'None' : 'All'} of ${group}`} onClick={() => choose(names, !all)}>
                    {all ? 'None' : 'All'}
                  </Button>
                </div>
                <div className="grid gap-x-6 gap-y-2 md:grid-cols-2 wide:grid-cols-3">
                  {names.map((name) => (
                    <label key={name} className="flex items-start gap-2">
                      <input
                        type="checkbox"
                        checked={chosen.has(name)}
                        onChange={(event) => choose([name], event.currentTarget.checked)}
                        className="mt-0.5 size-4 shrink-0 accent-accent"
                      />
                      {WORDS[name] ?? name}
                    </label>
                  ))}
                </div>
              </div>
            )
          })}
        </fieldset>
      )}

      {problem && <Problem>{problem}</Problem>}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" busy={busy}>
          {draft.id === null ? 'Create webhook' : 'Save'}
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  )
}
