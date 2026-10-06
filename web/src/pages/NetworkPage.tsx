import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Check, LoaderCircle, Plus, TriangleAlert, Waypoints } from 'lucide-react'
import { useEffect, useState, type ReactNode } from 'react'

import { checkGate, connectGate, disconnectGate, type Gate, type PortProtocol } from '../api/client'
import { Button, Confirm, CopyChip, Field, PageBar, Pill, Problem, Steps, buttonClass, type Tone } from '../components/ui'
import { bytes } from '../format'
import { EVERY_GATE, addressOf, gateLook, gateQuery, isChanging } from '../gate'

const PROTOCOLS: Record<PortProtocol, string> = { tcp: 'TCP', udp: 'UDP', both: 'TCP and UDP' }

const PLAYERS: Record<Gate['player_addresses'], { word: string; tone: Tone; sentence: string }> = {
  preserved: {
    word: 'Preserved',
    tone: 'running',
    sentence: 'Servers see each player’s own address, so a ban or a log line means the player it names.',
  },
  hidden: {
    word: 'Hidden',
    tone: 'starting',
    sentence: 'Servers see the Gate’s address for every player. Everything else works as it should.',
  },
  unchecked: {
    word: 'Not checked',
    tone: 'offline',
    sentence: 'Homewarp has not been able to find out whether servers see their players’ own addresses.',
  },
}

const CONNECT_STEPS = ['VPS address', 'Run one command', 'Verify']

/** The Gate for a page that cannot paint without it, asked again as often as it is changing. */
function useGateHere(): Gate {
  const { data } = useSuspenseQuery({
    ...gateQuery,
    refetchInterval: (query) => (isChanging(query.state.data) ? EVERY_GATE.changing : EVERY_GATE.steady),
  })
  return data
}

