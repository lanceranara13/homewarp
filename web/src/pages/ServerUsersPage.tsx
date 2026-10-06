import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, getRouteApi } from '@tanstack/react-router'
import { useState } from 'react'

import { PERMISSIONS, accountsQuery, serverUsersQuery } from '../accounts'
import { letIn, turnOut, type Permission } from '../api/client'
import { Button, Problem, Select } from '../components/ui'

const route = getRouteApi('/shell/servers/$serverId')

/**
 * Who besides the owner is let into this server, and what each may do there
 * (DESIGN.md, Inside a server). Being let in is being let look: its state, its
 * console, what it uses. The rest is said one thing at a time.
 */
export function ServerUsersTab() {
  const { serverId } = route.useParams()
  const id = Number(serverId)
  const queryClient = useQueryClient()
  const { data: inside } = useSuspenseQuery(serverUsersQuery(id))
  const { data: accounts } = useSuspenseQuery(accountsQuery)
  const [picked, setPicked] = useState('')
  const outside = accounts.filter((account) => !account.owner && !inside.some((user) => user.user_id === account.id))

  // Null for `permissions` is out of the server altogether.
  const changing = useMutation({
    mutationFn: ({ user, permissions }: { user: number; permissions: Permission[] | null }) =>
      permissions === null ? turnOut(id, user) : letIn(id, user, permissions),
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: serverUsersQuery(id).queryKey }),
        // How many servers each account is in is told on the Settings page.
        queryClient.invalidateQueries({ queryKey: accountsQuery.queryKey }),
      ]),
  })
  const busyWith = changing.isPending ? changing.variables.user : null

  return (
    <>
      <p className="max-w-140 text-small text-ink-subtle">
        An account that is let in here sees this server among its own: what it is doing, its console, what it uses of the
        machine. Tick what it may do besides. The owner of this Homewarp is in every server, and is not in this list.
      </p>
      {changing.error && <Problem>{changing.error.message}</Problem>}

      {inside.length > 0 && (
        <ul className="flex flex-col gap-4">
          {inside.map((user) => (
            <li key={user.user_id} className="rounded-lg border border-hairline bg-surface-1 p-4">
              <div className="flex items-center justify-between gap-3">
                <h2 className="truncate text-body font-medium text-ink">{user.username}</h2>
                <Button variant="ghost" disabled={busyWith === user.user_id} onClick={() => changing.mutate({ user: user.user_id, permissions: null })}>
                  Take out
                </Button>
              </div>
              <fieldset disabled={busyWith === user.user_id} className="mt-3 grid gap-x-6 gap-y-3 md:grid-cols-2 wide:grid-cols-3">
                <legend className="sr-only">What {user.username} may do here</legend>
                {PERMISSIONS.map(({ name, label, means }) => (
                  <label key={name} className="flex items-start gap-2.5">
                    <input
                      type="checkbox"
                      checked={user.permissions.includes(name)}
                      onChange={(event) =>
                        changing.mutate({
                          user: user.user_id,
                          permissions: event.currentTarget.checked
                            ? [...user.permissions, name]
                            : user.permissions.filter((had) => had !== name),
                        })
                      }
                      className="mt-0.5 size-4 shrink-0 accent-accent"
                    />
                    <span>
                      <span className="block font-medium text-ink">{label}</span>
                      <span className="block text-small text-ink-subtle">{means}</span>
                    </span>
                  </label>
                ))}
              </fieldset>
            </li>
          ))}
        </ul>
      )}

      {outside.length > 0 ? (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            if (!picked) return
            // Let in to look. What more it may do is ticked once it is in the list.
            changing.mutate({ user: Number(picked), permissions: [] }, { onSuccess: () => setPicked('') })
          }}
        >
          <Select label="Let an account in" value={picked} onChange={(event) => setPicked(event.currentTarget.value)} className="md:w-56">
            <option value="">Choose an account</option>
            {outside.map((account) => (
              <option key={account.id} value={account.id}>
                {account.username}
              </option>
            ))}
          </Select>
          <Button type="submit" disabled={!picked || changing.isPending}>
            Let in
          </Button>
        </form>
      ) : (
        <p className="text-small text-ink-subtle">
          {inside.length > 0 ? 'Every account is in this server. ' : 'There is no account to let in yet. '}
          Accounts are made in{' '}
          <Link to="/settings" className="text-accent hover:underline">
            Settings
          </Link>
          .
        </p>
      )}
    </>
  )
}
