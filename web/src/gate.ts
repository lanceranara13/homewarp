import { queryOptions, useQuery } from '@tanstack/react-query'

import { getGuard, getNetwork, getPanel, getTraffic, type Gate, type Network } from './api/client'

/**
 * How often an open page asks after the VPSes, in milliseconds: as often as
 * Homewarp itself asks them, and faster while one is being connected, when
 * each step takes a second or two.
 */
export const EVERY_GATE = { steady: 10_000, changing: 2_000 }

/** Every VPS, the tunnel to each, and every port of every server. */
export const gateQuery = queryOptions({
  queryKey: ['gates'],
  queryFn: getNetwork,
  staleTime: 2_000,
})

/** What passes through each tunnel now. Only the owner's Network page asks, for as long as it is open. */
export const trafficQuery = queryOptions({
  queryKey: ['gates', 'traffic'],
  queryFn: getTraffic,
  staleTime: 1_000,
})

/** The name the panel is reached by from the internet, and its certificate. Only the owner's page asks. */
export const panelQuery = queryOptions({
  queryKey: ['panel'],
  queryFn: getPanel,
  staleTime: 2_000,
})

/** The guard on one VPS itself. Only the owner's page asks, and only of a VPS that answers. */
export function guardQuery(id: number) {
  return queryOptions({
    queryKey: ['gates', id, 'guard'],
    queryFn: () => getGuard(id),
    staleTime: 1_000,
  })
}

/** Whether a VPS is in the middle of something that the next few seconds will change. */
export function isChanging(gate: Gate | undefined): boolean {
  return gate?.state === 'waiting' || (gate?.state === 'connected' && gate.player_addresses === 'unchecked' && !gate.note)
}

/** How often to ask again: faster while any VPS is changing. */
export function everyGate(network: Network | undefined): number {
  return network?.gates.some(isChanging) ? EVERY_GATE.changing : EVERY_GATE.steady
}

/**
 * The VPSes as they were last heard of, asked again by itself. Nothing waits
 * for it: the sidebar and the address chips are painted before it answers, and
 * every page that uses it shares the one request.
 */
export function useNetwork(): Network | undefined {
  const { data } = useQuery({ ...gateQuery, refetchInterval: (query) => everyGate(query.state.data) })
  return data
}

/** The VPS a server is reached through, once it is connected. */
export function gateOf(network: Network | undefined, server: { gate_id?: number | null }): Gate | undefined {
  return network?.gates.find((gate) => gate.id === server.gate_id && gate.state === 'connected')
}

/** Where players reach a server's port: at its VPS once it has one, on the home network until then. */
export function addressOf(network: Network | undefined, server: { port: number; gate_id?: number | null }): string {
  return `${gateOf(network, server)?.address ?? window.location.hostname}:${server.port}`
}

type Look = { word: string; tone: 'running' | 'starting' | 'crashed' | 'offline' }

/** What one VPS is doing, as a pill says it: a word, and the state whose colour and mark it takes. */
export function lookOf(gate: Gate): Look {
  if (gate.state === 'expired') return { word: 'Not connected', tone: 'offline' }
  if (gate.state === 'waiting') return { word: 'Connecting', tone: 'starting' }
  if (!gate.reachable) return { word: 'Unreachable', tone: 'crashed' }
  return { word: gate.latency_ms == null ? 'Up' : `${gate.latency_ms} ms`, tone: 'running' }
}

/** The same for all of them at once, as the sidebar's one mark says it. */
export function gateLook(network: Network | undefined): Look {
  const connected = network?.gates.filter((gate) => gate.state === 'connected') ?? []
  if (connected.length === 0) {
    return network?.gates.some((gate) => gate.state === 'waiting') ? { word: 'Connecting', tone: 'starting' } : { word: 'No VPS', tone: 'offline' }
  }
  const down = connected.filter((gate) => !gate.reachable).length
  if (connected.length === 1) {
    const [gate] = connected
    if (down > 0) return { word: 'Gate unreachable', tone: 'crashed' }
    return { word: gate?.latency_ms == null ? 'Gate up' : `Gate ${gate.latency_ms} ms`, tone: 'running' }
  }
  if (down > 0) return { word: `${down} of ${connected.length} VPSes unreachable`, tone: 'crashed' }
  return { word: `${connected.length} VPSes up`, tone: 'running' }
}
