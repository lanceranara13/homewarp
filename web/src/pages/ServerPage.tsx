import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, Outlet, getRouteApi, notFound, useNavigate } from '@tanstack/react-router'
import { Play, Square, Trash2 } from 'lucide-react'
import { createContext, use, useEffect, useRef, useState, type ReactNode } from 'react'

import {
  changeServer,
  commandServer,
  followServer,
  powerServer,
  removeServer,
  type Permission,
  type Power,
  type ServerEvent,
  type ServerSettings,
  type ServerState,
  type Usage,
} from '../api/client'
import { Button, Confirm, CopyChip, Field, PageBar, Problem, StatusPill, buttonClass } from '../components/ui'
import { addressOf, useGate } from '../gate'
import { EVERY, serverQuery, serversQuery } from '../servers'
import { useOwner } from '../session'
import { templateQuery } from '../templates'
import { ServerForm } from './NewServerPage'

const route = getRouteApi('/shell/servers/$serverId')

/** How many lines of console a page holds on to. */
const KEPT_LINES = 1000
/** The states in which a server has a process, and so uses some of the machine. */
const USES: ServerState[] = ['starting', 'running', 'stopping']

/** What the socket has told a page about its server. */
type Followed = { state: ServerState; lines: string[]; usage: Usage | null }

/** What the socket has said, for the tab that shows it. Null while there is no socket. */
const Live = createContext<Followed | null>(null)

/**
 * The frame every page of one server sits in: its name, what it is doing, where
 * players reach it and its power controls, always in view, and under them the
 * tabs (DESIGN.md, Inside a server). The socket is the frame's, so a console
 * left for another tab is as it was on coming back.
 */
export function ServerLayout() {
  const { serverId } = route.useParams()
  const id = Number(serverId)
  const followed = useFollowed(id)
  // The first paint comes from this one request. The socket takes over once it has said where
  // things stand, and asking again is what is left when there is no socket.
  const { data: server } = useSuspenseQuery({ ...serverQuery(id), refetchInterval: followed ? false : EVERY.server })
  // The loader turns away an address with no server behind it; this is one removed since.
  if (!server) throw notFound()
  const state = followed?.state ?? server.state
  // Wanted, not needed: the chip is painted with the address at home, and takes the VPS's once it is known.
  const gate = useGate()
  const owner = useOwner()

  return (
    <>
      <PageBar title={server.name} crumb={<Link to="/">Servers</Link>}>
        {owner && <RemoveServer id={server.id} name={server.name} />}
      </PageBar>
      <div className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <div className="flex flex-wrap items-start gap-3">
          <div className="flex min-h-10 flex-wrap items-center gap-3 md:min-h-8">
            <StatusPill state={state} />
            <CopyChip text={addressOf(gate, server.port)} />
          </div>
          {/* What an account has not been let do is not put before it to be refused. */}
          {server.permissions.includes('power') && (
            <div className="ml-auto">
              <PowerControls id={server.id} state={state} />
            </div>
          )}
        </div>
        <Tabs serverId={serverId} may={server.permissions} owner={owner} />
        <main className="flex min-w-0 flex-1 flex-col gap-4">
          <Live value={followed}>
            <Outlet />
          </Live>
        </main>
      </div>
    </>
  )
}

const TAB = 'flex h-10 shrink-0 items-center border-b-2 px-3 text-body font-medium transition-colors duration-120 ease-out'
const TAB_HERE = { className: 'border-accent text-ink' }
const TAB_ELSEWHERE = { className: 'border-transparent text-ink-subtle hover:text-ink' }