/** 1536 bytes as "1.5 kB". */
function amount(bytes: number): string {
  const units = ['bytes', 'kB', 'MB', 'GB', 'TB']
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${unit === 0 || value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`
}

/** The tunnel, how it is doing, and every port players reach a server by. */
export function NetworkPage() {
  const gate = useGateHere()
  const connected = gate.state === 'connected'

  return (
    <>
      <PageBar title="Network" />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-8 p-4 md:p-6">
        {connected ? (
          <>
            <Tunnel gate={gate} />
            <PlayerAddresses gate={gate} />
          </>
        ) : gate.state === 'waiting' ? (
          <section className="flex flex-col items-start gap-3 rounded-lg border border-hairline bg-surface-1 p-4">
            <Pill tone="starting">Connecting</Pill>
            <p className="max-w-140">
              The VPS at <code className="font-mono text-ink">{gate.address}</code> has been given its command and has not been
              heard from yet.
            </p>
            <Link to="/network/connect" className={buttonClass('primary')}>
              Continue
            </Link>
          </section>
        ) : (
          <section className="flex flex-col items-center py-10 text-center">
            <Waypoints aria-hidden size={32} className="mb-3 text-ink-faint" />
            <h2 className="text-section text-ink">No VPS connected.</h2>
            <p className="mt-1 max-w-120">
              Servers are reached on your home network only. Connect a VPS and players anywhere join through it, with no port
              opened at home.
            </p>
            <div className="mt-4">
              <Link to="/network/connect" className={buttonClass('primary')}>
                <Plus aria-hidden size={16} />
                Connect a VPS
              </Link>
            </div>
          </section>
        )}
        <Ports gate={gate} />
        {connected && <Disconnect address={gate.address ?? ''} />}
      </main>
    </>
  )
}

/** One of the four places a player's packets pass: a small card (DESIGN.md, Tunnel diagram). */
function Place({ label, pill, sub, children }: { label: string; pill?: ReactNode; sub?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 rounded-lg border border-hairline bg-surface-1 p-3 md:flex-1 md:basis-0">
      <div className="flex min-h-5.5 items-center justify-between gap-2">
        <span className="text-caption text-ink-subtle">{label}</span>
        {pill}
      </div>
      <div className="truncate text-body font-medium text-ink">{children}</div>
      <div className="truncate text-small">{sub}</div>
    </div>
  )
}

/** The line between two places: down the page on a phone, across it otherwise. */
function Wire({ down = false }: { down?: boolean }) {
  return (
    <span
      aria-hidden
      className={`mx-auto h-4 w-0 border-l border-current md:mx-0 md:h-0 md:w-6 md:shrink-0 md:border-t md:border-l-0 ${down ? 'border-dashed text-state-crashed' : 'text-accent'}`}
    />
  )
}

function Tunnel({ gate }: { gate: Gate }) {
  const servers = new Set(gate.ports.map((port) => port.server_id)).size
  const look = gateLook(gate)
  const down = !gate.reachable

  return (
    <section className="flex flex-col gap-3">
      <div className="flex flex-col md:flex-row md:items-center">
        <Place label="Players" sub="anywhere">
          The internet
        </Place>
        <Wire down={down} />
        <Place
          label="Gate"
          pill={<Pill tone={look.tone}>{down ? 'Unreachable' : gate.latency_ms == null ? 'Up' : `${gate.latency_ms} ms`}</Pill>}
          sub="your VPS"
        >
          <span className="font-mono text-mono">{gate.address}</span>
        </Place>
        <Wire down={down} />
        <Place label="Home" sub="where Homewarp runs">
          This machine
        </Place>
        <Wire />
        <Place label="Servers" sub={`${gate.ports.length} ${gate.ports.length === 1 ? 'port' : 'ports'}`}>
          {servers} {servers === 1 ? 'server' : 'servers'}
        </Place>
      </div>
      {gate.problem ? (
        <Problem>{gate.problem}</Problem>
      ) : (
        gate.reachable && (
          <p className="text-small text-ink-subtle">
            {amount(gate.received_bytes)} from home and {amount(gate.sent_bytes)} to it through the tunnel
            {gate.version && ` · Gate ${gate.version}`}
          </p>
        )
      )}
    </section>
  )
}

function PlayerAddresses({ gate }: { gate: Gate }) {
  const queryClient = useQueryClient()
  const checking = useMutation({
    mutationFn: checkGate,
    onSuccess: (checked) => queryClient.setQueryData(gateQuery.queryKey, checked),
  })
  const { word, tone, sentence } = PLAYERS[gate.player_addresses]

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-section text-ink">Player IP addresses</h2>
      <div className="flex flex-wrap items-center gap-3">
        <Pill tone={tone}>{word}</Pill>
        <Button busy={checking.isPending} onClick={() => checking.mutate()}>
          Check again
        </Button>
      </div>
      <p className="max-w-140 text-small text-ink-subtle">{sentence}</p>
      {gate.note && <p className="max-w-140 text-small">{gate.note}</p>}
      {checking.error && <Problem>{checking.error.message}</Problem>}
    </section>
  )
}

/** Every port of every server, and where players reach it. */
function Ports({ gate }: { gate: Gate }) {
  const connected = gate.state === 'connected'
  const cell = 'px-4 font-normal'

  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-section text-ink">{connected ? 'Forwarded ports' : 'Ports'}</h2>
      {gate.ports.length === 0 ? (
        <p>No server has a port yet.</p>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-hairline bg-surface-1">
          <table className="w-full text-left whitespace-nowrap">
            <thead>
              <tr className="h-10 border-b border-hairline text-caption text-ink-subtle">
                <th className={cell}>{connected ? 'Public address' : 'Address at home'}</th>
                <th className={cell}>Protocol</th>
                <th className={cell}>Server</th>
                {connected && <th className={`${cell} text-right`}>Traffic, last day</th>}
              </tr>
            </thead>
            <tbody>
              {gate.ports.map((port) => (
                <tr key={port.port} className="h-10 border-b border-hairline last:border-b-0">
                  <td className={cell}>
                    <CopyChip text={addressOf(gate, port.port)} />
                  </td>
                  <td className={cell}>{PROTOCOLS[port.protocol]}</td>
                  <td className={cell}>
                    <Link
                      to="/servers/$serverId"
                      params={{ serverId: String(port.server_id) }}
                      className="font-medium text-ink hover:underline"
                    >
                      {port.server}
                    </Link>
                  </td>
                  {/* What the VPS counted through the port, both ways. A server reached at home only goes uncounted. */}
                  {connected && <td className={`${cell} text-right tabular-nums`}>{bytes(port.traffic_bytes)}</td>}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}

function Disconnect({ address }: { address: string }) {
  const [asking, setAsking] = useState(false)
  const queryClient = useQueryClient()
  const disconnecting = useMutation({
    mutationFn: disconnectGate,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: gateQuery.queryKey })
      setAsking(false)
    },
  })

  return (
    <section>
      <Button onClick={() => setAsking(true)}>Disconnect this VPS</Button>
      <Confirm
        open={asking}
        onClose={() => setAsking(false)}
        title="Disconnect this VPS?"
        action={
          <Button variant="danger" busy={disconnecting.isPending} onClick={() => disconnecting.mutate()}>
            Disconnect
          </Button>
        }
      >
        <p>
          Players will no longer reach your servers at <code className="font-mono text-ink">{address}</code>. The servers keep
          running, and are reached on your home network as before.
        </p>
        <p className="mt-2">
          The Gate program stays on the VPS until <code className="font-mono text-ink">homewarp-gate leave</code> is run
          there.
        </p>
        {disconnecting.error && (
          <div className="mt-3">
            <Problem>{disconnecting.error.message}</Problem>
          </div>
        )}
      </Confirm>
    </section>
  )
}

/**
 * Connect a VPS: three steps, one thing to do in each (DESIGN.md, Key screens).
 * Which step is open follows from where the Gate stands, so the page can be
 * left and come back to, or opened on another machine, and is where it was.
 */
export function ConnectPage() {
  const gate = useGateHere()
  const at = gate.state === 'waiting' ? 1 : gate.state === 'connected' ? 2 : 0

  return (
    <>
      <PageBar title="Connect a VPS" crumb={<Link to="/network">Network</Link>} />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <Steps steps={CONNECT_STEPS} at={at} />
        {at === 0 ? <AddressStep expired={gate.state === 'expired'} /> : at === 1 ? <CommandStep gate={gate} /> : <VerifyStep gate={gate} />}
      </main>
    </>
  )
}

function AddressStep({ expired }: { expired: boolean }) {
  const queryClient = useQueryClient()
  const connecting = useMutation({
    mutationFn: connectGate,
    onSuccess: (gate) => queryClient.setQueryData(gateQuery.queryKey, gate),
  })

  return (
    <form
      className="flex max-w-140 flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault()
        const form = new FormData(event.currentTarget)
        connecting.mutate({ address: String(form.get('address') ?? ''), wg_port: Number(form.get('wg_port')) || null })
      }}
    >
      <p>
        A VPS gives your servers an address that anyone can reach. It only passes traffic on: your worlds, your files and this
        panel stay at home.
      </p>
      {expired && <p className="text-small text-ink-subtle">The last command was not run within its quarter of an hour. This makes a new one.</p>}
      <Field
        label="VPS IP or hostname"
        name="address"
        required
        autoFocus
        mono
        autoComplete="off"
        spellCheck={false}
        placeholder="203.0.113.10"
        hint="Its public IPv4 address, or a name that leads to it. This is what players will type."
      />
      <details>
        <summary className="cursor-pointer text-ink">Advanced</summary>
        <div className="mt-4">
          <Field
            label="Tunnel port"
            name="wg_port"
            type="number"
            mono
            min={1}
            max={65535}
            defaultValue={51820}
            hint="The UDP port the tunnel uses on the VPS. A firewall in front of the VPS has to let it in."
          />
        </div>
      </details>
      {connecting.error && <Problem>{connecting.error.message}</Problem>}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" busy={connecting.isPending}>
          Continue
        </Button>
        <Link to="/network" className={buttonClass('ghost')}>
          Cancel
        </Link>
      </div>
    </form>
  )
}

/** How long is left until a moment given in Unix seconds, as minutes and seconds. */
function useTimeLeft(until: number | null | undefined): string {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [])
  const left = Math.max(0, (until ?? 0) - Math.floor(now / 1000))
  return `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}`
}

function CommandStep({ gate }: { gate: Gate }) {
  const left = useTimeLeft(gate.expires_at)
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const cancelling = useMutation({
    mutationFn: disconnectGate,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: gateQuery.queryKey })
      await navigate({ to: '/network' })
    },
  })

  return (
    <div className="flex max-w-140 flex-col gap-4">
      <p>
        Run this on <code className="font-mono text-ink">{gate.address}</code>, as root:
      </p>
      {gate.command && <CopyChip text={gate.command} />}
      <p className="text-small text-ink-subtle">
        It needs the <code className="font-mono">homewarp-gate</code> program on the VPS. It sets up the tunnel, a service that
        keeps it up, and the openings a firewall on the VPS needs. Nothing else there is touched.
      </p>
      <p className="flex flex-wrap items-center gap-x-2 text-ink">
        <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
        Waiting for the Gate to come online…
        <span className="text-small text-ink-subtle">The command counts for {left} more.</span>
      </p>
      {gate.problem && <Problem>{gate.problem}</Problem>}
      {cancelling.error && <Problem>{cancelling.error.message}</Problem>}
      <div>
        <Button busy={cancelling.isPending} onClick={() => cancelling.mutate()}>
          Cancel
        </Button>
      </div>
    </div>
  )
}

/** One line of the checklist: done, still being found out, or done with something to say. */
function Checked({ mark, sub, children }: { mark: 'done' | 'busy' | 'warn'; sub?: ReactNode; children: ReactNode }) {
  return (
    <li className="flex items-start gap-2">
      {mark === 'done' ? (
        <Check aria-hidden className="mt-0.5 size-4 shrink-0 text-state-running" />
      ) : mark === 'warn' ? (
        <TriangleAlert aria-hidden className="mt-0.5 size-4 shrink-0 text-state-starting" />
      ) : (
        <LoaderCircle aria-hidden className="mt-0.5 size-4 shrink-0 animate-spin motion-reduce:animate-none" />
      )}
      <div>
        <div className="text-ink">{children}</div>
        {sub && <div className="text-small text-ink-subtle">{sub}</div>}
      </div>
    </li>
  )
}

function VerifyStep({ gate }: { gate: Gate }) {
  const players = gate.player_addresses

  return (
    <div className="flex max-w-140 flex-col gap-4">
      <ul className="flex flex-col gap-3">
        {gate.reachable ? (
          <Checked mark="done" sub={gate.latency_ms != null && `${gate.latency_ms} ms from home to the VPS and back.`}>
            Tunnel up
          </Checked>
        ) : (
          <Checked mark="busy" sub={gate.problem}>
            Tunnel coming up…
          </Checked>
        )}
        <Checked mark="done" sub="The Gate made keys of its own. The command you ran opens nothing now.">
          Keys rotated
        </Checked>
        {players === 'preserved' ? (
          <Checked mark="done" sub={PLAYERS.preserved.sentence}>
            Player IP addresses preserved
          </Checked>
        ) : isChanging(gate) ? (
          <Checked mark="busy">Finding out how players’ addresses arrive…</Checked>
        ) : (
          <Checked mark="warn" sub={gate.note ?? PLAYERS[players].sentence}>
            {players === 'hidden' ? 'Servers see the Gate’s address, not their players’' : 'Player IP addresses not checked'}
          </Checked>
        )}
      </ul>
      <div className="flex gap-2">
        {gate.ports.length === 0 ? (
          <>
            <Link to="/servers/new" className={buttonClass('primary')}>
              Create first server
            </Link>
            <Link to="/network" className={buttonClass('ghost')}>
              Done
            </Link>
          </>
        ) : (
          <Link to="/network" className={buttonClass('primary')}>
            Done
          </Link>
        )}
      </div>
    </div>
  )
}
