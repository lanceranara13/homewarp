import { queryOptions } from '@tanstack/react-query'

import { getServer, listBackups, listSchedules, listServers } from './api/client'

/**
 * How often an open page asks again, in milliseconds. What a server is doing
 * and what it prints change by themselves, and until the panel has its socket
 * (PLAN.md §5.8) asking is how a page finds out.
 */
export const EVERY = { list: 4_000, server: 1_500 }

/** Every server: the one request the Servers page waits for. */
export const serversQuery = queryOptions({
  queryKey: ['servers'],
  queryFn: listServers,
  staleTime: 2_000,
})

/** A server's backups and how many are kept. Under the server's own key, to go when it goes. */
export function backupsQuery(id: number) {
  return queryOptions({
    queryKey: ['servers', id, 'backups'],
    queryFn: () => listBackups(id),
    staleTime: 1_000,
  })
}

/** What a server does by the clock. */
export function schedulesQuery(id: number) {
  return queryOptions({
    queryKey: ['servers', id, 'schedules'],
    queryFn: () => listSchedules(id),
    staleTime: 1_000,
  })
}

/** One server with the end of its console, or null where there is none. */
export function serverQuery(id: number) {
  return queryOptions({
    queryKey: ['servers', id],
    queryFn: () => getServer(id),
    staleTime: 1_000,
  })
}
