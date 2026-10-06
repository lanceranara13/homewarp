import { queryOptions, useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'

import { getSession, signOut, type Session } from './api/client'

/**
 * Who is signed in and whether setup is still to do: the one request a page
 * waits for before it paints. The route guards and the shell read the same copy.
 */
export const sessionQuery = queryOptions({
  queryKey: ['session'],
  queryFn: getSession,
  staleTime: 60_000,
})

/**
 * Whether the account signed in here owns this Homewarp. The owner may do
 * everything and makes the other accounts; they see the servers they have been
 * let into. For the pages inside the shell, where the session is known.
 */
export function useOwner(): boolean {
  return useSuspenseQuery(sessionQuery).data.user?.owner ?? false
}

/** For a request that ends signed in (setup, sign-in): keeps its answer and opens the panel. */
export function useEnter<Input>(request: (input: Input) => Promise<Session>) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  return useMutation({
    mutationFn: request,
    onSuccess: async (session) => {
      queryClient.setQueryData(sessionQuery.queryKey, session)
      await navigate({ to: '/' })
    },
  })
}

export function useSignOut() {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  return useMutation({
    mutationFn: signOut,
    onSuccess: async () => {
      queryClient.setQueryData(sessionQuery.queryKey, (session) => session && { ...session, user: null })
      await navigate({ to: '/login' })
    },
  })
}
