import { useSuspenseQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { LayoutGrid, Plus } from 'lucide-react'

import { CopyChip, PageBar, StatusPill, buttonClass } from '../components/ui'
import { addressOf, useNetwork } from '../gate'
import { EVERY, serversQuery } from '../servers'
import { useOwner } from '../session'

/** The landing page: every server, what it is doing and where players reach it. */
export function ServersPage() {
  const { data: servers } = useSuspenseQuery({ ...serversQuery, refetchInterval: EVERY.list })
  // Wanted, not needed: the cards are painted with the address at home, and take the VPS's once it is known.
  const network = useNetwork()
  // Making a server is the owner's to do.
  const owner = useOwner()
  // The page's one primary action: in the bar once there are servers, in the empty state until then.
  const newLink = owner && (
    <Link to="/servers/new" className={buttonClass('primary')}>
      <Plus aria-hidden size={16} />
      New server
    </Link>
  )

  return (
    <>
      <PageBar title="Servers">{servers.length > 0 && newLink}</PageBar>
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col p-4 md:p-6">
        {servers.length === 0 ? (
          <section className="flex flex-1 flex-col items-center justify-center py-16 text-center">
            <LayoutGrid aria-hidden size={32} className="mb-3 text-ink-faint" />
            <h2 className="text-section text-ink">No servers yet.</h2>
            <p className="mt-1">
              {owner ? 'Pick a game and Homewarp handles the rest.' : 'The owner of this Homewarp has not let this account into one.'}
            </p>
            <div className="mt-4">{newLink}</div>
          </section>
        ) : (
          <ul className="grid gap-4 md:grid-cols-2 wide:grid-cols-3">
            {servers.map((server) => (
              <li
                key={server.id}
                className="relative flex min-w-0 flex-col gap-3 rounded-lg border border-hairline bg-surface-1 p-4 transition-colors duration-120 ease-out hover:bg-surface-2"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <h2 className="truncate text-body font-medium text-ink">
                      {/* The whole card opens the server. The address below stays a target of its own. */}
                      <Link to="/servers/$serverId" params={{ serverId: String(server.id) }} className="after:absolute after:inset-0">
                        {server.name}
                      </Link>
                    </h2>
                    <p className="truncate text-small">{server.template}</p>
                  </div>
                  <StatusPill state={server.state} />
                </div>
                <div className="relative flex flex-wrap items-center gap-3 self-start">
                  <CopyChip text={addressOf(network, server)} />
                  {server.players && (
                    <span className="text-small text-ink-subtle">
                      {server.players.online} of {server.players.max} on it
                    </span>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}
      </main>
    </>
  )
}
