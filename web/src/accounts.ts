import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query'

import {
  getSettings,
  getUpdate,
  getTwoSteps,
  listAccounts,
  listActivity,
  listPasskeys,
  listServerUsers,
  listWebhooks,
  type Permission,
} from './api/client'

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

/** The passkeys of the account signed in here. */
export const passkeysQuery = queryOptions({
  queryKey: ['passkeys'],
  queryFn: listPasskeys,
  staleTime: 5_000,
})

/** Whether the account signed in here has a second step to its sign-in. */
export const twoStepsQuery = queryOptions({
  queryKey: ['two-steps'],
  queryFn: getTwoSteps,
  staleTime: 5_000,
})

/** What is set for this Homewarp as a whole. Only the owner's page asks. */
export const settingsQuery = queryOptions({
  queryKey: ['settings'],
  queryFn: getSettings,
  staleTime: 5_000,
})

/** The addresses that are told what happens, and everything one can be told of. Only the owner's page asks. */
export const webhooksQuery = queryOptions({
  queryKey: ['webhooks'],
  queryFn: listWebhooks,
  staleTime: 5_000,
})

/** Which version of Homewarp runs, and whether a newer one is out. Only the owner's page asks. */
export const updateQuery = queryOptions({
  queryKey: ['update'],
  queryFn: getUpdate,
  staleTime: 2_000,
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
  { name: 'backups', label: 'Backups', means: 'Make backups, and put one back. Downloading one needs Files too' },
  { name: 'schedules', label: 'Schedules', means: 'Set what it does by the clock' },
  { name: 'settings', label: 'Settings', means: 'Change its name, its image and what its template leaves to users' },
]

export type SettingsGroup = 'account' | 'security' | 'users' | 'backups' | 'system' | 'updates'

/**
 * What the Settings page is divided into, in the order it is listed in: an
 * account's own two groups first, then the owner's, which are about this
 * Homewarp as a whole.
 */
export const SETTINGS_GROUPS: { name: SettingsGroup; label: string; owners?: true }[] = [
  { name: 'account', label: 'Account' },
  { name: 'security', label: 'Security' },
  { name: 'users', label: 'Users', owners: true },
  { name: 'backups', label: 'Backups', owners: true },
  { name: 'system', label: 'System', owners: true },
  { name: 'updates', label: 'Updates', owners: true },
]