/** Underline tabs on a hairline. They scroll sideways on a narrow screen, and never wrap (DESIGN.md, Tabs). */
function Tabs({ serverId, may, owner }: { serverId: string; may: Permission[]; owner: boolean }) {
  return (
    // The hairline is the frame's and the underlines are the tabs': the row of tabs sits a pixel
    // into it, as a whole, because a row that scrolls sideways cuts off what hangs out of it.
    <div className="border-b border-hairline">
      <nav aria-label="Server" className="-mb-px flex overflow-x-auto">
        <Link
          to="/servers/$serverId"
          params={{ serverId }}
          activeOptions={{ exact: true }}
          className={TAB}
          activeProps={TAB_HERE}
          inactiveProps={TAB_ELSEWHERE}
        >
          Console
        </Link>
        {/* Here for every folder, and for the editor too. */}
        {may.includes('files') && (
          <Link
            to="/servers/$serverId/files"
            params={{ serverId }}
            activeOptions={{ includeSearch: false }}
            className={TAB}
            activeProps={TAB_HERE}
            inactiveProps={TAB_ELSEWHERE}
          >
            Files
          </Link>
        )}
        {may.includes('backups') && (
          <Link
            to="/servers/$serverId/backups"
            params={{ serverId }}
            className={TAB}
            activeProps={TAB_HERE}
            inactiveProps={TAB_ELSEWHERE}
          >
            Backups
          </Link>
        )}
        {may.includes('schedules') && (
          <Link
            to="/servers/$serverId/schedules"
            params={{ serverId }}
            className={TAB}
            activeProps={TAB_HERE}
            inactiveProps={TAB_ELSEWHERE}
          >
            Schedules
          </Link>
        )}
        {owner && (
          <Link
            to="/servers/$serverId/users"
            params={{ serverId }}
            className={TAB}
            activeProps={TAB_HERE}
            inactiveProps={TAB_ELSEWHERE}
          >
            Users
          </Link>
        )}
        {may.includes('settings') && (
          <Link
            to="/servers/$serverId/settings"
            params={{ serverId }}
            className={TAB}
            activeProps={TAB_HERE}
            inactiveProps={TAB_ELSEWHERE}
          >
            Settings
          </Link>
        )}
      </nav>
    </div>
  )
}

/** The console, what the server uses of the machine, and what it is made of. */
export function ConsoleTab() {
  const { serverId } = route.useParams()
  const id = Number(serverId)
  // The frame keeps this fresh; here it is only read.
  const { data: server } = useSuspenseQuery(serverQuery(id))
  const followed = use(Live)
  const owner = useOwner()
  if (!server) throw notFound()
  const state = followed?.state ?? server.state

  return (
    <>
      <div className="grid gap-4 wide:grid-cols-[minmax(0,1fr)_16rem]">
        <Console
          id={server.id}
          lines={followed?.lines ?? server.console}
          open={state === 'starting' || state === 'running'}
          may={server.permissions.includes('console')}
        />
        <UsageTiles usage={followed?.usage ?? null} />
      </div>
      <p className="text-small text-ink-subtle">
        {/* Templates are the owner's: to anyone else this is a name, and leads nowhere. */}
        {owner ? (
          <Link to="/templates/$templateId" params={{ templateId: String(server.template_id) }} className="text-accent hover:underline">
            {server.template}
          </Link>
        ) : (
          server.template
        )}
        {' · '}
        <code className="font-mono wrap-anywhere">{server.image}</code>
        {` · ${server.memory_mb} MB`}
        {server.cpu_percent > 0 && ` · ${server.cpu_percent} % of a core`}
        {server.ports.length > 0 &&
          ` · also on ${server.ports.map((further) => `${further.port}${!further.protocol || further.protocol === 'both' ? '' : `/${further.protocol}`}`).join(', ')}`}
      </p>
    </>
  )
}

/**
 * Where a server is changed: its name, what it may use of the machine, and what
 * its template asks. It has to be stopped, and is as it was changed from its
 * next start.
 */
