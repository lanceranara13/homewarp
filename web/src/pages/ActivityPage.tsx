import { useQuery, useSuspenseInfiniteQuery } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { ScrollText } from 'lucide-react'

import { accountsQuery, activityQuery } from '../accounts'
import { Button, PageBar, Select } from '../components/ui'
import { when } from '../format'
import { WORDS } from '../happened'
import { serversQuery } from '../servers'

const route = getRouteApi('/shell/activity')

/** What was done through the panel, by whom and to which server: the owner's to read (DESIGN.md, Global navigation). */
export function ActivityPage() {
  const { server, user } = route.useSearch()
  const navigate = useNavigate()
  const log = useSuspenseInfiniteQuery(activityQuery(server, user))
  // Wanted, not needed: the lines are painted first, and what to narrow them by comes when it comes.
  const servers = useQuery(serversQuery)
  const accounts = useQuery(accountsQuery)
  const lines = log.data.pages.flat()
  const narrow = (to: { server?: number; user?: number }) => void navigate({ to: '/activity', search: to })
  const chosen = (value: string) => Number(value) || undefined

  return (
    <>
      <PageBar title="Activity" />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <div className="flex flex-wrap gap-3">
          <Select
            label="Server"
            value={server ?? ''}
            onChange={(event) => narrow({ server: chosen(event.currentTarget.value), user })}
            className="md:w-56"
          >
            <option value="">Every server</option>
            {servers.data?.map((one) => (
              <option key={one.id} value={one.id}>
                {one.name}
              </option>
            ))}
          </Select>
          <Select
            label="Account"
            value={user ?? ''}
            onChange={(event) => narrow({ server, user: chosen(event.currentTarget.value) })}
            className="md:w-56"
          >
            <option value="">Everyone</option>
            {accounts.data?.map((one) => (
              <option key={one.id} value={one.id}>
                {one.username}
              </option>
            ))}
          </Select>
        </div>

        {lines.length === 0 ? (
          <section className="flex flex-1 flex-col items-center justify-center py-16 text-center">
            <ScrollText aria-hidden size={32} className="mb-3 text-ink-faint" />
            <h2 className="text-section text-ink">Nothing has been written down.</h2>
            <p className="mt-1">What is done through the panel is kept here for half a year.</p>
          </section>
        ) : (
          <ul className="overflow-hidden rounded-lg border border-hairline bg-surface-1">
            {lines.map((line) => (
              <li
                key={line.id}
                className="flex flex-col gap-0.5 border-b border-hairline px-4 py-2.5 last:border-b-0 md:flex-row md:items-baseline md:gap-4"
              >
                <span className="shrink-0 text-small text-ink-subtle tabular-nums md:w-44">{when(line.at)}</span>
                <span className="shrink-0 truncate font-medium text-ink md:w-32">{line.username}</span>
                <span className="min-w-0 flex-1">
                  {WORDS[line.action] ?? line.action}
                  {line.server && (
                    <>
                      {' · '}
                      {line.server_id !== null && line.server_id !== undefined && servers.data?.some((one) => one.id === line.server_id) ? (
                        <Link to="/servers/$serverId" params={{ serverId: String(line.server_id) }} className="text-accent hover:underline">
                          {line.server}
                        </Link>
                      ) : (
                        // A server that has since been removed has a name and nothing to lead to.
                        line.server
                      )}
                    </>
                  )}
                  {line.detail && <code className="ml-2 font-mono text-mono wrap-anywhere text-ink-muted">{line.detail}</code>}
                </span>
              </li>
            ))}
          </ul>
        )}
        {log.hasNextPage && (
          <div>
            <Button busy={log.isFetchingNextPage} onClick={() => void log.fetchNextPage()}>
              Older
            </Button>
          </div>
        )}
      </main>
    </>
  )
}
