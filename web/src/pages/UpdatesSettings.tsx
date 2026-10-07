import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Check, LoaderCircle } from 'lucide-react'
import { useEffect, useState, type ReactNode } from 'react'

import { updateQuery } from '../accounts'
import { changeUpdate, checkUpdate, installUpdate, type Update } from '../api/client'
import { Button, CopyChip, Problem, Select } from '../components/ui'
import { when } from '../format'
import { useNetwork } from '../gate'

type Step = NonNullable<Update['updating']>['step']

/** The steps of an update, in the order they are taken, each as it is said while it is being done. */
const STEPS: { step: Step; doing: string }[] = [
  { step: 'fetching', doing: 'Fetching the release, and checking it against Homewarp’s signature' },
  { step: 'backing_up', doing: 'Copying Homewarp’s database, and backing up what was asked for' },
  { step: 'starting_again', doing: 'Starting again as the new version' },
]

function Section({ title, lead, children }: { title: string; lead?: ReactNode; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-4">
      <div>
        <h2 className="text-section text-ink">{title}</h2>
        {lead && <p className="mt-1 max-w-140 text-small text-ink-subtle">{lead}</p>}
      </div>
      {children}
    </section>
  )
}

/**
 * Which version runs, whether a newer one is out, and putting it in place.
 * While an update is made the page asks every two seconds; Homewarp stops
 * answering for a moment near the end, and the page waits for it to answer
 * again, as whichever version it then is.
 */
export function Updates() {
  const queryClient = useQueryClient()
  const { data: update } = useSuspenseQuery({
    ...updateQuery,
    refetchInterval: (query) => (query.state.data?.updating ? 2_000 : 60_000),
  })
  const keep = (changed: Update) => queryClient.setQueryData(updateQuery.queryKey, changed)
  const looking = useMutation({ mutationFn: checkUpdate, onSuccess: keep })
  const changing = useMutation({ mutationFn: changeUpdate, onSuccess: keep })
  const installing = useMutation({ mutationFn: installUpdate, onSuccess: keep })

  // The page was written for the version that sent it. Once another answers, it is fetched anew.
  const [sentBy] = useState(update.version)
  useEffect(() => {
    if (update.version !== sentBy) window.location.reload()
  }, [update.version, sentBy])

  const at = STEPS.findIndex(({ step }) => step === update.updating?.step)

  return (
    <>
      <Section title="This Homewarp">
        <div className="flex max-w-140 flex-col items-start gap-3">
          <p>
            Version <span className="font-mono text-mono text-ink">{update.version}</span> is running.{' '}
            {update.available ? (
              <span className="text-ink">Homewarp {update.newest} is out.</span>
            ) : update.newest ? (
              <>It is the newest there is on the {update.channel} channel.</>
            ) : (
              update.checked_at == null && <>It has not looked for a newer one yet.</>
            )}
          </p>
          {update.problem && <Problem>{update.problem}</Problem>}
          {looking.error && <Problem>{looking.error.message}</Problem>}
          {!update.updating && (
            <div className="flex flex-wrap items-center gap-3">
              <Button busy={looking.isPending} onClick={() => looking.mutate()}>
                Look for a newer one
              </Button>
              {update.checked_at != null && <span className="text-small text-ink-subtle">Last looked {when(update.checked_at)}.</span>}
            </div>
          )}
        </div>
      </Section>

      {update.updating ? (
        <Section title={`Updating to ${update.updating.to}`} lead="Servers that are running go on running. This page comes back by itself.">
          <ul className="flex flex-col gap-3">
            {STEPS.map(({ step, doing }, index) => (
              <li key={step} className={`flex items-start gap-2 ${index > at ? 'text-ink-subtle' : 'text-ink'}`}>
                {index < at ? (
                  <Check aria-hidden className="mt-0.5 size-4 shrink-0 text-state-running" />
                ) : index === at ? (
                  <LoaderCircle aria-hidden className="mt-0.5 size-4 shrink-0 animate-spin motion-reduce:animate-none" />
                ) : (
                  <span aria-hidden className="size-4 shrink-0" />
                )}
                {doing}
              </li>
            ))}
          </ul>
        </Section>
      ) : (
        update.available && (
          <Section title={`Update to ${update.newest}`}>
            {update.installs ? (
              <form
                className="flex max-w-140 flex-col gap-4"
                onSubmit={(event) => {
                  event.preventDefault()
                  installing.mutate(new FormData(event.currentTarget).get('servers') === 'on')
                }}
              >
                <p>
                  Homewarp fetches the release, checks it against its signature, and starts again as the new version. Servers
                  that are running go on running, and the panel is away for under a minute.
                </p>
                <label className="flex items-start gap-2">
                  <input type="checkbox" name="servers" className="mt-0.5 size-4 accent-accent" />
                  <span>
                    Back up every server first
                    <span className="block text-small text-ink-subtle">
                      As each server’s Backups tab does, and kept there. The update goes on only once every backup is made,
                      which takes as long as the servers are large.
                    </span>
                  </span>
                </label>
                <p className="text-small text-ink-subtle">
                  Homewarp’s own database is copied either way. If the new version does not start, the one that runs now is
                  put back, with the database as it was.
                </p>
                {installing.error && <Problem>{installing.error.message}</Problem>}
                <div>
                  <Button type="submit" variant="primary" busy={installing.isPending}>
                    Update to {update.newest}
                  </Button>
                </div>
              </form>
            ) : (
              <div className="flex max-w-140 flex-col items-start gap-3">
                <p>{update.by_hand}</p>
                {update.line && (
                  <>
                    <p>Run this on the machine Homewarp runs on. It puts the new version in place and leaves everything else as it is:</p>
                    <CopyChip text={update.line} />
                  </>
                )}
              </div>
            )}
          </Section>
        )
      )}

      {update.last && !update.updating && (
        <Section title="The last update">
          <div className="flex max-w-140 flex-col gap-2">
            <p>
              {update.last.ok
                ? `Homewarp was updated to ${update.last.to}, ${when(update.last.at)}.`
                : `The update to ${update.last.to} did not start, and the version before it was put back, ${when(update.last.at)}.`}
            </p>
            {update.last.detail && (
              <pre className="overflow-x-auto rounded-md border border-hairline bg-surface-1 p-2.5 font-mono text-mono whitespace-pre-wrap text-ink-muted">
                {update.last.detail}
              </pre>
            )}
          </div>
        </Section>
      )}

      <Section
        title="Which releases"
        lead="A beta is a release that is being tried before it is called one. It has what is new sooner, and what is wrong with it has not been found yet."
      >
        <div className="flex max-w-80 flex-col gap-3">
          <Select
            label="Take"
            value={update.channel}
            disabled={changing.isPending || update.updating != null}
            onChange={(event) => changing.mutate(event.target.value as Update['channel'])}
          >
            <option value="stable">Only what has been released</option>
            <option value="beta">Betas as well</option>
          </Select>
          {changing.error && <Problem>{changing.error.message}</Problem>}
        </div>
      </Section>

      <Gates update={update} />
    </>
  )
}

