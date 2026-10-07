import { useMutation, useQuery, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { Check, LoaderCircle, Plus, TriangleAlert, Waypoints } from 'lucide-react'
import { useEffect, useState, type ReactNode } from 'react'

import { checkGate, connectGate, disconnectGate, renameGate, type Gate, type Network, type PortProtocol } from '../api/client'
import { TrafficChart, type Sample } from '../components/TrafficChart'
import { Button, Confirm, CopyChip, Field, PageBar, Pill, Problem, Steps, buttonClass, type Tone } from '../components/ui'
import { bytes } from '../format'
import { addressOf, everyGate, gateQuery, isChanging, lookOf, trafficQuery } from '../gate'
import { useOwner } from '../session'
import { PanelAddress } from './PanelAddress'
import { VpsGuard } from './VpsGuard'

const connect = getRouteApi('/shell/network/connect')

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

/** The VPSes for a page that cannot paint without them, asked again as often as one is changing. */
function useNetworkHere(): Network {
  const { data } = useSuspenseQuery({ ...gateQuery, refetchInterval: (query) => everyGate(query.state.data) })
  return data
}

/**
 * What has passed through each tunnel over the last few minutes, a sample at
 * a time, by the VPS's id. Wanted, not needed: the page is painted before it
 * answers, and it is asked again as often as Homewarp looks.
 */
function useTraffic(asked: boolean): { slots: number; of: (id: number) => Sample[]; all: Sample[] } {
  const { data, dataUpdatedAt } = useQuery({ ...trafficQuery, enabled: asked, refetchInterval: 2_000 })
  const every = (data?.every_seconds ?? 2) * 1000
  // The newest sample is of now, and each one before it is that much older.
  const timed = (samples: { received: number; sent: number }[]): Sample[] =>
    samples.map((sample, index) => ({ ...sample, at: dataUpdatedAt - (samples.length - 1 - index) * every }))
  const gates = data?.gates ?? []
  const longest = gates.reduce((longest, gate) => Math.max(longest, gate.samples.length), 0)
  // All of them together, counted back from the newest: a tunnel that has been up for less adds nothing before it was.
  const all = Array.from({ length: longest }, (_, index) => {
    const back = longest - 1 - index
    return gates.reduce(
      (sum, gate) => {
        const sample = gate.samples[gate.samples.length - 1 - back]
        return sample ? { received: sum.received + sample.received, sent: sum.sent + sample.sent } : sum
      },
      { received: 0, sent: 0 },
    )
  })
  return {
    slots: data?.most ?? 150,
    of: (id) => timed(gates.find((gate) => gate.id === id)?.samples ?? []),
    all: timed(all),
  }
}

/** Every VPS, what passes through each, and every port players reach a server by. */
export function NetworkPage() {
  const network = useNetworkHere()
  const owner = useOwner()
  const connected = network.gates.filter((gate) => gate.state === 'connected')
  const traffic = useTraffic(owner && connected.length > 0)
  const another = owner && (
    <Link to="/network/connect" className={buttonClass('primary')}>
      <Plus aria-hidden size={16} />
      Connect a VPS
    </Link>
  )

  return (
    <>
      <PageBar title="Network">{network.gates.length > 0 && another}</PageBar>
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-8 p-4 md:p-6">
        {network.gates.length === 0 ? (
          <section className="flex flex-col items-center py-10 text-center">
            <Waypoints aria-hidden size={32} className="mb-3 text-ink-faint" />
            <h2 className="text-section text-ink">No VPS connected.</h2>
            <p className="mt-1 max-w-120">
              Servers are reached on your home network only. Connect a VPS and players anywhere join through it, with no port
              opened at home.
            </p>
            <div className="mt-4">{another}</div>
          </section>
        ) : (
          <>
            {owner && connected.length > 0 && (
              <section className="flex flex-col gap-3">
                <div>
                  <h2 className="text-section text-ink">Traffic now</h2>
                  <p className="mt-1 max-w-140 text-small text-ink-subtle">
                    What passes between players and your servers{connected.length > 1 && ', through every VPS together'}, over
                    the last five minutes.
                  </p>
                </div>
                <div className="rounded-lg border border-hairline bg-surface-1 p-4">
                  <TrafficChart samples={traffic.all} slots={traffic.slots} labels={['To servers', 'To players']} />
                </div>
              </section>
            )}
            <section className="flex flex-col gap-3">
              <h2 className="text-section text-ink">{network.gates.length === 1 ? 'Your VPS' : 'Your VPSes'}</h2>
              <div className="grid items-start gap-4 wide:grid-cols-2">
                {network.gates.map((gate) => (
                  <Vps
                    key={gate.id}
                    gate={gate}
                    owner={owner}
                    samples={traffic.of(gate.id)}
                    slots={traffic.slots}
                    // Which one a new server would take is worth saying only where there is a choice.
                    recommended={connected.length > 1 && network.recommended?.gate_id === gate.id ? network.recommended.why : null}
                  />
                ))}
              </div>
            </section>
          </>
        )}
        <Ports network={network} />
        {owner && <PanelAddress connected={connected.length > 0} />}
      </main>
    </>
  )
}

/** One figure of a VPS: what it is, and how much. */
function Figure({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-caption text-ink-subtle">{label}</dt>
      <dd className="truncate font-medium text-ink tabular-nums">{children}</dd>
    </div>
  )
}

/**
 * One VPS: where it is, how the tunnel to it is doing, what passes through it,
 * and for its owner what can be done with it.
 */
function Vps({
  gate,
  owner,
  samples,
  slots,
  recommended,
}: {
  gate: Gate
  owner: boolean
  samples: Sample[]
  slots: number
  recommended: string | null
}) {
  const look = lookOf(gate)
  const players = PLAYERS[gate.player_addresses]

  return (
    <article className="flex min-w-0 flex-col gap-4 rounded-lg border border-hairline bg-surface-1 p-4">
      <header className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {/* One that has not been called anything is called by its address, which is then said once. */}
        {gate.name === gate.address ? (
          <h3 className="min-w-0">
            <CopyChip text={gate.address} />
          </h3>
        ) : (
          <>
            <h3 className="min-w-0 truncate font-medium text-ink">{gate.name}</h3>
            <CopyChip text={gate.address} />
          </>
        )}
        <span className="ml-auto">
          <Pill tone={look.tone}>{look.word}</Pill>
        </span>
      </header>
      {gate.state === 'connected' ? (
        <>
          {gate.problem && <Problem>{gate.problem}</Problem>}
          {recommended && <p className="text-small text-ink-subtle">A new server would take this one: {recommended}.</p>}
          <dl className="grid grid-cols-2 gap-x-4 gap-y-3 md:grid-cols-4">
            <Figure label="Ping">{gate.latency_ms == null ? '—' : `${gate.latency_ms} ms`}</Figure>
            <Figure label="CPU usage">{gate.load_percent == null ? '—' : `${gate.load_percent} %`}</Figure>
            <Figure label="Servers">{gate.servers}</Figure>
            <Figure label="Traffic, last day">{bytes(gate.traffic_bytes)}</Figure>
          </dl>
          {owner && <TrafficChart samples={samples} slots={slots} labels={['To servers', 'To players']} compact />}
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-small">
            <span className="text-ink-subtle">Player IP addresses</span>
            <Pill tone={players.tone}>{players.word}</Pill>
            {gate.version && <span className="ml-auto text-ink-subtle">Gate {gate.version}</span>}
          </div>
          {gate.note && <p className="text-small">{gate.note}</p>}
          {owner && <Manage gate={gate} />}
        </>
      ) : (
        <Awaited gate={gate} />
      )}
    </article>
  )
}

/** A VPS that was given its command and is not connected: still awaited, or given up on. */
function Awaited({ gate }: { gate: Gate }) {
  const queryClient = useQueryClient()
  const removing = useMutation({
    mutationFn: () => disconnectGate(gate.id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: gateQuery.queryKey }),
  })

  return (
    <>
      <p>
        {gate.state === 'waiting'
          ? 'It has been given its command and has not been heard from yet.'
          : 'Its command was not run within its quarter of an hour, and opens nothing now.'}
      </p>
      {removing.error && <Problem>{removing.error.message}</Problem>}
      <div className="flex flex-wrap gap-2">
        <Link to="/network/connect" search={{ gate: gate.id }} className={buttonClass('secondary')}>
          {gate.state === 'waiting' ? 'Continue' : 'Connect it again'}
        </Link>
        <Button variant="ghost" busy={removing.isPending} onClick={() => removing.mutate()}>
          Give it up
        </Button>
      </div>
    </>
  )
}

