import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, getRouteApi, notFound, useNavigate } from '@tanstack/react-router'
import { Play, Square, Trash2 } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

import { powerServer, removeServer, type Power, type ServerState } from '../api/client'
import { Button, Confirm, CopyChip, Field, PageBar, Problem, StatusPill } from '../components/ui'
import { EVERY, serverQuery, serversQuery } from '../servers'

const route = getRouteApi('/shell/servers/$serverId')

/** One server: what it is doing, where players reach it, its power controls and its console. */
export function ServerPage() {
  const { serverId } = route.useParams()
  const { data: server } = useSuspenseQuery({ ...serverQuery(Number(serverId)), refetchInterval: EVERY.server })
  // The loader turns away an address with no server behind it; this is one removed since.
  if (!server) throw notFound()

  return (
    <>
      <PageBar title={server.name} crumb={<Link to="/">Servers</Link>}>
        <RemoveServer id={server.id} name={server.name} />
      </PageBar>
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <div className="flex flex-wrap items-start gap-3">
          <div className="flex min-h-10 flex-wrap items-center gap-3 md:min-h-8">
            <StatusPill state={server.state} />
            <CopyChip text={`${window.location.hostname}:${server.port}`} />
          </div>
          <div className="ml-auto">
            <PowerControls id={server.id} state={server.state} />
          </div>
        </div>
        <Console lines={server.console} />
        <p className="text-small text-ink-subtle">
          <Link to="/templates/$templateId" params={{ templateId: String(server.template_id) }} className="text-accent hover:underline">
            {server.template}
          </Link>
          {' · '}
          <code className="font-mono wrap-anywhere">{server.image}</code>
          {` · ${server.memory_mb} MB`}
          {server.cpu_percent > 0 && ` · ${server.cpu_percent} % of a core`}
        </p>
      </main>
    </>
  )
}

/** Start and stop, always in view. Only what the server's state allows can be pressed. */
function PowerControls({ id, state }: { id: number; state: ServerState }) {
  const queryClient = useQueryClient()
  const power = useMutation({
    mutationFn: (action: Power) => powerServer(id, action),
    // The server has changed state by the time it answers, so the page asks what to.
    onSettled: () => queryClient.invalidateQueries({ queryKey: serverQuery(id).queryKey }),
  })
  const allowed = (...states: ServerState[]) => states.includes(state) && !power.isPending

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap justify-end gap-2">
        {state === 'install_failed' ? (
          <Button variant="primary" busy={power.isPending} onClick={() => power.mutate('install')}>
            Install again
          </Button>
        ) : (
          <>
            <Button variant="primary" disabled={!allowed('offline', 'crashed')} onClick={() => power.mutate('start')}>
              <Play aria-hidden size={16} />
              Start
            </Button>
            <Button disabled={!allowed('starting', 'running')} onClick={() => power.mutate('stop')}>
              <Square aria-hidden size={16} />
              Stop
            </Button>
            {/* For a server that was asked to stop and has not: the way out that does not wait for it. */}
            {state === 'stopping' && (
              <Button variant="ghost" disabled={power.isPending} onClick={() => power.mutate('kill')}>
                Kill
              </Button>
            )}
          </>
        )}
      </div>
      {power.error && <Problem>{power.error.message}</Problem>}
    </div>
  )
}

/**
 * The end of the server's console, near-black in both themes. It keeps to the
 * newest line unless the reader has scrolled up to look at older ones.
 */
function Console({ lines }: { lines: string[] }) {
  const box = useRef<HTMLDivElement>(null)
  const following = useRef(true)
  useEffect(() => {
    if (box.current && following.current) box.current.scrollTop = box.current.scrollHeight
  }, [lines])

  return (
    <div
      ref={box}
      role="log"
      aria-label="Console"
      tabIndex={0}
      onScroll={({ currentTarget: at }) => {
        following.current = at.scrollHeight - at.scrollTop - at.clientHeight < 24
      }}
      className="h-[60dvh] min-h-64 overflow-y-auto rounded-lg bg-console p-3 font-mono text-mono text-on-console"
    >
      {lines.length === 0 && <p className="opacity-60">Nothing printed yet.</p>}
      {lines.map((line, index) => (
        <div key={index} className="min-h-5 wrap-anywhere whitespace-pre-wrap">
          {line}
        </div>
      ))}
    </div>
  )
}

/** The way out of a server. Its files go with it, so its name has to be typed first. */
function RemoveServer({ id, name }: { id: number; name: string }) {
  const [asking, setAsking] = useState(false)
  const [typed, setTyped] = useState('')
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const removing = useMutation({
    mutationFn: () => removeServer(id),
    onSuccess: async () => {
      // The Servers page asks afresh; this page's own answer goes once the page has.
      queryClient.removeQueries({ queryKey: serversQuery.queryKey, exact: true })
      await navigate({ to: '/' })
      queryClient.removeQueries({ queryKey: serverQuery(id).queryKey })
    },
  })

  return (
    <>
      <Button onClick={() => setAsking(true)}>
        <Trash2 aria-hidden size={16} />
        Remove
      </Button>
      <Confirm
        open={asking}
        onClose={() => {
          setAsking(false)
          setTyped('')
          removing.reset()
        }}
        title={`Remove ${name}?`}
        action={
          <Button variant="danger" disabled={typed !== name} busy={removing.isPending} onClick={() => removing.mutate()}>
            Remove
          </Button>
        }
      >
        <p>The server goes, and its files with it: worlds, settings, all of it. They cannot be brought back.</p>
        <div className="mt-3">
          <Field
            label={`Type ${name} to go ahead`}
            value={typed}
            onChange={(event) => setTyped(event.currentTarget.value)}
            autoComplete="off"
            spellCheck={false}
          />
        </div>
        {removing.error && (
          <div className="mt-3">
            <Problem>{removing.error.message}</Problem>
          </div>
        )}
      </Confirm>
    </>
  )
}