export function ServerSettingsTab() {
  const { serverId } = route.useParams()
  const id = Number(serverId)
  const { data: server } = useSuspenseQuery(serverQuery(id))
  if (!server) throw notFound()
  const { data: template } = useSuspenseQuery(templateQuery(server.template_id))
  if (!template) throw notFound()

  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const changing = useMutation({
    mutationFn: (settings: ServerSettings) => changeServer(id, settings),
    onSuccess: async (changed) => {
      queryClient.setQueryData(serverQuery(id).queryKey, changed)
      void queryClient.invalidateQueries({ queryKey: serversQuery.queryKey, exact: true })
      await navigate({ to: '/servers/$serverId', params: { serverId } })
    },
  })

  return (
    <>
      <p className="max-w-140 text-small text-ink-subtle">
        A server is changed while it is stopped, and runs as it was changed from its next start. Its files are not
        touched.
      </p>
      <ServerForm
        template={template}
        start={server}
        submit="Save"
        pending={changing.isPending}
        problem={changing.error?.message}
        onSubmit={(settings) => changing.mutate(settings)}
        cancel={
          <Link to="/servers/$serverId" params={{ serverId }} className={buttonClass('ghost')}>
            Cancel
          </Link>
        }
      />
    </>
  )
}

function told(before: Followed | null, event: ServerEvent): Followed | null {
  switch (event.kind) {
    case 'snapshot':
      return { state: event.state, lines: event.lines, usage: event.usage ?? null }
    case 'line':
      return before && { ...before, lines: [...before.lines.slice(1 - KEPT_LINES), event.text] }
    case 'state':
      // What is not running uses nothing, and is not told so in a message of its own.
      return before && { ...before, state: event.state, usage: USES.includes(event.state) ? before.usage : null }
    case 'usage':
      return before && { ...before, usage: event.usage }
  }
}

/**
 * Follows a server over its socket. Null until the socket has said where things
 * stand, and again whenever it is lost; it is then opened again, a little later
 * each time it fails.
 */
function useFollowed(id: number): Followed | null {
  const [followed, setFollowed] = useState<Followed | null>(null)

  useEffect(() => {
    let socket: WebSocket | undefined
    let retry: number | undefined
    let wait = 500
    let frame: number | undefined
    let unseen: ServerEvent[] = []
    let over = false

    // A server that is starting prints hundreds of lines a second. They are
    // taken together, once per frame, rather than one render each.
    const show = () => {
      frame = undefined
      const events = unseen
      unseen = []
      setFollowed((before) => events.reduce(told, before))
    }
    const open = () => {
      socket = followServer(id)
      socket.onmessage = (message) => {
        wait = 500
        unseen.push(JSON.parse(String(message.data)) as ServerEvent)
        frame ??= window.requestAnimationFrame(show)
      }
      socket.onclose = () => {
        if (over) return
        setFollowed(null)
        retry = window.setTimeout(open, wait)
        wait = Math.min(wait * 2, 8000)
      }
    }
    open()

    return () => {
      over = true
      window.clearTimeout(retry)
      if (frame !== undefined) window.cancelAnimationFrame(frame)
      socket?.close()
      setFollowed(null)
    }
  }, [id])

  return followed
}