/** What its owner can do with a connected VPS, tucked away: none of it is done often. */
function Manage({ gate }: { gate: Gate }) {
  const queryClient = useQueryClient()
  const keep = (changed: Network) => queryClient.setQueryData(gateQuery.queryKey, changed)
  const checking = useMutation({ mutationFn: () => checkGate(gate.id), onSuccess: keep })
  const renaming = useMutation({ mutationFn: (name: string) => renameGate(gate.id, name), onSuccess: keep })
  const [asking, setAsking] = useState(false)
  const disconnecting = useMutation({
    mutationFn: () => disconnectGate(gate.id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: gateQuery.queryKey })
      setAsking(false)
    },
  })

  return (
    <details className="border-t border-hairline pt-3">
      <summary className="cursor-pointer text-ink">Manage this VPS</summary>
      <div className="mt-4 flex flex-col gap-6">
        <section className="flex flex-col items-start gap-2">
          <h3 className="font-medium text-ink">Player IP addresses</h3>
          <p className="max-w-140 text-small text-ink-subtle">{PLAYERS[gate.player_addresses].sentence}</p>
          {checking.error && <Problem>{checking.error.message}</Problem>}
          <Button busy={checking.isPending} onClick={() => checking.mutate()}>
            Check again
          </Button>
        </section>
        <form
          // What was kept is what the field starts from again.
          key={gate.name}
          className="flex max-w-80 flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault()
            renaming.mutate(String(new FormData(event.currentTarget).get('name') ?? ''))
          }}
        >
          <Field
            label="What to call it"
            name="name"
            maxLength={40}
            autoComplete="off"
            defaultValue={gate.name}
            hint="Where it is, say. Left empty it is called by its address."
          />
          {renaming.error && <Problem>{renaming.error.message}</Problem>}
          <div>
            <Button type="submit" busy={renaming.isPending}>
              Save
            </Button>
          </div>
        </form>
        {gate.reachable && <VpsGuard id={gate.id} />}
        <section>
          <Button onClick={() => setAsking(true)}>Disconnect this VPS</Button>
          <Confirm
            open={asking}
            onClose={() => setAsking(false)}
            title={`Disconnect ${gate.name}?`}
            action={
              <Button variant="danger" busy={disconnecting.isPending} onClick={() => disconnecting.mutate()}>
                Disconnect
              </Button>
            }
          >
            <p>
              Players will no longer reach your servers at <code className="font-mono text-ink">{gate.address}</code>. The
              servers keep running. Those that were reached through it are reached through another VPS if one is connected,
              and on your home network if none is.
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
      </div>
    </details>
  )
}

