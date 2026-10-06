import { queryOptions, useQuery } from '@tanstack/react-query'

import { getGate, type Gate } from './api/client'

/**
 * How often an open page asks after the Gate, in milliseconds: as often as
 * Homewarp itself asks it, and faster while a VPS is being connected, when
 * each step takes a second or two.
 */
export const EVERY_GATE = { steady: 10_000, changing: 2_000 }

/** The Gate, the tunnel to it, and every port of every server. */
export const gateQuery = queryOptions({
  queryKey: ['gate'],
  queryFn: getGate,
  staleTime: 2_000,
})

/** Whether the Gate is in the middle of something that the next few seconds will change. */
export function isChanging(gate: Gate | undefined): boolean {
  return gate?.state === 'waiting' || (gate?.state === 'connected' && gate.player_addresses === 'unchecked' && !gate.note)
}

/**
 * The Gate as it was last heard of, asked again by itself. Nothing waits for
 * it: the sidebar and the address chips are painted before it answers, and
 * every page that uses it shares the one request.
 */
export function useGate(): Gate | undefined {
  const { data } = useQuery({
    ...gateQuery,
    refetchInterval: (query) => (isChanging(query.state.data) ? EVERY_GATE.changing : EVERY_GATE.steady),
  })
  return data
}

/** Where players reach a port: at the VPS once one is connected, on the home network until then. */
export function addressOf(gate: Gate | undefined, port: number): string {
  const host = gate?.state === 'connected' && gate.address ? gate.address : window.location.hostname
  return `${host}:${port}`
}

/** What the Gate is doing, as a pill says it: a word, and the state whose colour and mark it takes. */
export function gateLook(gate: Gate | undefined): { word: string; tone: 'running' | 'starting' | 'crashed' | 'offline' } {
  if (!gate || gate.state === 'none' || gate.state === 'expired') return { word: 'No VPS', tone: 'offline' }
  if (gate.state === 'waiting') return { word: 'Connecting', tone: 'starting' }
  if (!gate.reachable) return { word: 'Gate unreachable', tone: 'crashed' }
  return { word: gate.latency_ms == null ? 'Gate up' : `Gate ${gate.latency_ms} ms`, tone: 'running' }
}
