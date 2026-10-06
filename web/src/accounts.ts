import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query'

import { getSettings, listAccounts, listActivity, listServerUsers, type Permission } from './api/client'

/** How many lines of what was done Homewarp answers with at a time. Fewer than that, and there are no older ones. */
const PAGE = 100

/**
 * What was done through the panel, the newest first: to one server, by one
 * account, or all of it. Each page after the first is what is older than the
 * last line of the one before.
 */
export function activityQuery(server: number | undefined, user: number | undefined) {
  return infiniteQueryOptions({
    queryKey: ['activity', server ?? null, user ?? null],
    queryFn: ({ pageParam }) => listActivity({ server, user, before: pageParam }),
    initialPageParam: undefined as number | undefined,
    getNextPageParam: (last) => (last.length === PAGE ? last.at(-1)?.id : undefined),
    staleTime: 2_000,
  })
}

/** Every account, the owner's first. Only the owner's pages ask. */
export const accountsQuery = queryOptions({
  queryKey: ['accounts'],
  queryFn: listAccounts,
  staleTime: 5_000,
})

/** What is set for this Homewarp as a whole. Only the owner's page asks. */
export const settingsQuery = queryOptions({
  queryKey: ['settings'],
  queryFn: getSettings,
  staleTime: 5_000,
})

/** The accounts that have been let into a server. Under the server's own key, to go when it goes. */
export function serverUsersQuery(id: number) {
  return queryOptions({
    queryKey: ['servers', id, 'users'],
    queryFn: () => listServerUsers(id),
    staleTime: 5_000,
  })
}

/** What an account may be let do with a server beyond looking at it, in the order it is always told in. */
export const PERMISSIONS: { name: Permission; label: string; means: string }[] = [
  { name: 'console', label: 'Console', means: 'Type commands into it' },
  { name: 'power', label: 'Power', means: 'Start, stop and kill it' },
  { name: 'files', label: 'Files', means: 'Read, change, upload and delete its files' },
  { name: 'backups', label: 'Backups', means: 'Make backups, and put one back' },
  { name: 'schedules', label: 'Schedules', means: 'Set what it does by the clock' },
  { name: 'settings', label: 'Settings', means: 'Change what it is made of' },
]