/** Every port of every server, and where players reach it. */
function Ports({ network }: { network: Network }) {
  const connected = network.gates.some((gate) => gate.state === 'connected')
  const several = network.gates.length > 1
  const cell = 'px-4 font-normal'

  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-section text-ink">{connected ? 'Forwarded ports' : 'Ports'}</h2>
      {network.ports.length === 0 ? (
        <p>No server has a port yet.</p>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-hairline bg-surface-1">
          <table className="w-full text-left whitespace-nowrap">
            <thead>
              <tr className="h-10 border-b border-hairline text-caption text-ink-subtle">
                <th className={cell}>{connected ? 'Public address' : 'Address at home'}</th>
                <th className={cell}>Protocol</th>
                <th className={cell}>Server</th>
                {several && <th className={cell}>Through</th>}
                {connected && <th className={`${cell} text-right`}>Traffic, last day</th>}
              </tr>
            </thead>
            <tbody>
              {network.ports.map((port) => (
                <tr key={port.port} className="h-10 border-b border-hairline last:border-b-0">
                  <td className={cell}>
                    <CopyChip text={addressOf(network, port)} />
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
                  {several && <td className={cell}>{network.gates.find((gate) => gate.id === port.gate_id)?.name ?? 'Home only'}</td>}
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

/**
 * Connect a VPS: three steps, one thing to do in each (DESIGN.md, Key screens).
 * Which VPS it is about is in the address, and which step is open follows from
 * where that VPS stands, so the page can be left and come back to, or opened
 * on another machine, and is where it was.
 */
export function ConnectPage() {
  const network = useNetworkHere()
  const { gate: id } = connect.useSearch()
  // Come to with no VPS named, it is the one that is awaited, if one is.
  const gate = network.gates.find((gate) => (id === undefined ? gate.state === 'waiting' : gate.id === id))
  const at = gate?.state === 'waiting' ? 1 : gate?.state === 'connected' ? 2 : 0

  return (
    <>
      <PageBar title="Connect a VPS" crumb={<Link to="/network">Network</Link>} />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <Steps steps={CONNECT_STEPS} at={at} />
        {!gate || gate.state === 'expired' ? (
          <AddressStep expired={gate} others={network.gates.some((other) => other.state === 'connected')} />
        ) : gate.state === 'waiting' ? (
          <CommandStep gate={gate} />
        ) : (
          <VerifyStep gate={gate} first={network.ports.length === 0} />
        )}
      </main>
    </>
  )
}

function AddressStep({ expired, others }: { expired: Gate | undefined; others: boolean }) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const connecting = useMutation({
    mutationFn: connectGate,
    onSuccess: async (network) => {
      queryClient.setQueryData(gateQuery.queryKey, network)
      // The one that is awaited now is the one that was just asked for.
      const awaited = network.gates.find((gate) => gate.state === 'waiting')
      await navigate({ to: '/network/connect', search: { gate: awaited?.id }, replace: true })
    },
  })

  return (
    <form
      className="flex max-w-140 flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault()
        const form = new FormData(event.currentTarget)
        connecting.mutate({
          address: String(form.get('address') ?? ''),
          name: String(form.get('name') ?? '') || null,
          wg_port: Number(form.get('wg_port')) || null,
        })
      }}
    >
      <p>
        A VPS gives your servers an address that anyone can reach. It only passes traffic on: your worlds, your files and this
        panel stay at home.
        {others && ' Each VPS has a tunnel of its own, and each server is reached through the one you choose for it.'}
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
        defaultValue={expired?.address}
        hint="Its public IPv4 address, or a name that leads to it. This is what players will type."
      />
      <Field
        label="What to call it"
        name="name"
        maxLength={40}
        autoComplete="off"
        placeholder="Frankfurt"
        defaultValue={expired && expired.name !== expired.address ? expired.name : undefined}
        hint="Where it is, say: for telling one VPS from another. Left empty it is called by its address."
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
    mutationFn: () => disconnectGate(gate.id),
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
        {gate.command?.startsWith('curl') ? (
          <>It fetches the Gate program and checks it against Homewarp’s signature before running it.</>
        ) : (
          <>
            It needs the <code className="font-mono">homewarp-gate</code> program on the VPS.
          </>
        )}{' '}
        It sets up the tunnel, a service that keeps it up, and the openings a firewall on the VPS needs. Nothing else
        there is touched.
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

function VerifyStep({ gate, first }: { gate: Gate; first: boolean }) {
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
        {first ? (
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