/** Start and stop, always in view. Only what the server's state allows can be pressed. */
function PowerControls({ id, state }: { id: number; state: ServerState }) {
  const queryClient = useQueryClient()
  const power = useMutation({
    mutationFn: (action: Power) => powerServer(id, action),
    // The server has changed state by the time it answers. A page without its socket asks what to.
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
 * The server's console, near-black in both themes, with a line to type into
 * while the server runs. It keeps to the newest line unless the reader has
 * scrolled up to look at older ones.
 */
function Console({ id, lines, open, may }: { id: number; lines: string[]; open: boolean; may: boolean }) {
  const box = useRef<HTMLDivElement>(null)
  const following = useRef(true)
  useEffect(() => {
    if (box.current && following.current) box.current.scrollTop = box.current.scrollHeight
  }, [lines])

  return (
    <div className="flex h-[60dvh] min-h-64 min-w-0 flex-col overflow-hidden rounded-lg bg-console font-mono text-mono text-on-console">
      <div
        ref={box}
        role="log"
        aria-label="Console"
        tabIndex={0}
        onScroll={({ currentTarget: at }) => {
          following.current = at.scrollHeight - at.scrollTop - at.clientHeight < 24
        }}
        className="flex-1 overflow-y-auto p-3"
      >
        {lines.length === 0 && <p className="opacity-60">Nothing printed yet.</p>}
        {lines.map((line, index) => (
          <div key={index} className="min-h-5 wrap-anywhere whitespace-pre-wrap">
            {line}
          </div>
        ))}
      </div>
      {/* An account that may only look has the console to read, and no line to type into. */}
      {may && <CommandLine id={id} open={open} />}
    </div>
  )
}

/** Where a command is typed. The up and down arrows go through what was typed before. */
function CommandLine({ id, open }: { id: number; open: boolean }) {
  const [typed, setTyped] = useState('')
  const earlier = useRef<string[]>([])
  const at = useRef(0)
  const sending = useMutation({ mutationFn: (command: string) => commandServer(id, command) })

  return (
    <form
      className="border-t border-on-console/15"
      onSubmit={(event) => {
        event.preventDefault()
        const command = typed.trim()
        if (!command) return
        earlier.current.push(command)
        at.current = earlier.current.length
        setTyped('')
        sending.mutate(command)
      }}
    >
      <label className="flex h-10 items-center gap-2 px-3">
        <span aria-hidden className="opacity-60">
          &gt;
        </span>
        <input
          aria-label="Command"
          value={typed}
          disabled={!open}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          placeholder={open ? 'Type a command' : 'The server is not running'}
          onChange={(event) => setTyped(event.currentTarget.value)}
          onKeyDown={(event) => {
            const step = event.key === 'ArrowUp' ? -1 : event.key === 'ArrowDown' ? 1 : 0
            if (step === 0) return
            event.preventDefault()
            at.current = Math.min(Math.max(at.current + step, 0), earlier.current.length)
            setTyped(earlier.current[at.current] ?? '')
          }}
          // The focus ring is drawn inside the field: outside it, the console's rounded edge cuts it off.
          className="h-full min-w-0 flex-1 bg-transparent -outline-offset-2 placeholder:text-on-console/50"
        />
      </label>
      {sending.error && (
        <div className="px-3 pb-2 font-sans">
          <Problem>{sending.error.message}</Problem>
        </div>
      )}
    </form>
  )
}

/** What the server is using of the machine, about once a second while it runs. */
function UsageTiles({ usage }: { usage: Usage | null }) {
  const megabytes = (bytes: number) => bytes / (1024 * 1024)
  const size = (bytes: number) =>
    megabytes(bytes) < 1024 ? `${Math.round(megabytes(bytes))} MB` : `${(megabytes(bytes) / 1024).toFixed(1)} GB`
  const full = usage && usage.memory_limit_bytes > 0 ? usage.memory_bytes / usage.memory_limit_bytes : 0

  return (
    <div className="-order-1 grid grid-cols-2 content-start gap-4 wide:order-none wide:grid-cols-1">
      <Tile label="Processor" value={usage ? `${Math.round(usage.cpu_percent)} %` : '—'} />
      <Tile label="Memory" value={usage ? size(usage.memory_bytes) : '—'}>
        {usage && usage.memory_limit_bytes > 0 && (
          <>
            <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-3">
              {/* Neutral until it matters: amber above four fifths, red when it is about to be stopped. */}
              <div
                className={`h-full rounded-full ${full > 0.95 ? 'bg-state-crashed' : full > 0.8 ? 'bg-state-starting' : 'bg-ink-subtle'}`}
                style={{ width: `${Math.min(full, 1) * 100}%` }}
              />
            </div>
            <p className="mt-1.5 text-small text-ink-subtle">of {size(usage.memory_limit_bytes)}</p>
          </>
        )}
      </Tile>
    </div>
  )
}

function Tile({ label, value, children }: { label: string; value: string; children?: ReactNode }) {
  return (
    <div className="rounded-lg border border-hairline bg-surface-1 p-4">
      <p className="text-caption text-ink-subtle">{label}</p>
      <p className="mt-1 text-stat text-ink tabular-nums">{value}</p>
      {children}
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
