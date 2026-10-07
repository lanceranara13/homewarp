import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { LoaderCircle } from 'lucide-react'
import { useState } from 'react'

import { askForCertificate, changePanel, type Panel } from '../api/client'
import { Button, Confirm, CopyChip, Field, Pill, Problem } from '../components/ui'
import { when } from '../format'
import { panelQuery } from '../gate'

/** Where an authority's terms are read, for the one this page knows by name. */
function termsOf(authority: string): string | null {
  return authority.endsWith('letsencrypt.org') ? 'https://letsencrypt.org/repository/' : null
}

/** What a web server on the VPS is to be told, where it has port 80: written for nginx, which most have. */
function forWebServer(name: string, port: number, answers: string): string {
  return [
    'server {',
    '    listen 80;',
    '    listen [::]:80;',
    `    server_name ${name};`,
    '    location /.well-known/acme-challenge/ {',
    `        alias ${answers}/;`,
    '        default_type text/plain;',
    '    }',
    '    location / {',
    `        return 301 https://$host${port === 443 ? '' : `:${port}`}$request_uri;`,
    '    }',
    '}',
  ].join('\n')
}

/**
 * The panel's way in from the internet: a name that leads to the VPS, which
 * passes the panel's port home unread. Nothing on the page waits for this.
 */
export function PanelAddress({ connected }: { connected: boolean }) {
  const queryClient = useQueryClient()
  const { data: panel } = useQuery({
    ...panelQuery,
    refetchInterval: (query) => (query.state.data?.state === 'asking' ? 2_000 : 30_000),
  })
  const keep = (changed: Panel) => queryClient.setQueryData(panelQuery.queryKey, changed)
  const changing = useMutation({ mutationFn: changePanel, onSuccess: keep })
  const asking = useMutation({ mutationFn: askForCertificate, onSuccess: keep })
  const [leaving, setLeaving] = useState(false)

  if (!panel) return null
  const terms = termsOf(panel.authority)
  const port = panel.port ?? 443

  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-section text-ink">Panel address</h2>
      {!panel.available ? (
        <p className="max-w-140 text-small text-ink-subtle">
          This panel is reached on your home network only. This Homewarp was started without a door for TLS, which reaching it
          by a name from anywhere takes.
        </p>
      ) : panel.name == null ? (
        <form
          className="flex max-w-140 flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            const form = new FormData(event.currentTarget)
            changing.mutate({ name: String(form.get('name') ?? ''), agreed: form.get('agreed') === 'on' })
          }}
        >
          <p>
            This panel is reached on your home network only. Give it a name that leads to your VPS and it is reached from
            anywhere, at <code className="font-mono text-ink">https://that-name{port === 443 ? '' : `:${port}`}</code>. The
            VPS passes it on without being able to read it: passwords and files are encrypted between your browser and this
            machine.
          </p>
          <Field
            label="Name"
            name="name"
            required
            mono
            autoComplete="off"
            spellCheck={false}
            placeholder="panel.example.com"
            hint="A DNS name whose address is your VPS’s. Make that record first."
          />
          <label className="flex items-start gap-2">
            <input type="checkbox" name="agreed" required className="mt-0.5 size-4 accent-accent" />
            <span>
              I agree to the{' '}
              {terms ? (
                <a href={terms} target="_blank" rel="noreferrer" className="text-accent hover:underline">
                  terms of {panel.authority}
                </a>
              ) : (
                <>terms of {panel.authority}</>
              )}
              .<span className="block text-small text-ink-subtle">That is who the certificate for the name is asked of.</span>
            </span>
          </label>
          {!connected && (
            <p className="text-small text-ink-subtle">A VPS has to be connected first: the name leads to it.</p>
          )}
          {changing.error && <Problem>{changing.error.message}</Problem>}
          <div>
            <Button type="submit" variant="primary" busy={changing.isPending} disabled={!connected}>
              Put the panel online
            </Button>
          </div>
        </form>
      ) : (
        <div className="flex max-w-140 flex-col gap-3">
          <div className="flex flex-wrap items-center gap-3">
            {panel.state === 'on' ? (
              <Pill tone="running">Online</Pill>
            ) : panel.state === 'failed' ? (
              <Pill tone="crashed">No certificate</Pill>
            ) : (
              <Pill tone="starting">{panel.state === 'asking' ? 'Asking for a certificate' : 'Waiting'}</Pill>
            )}
            {panel.address ? <CopyChip text={panel.address} /> : <code className="font-mono text-mono text-ink">{panel.name}</code>}
            {panel.state === 'asking' && <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />}
          </div>
          {panel.certificate && (
            <p className="text-small text-ink-subtle">
              Certificate from {panel.certificate.authority}, good until {when(panel.certificate.expires_at)}. A newer one is
              asked for by itself from {when(panel.certificate.renews_at)}.
            </p>
          )}
          {panel.problem && <Problem>{panel.problem}</Problem>}
          {panel.state === 'failed' && panel.answers && (
            <>
              <p className="text-small">
                The VPS has a web server on port 80, where the certificate’s question is asked. Have it give the answers the
                Gate keeps, and send everything else here. For nginx, as a file of its own:
              </p>
              <pre className="overflow-x-auto rounded-md border border-hairline bg-surface-1 p-3 font-mono text-mono text-ink">
                {forWebServer(panel.name, port, panel.answers)}
              </pre>
            </>
          )}
          {panel.state === 'on' && panel.caa && (
            <div className="flex flex-col items-start gap-1.5">
              <p className="text-small">
                The name leads to the VPS, so whoever had the VPS could ask for a certificate of their own. One more DNS record
                for <code className="font-mono text-ink">{panel.name}</code>, of type CAA, has the authority give one to this
                Homewarp only:
              </p>
              <CopyChip text={panel.caa} />
            </div>
          )}
          {asking.error && <Problem>{asking.error.message}</Problem>}
          <div className="flex flex-wrap gap-2">
            {panel.state !== 'asking' && panel.state !== 'on' && (
              <Button variant="primary" busy={asking.isPending} onClick={() => asking.mutate()}>
                Try again
              </Button>
            )}
            <Button onClick={() => setLeaving(true)}>Take the panel offline</Button>
          </div>
          <Confirm
            open={leaving}
            onClose={() => setLeaving(false)}
            title="Take the panel offline?"
            action={
              <Button
                variant="danger"
                busy={changing.isPending}
                onClick={() => changing.mutate({ name: null, agreed: false }, { onSuccess: () => setLeaving(false) })}
              >
                Take it offline
              </Button>
            }
          >
            <p>
              The panel will no longer be reached at <code className="font-mono text-ink">{panel.name}</code>, and its
              certificate is thrown away. It is reached on your home network as before. Servers are not affected.
            </p>
            {changing.error && (
              <div className="mt-3">
                <Problem>{changing.error.message}</Problem>
              </div>
            )}
          </Confirm>
        </div>
      )}
    </section>
  )
}