/**
 * The Gate on each VPS, which is a program of its own and is not replaced
 * from here: nothing at home can run a thing on a VPS, and that is by design.
 * Wanted, not needed: the list comes when the VPSes have answered.
 */
function Gates({ update }: { update: Update }) {
  const network = useNetwork()
  const gates = network?.gates.filter((gate) => gate.state === 'connected') ?? []
  if (gates.length === 0) return null
  const behind = gates.some((gate) => gate.version != null && gate.version !== update.version)

  return (
    <Section
      title="The Gate on each VPS"
      lead="A VPS runs a small program of its own, the Gate. Homewarp can run nothing on a VPS, so a newer Gate is put there by a line that is run on the VPS itself."
    >
      <ul className="max-w-140 overflow-hidden rounded-lg border border-hairline bg-surface-1">
        {gates.map((gate) => (
          <li key={gate.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-hairline px-4 py-2.5 last:border-b-0">
            <span className="min-w-0 flex-1 truncate font-medium text-ink">{gate.name}</span>
            <span className="text-small text-ink-subtle">
              {gate.version == null
                ? 'It does not answer just now'
                : gate.version === update.version
                  ? `Gate ${gate.version}, as this Homewarp`
                  : `Gate ${gate.version}`}
            </span>
          </li>
        ))}
      </ul>
      {behind && update.gate_line && (
        <div className="flex max-w-140 flex-col items-start gap-2">
          <p>
            To put the Gate of {update.version} on a VPS, run this there as root. Its keys and what it forwards stay as they
            are, and players who are connected stay connected:
          </p>
          <CopyChip text={update.gate_line} />
        </div>
      )}
    </Section>
  )
}
