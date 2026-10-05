import { LayoutGrid, Plus } from 'lucide-react'

import { Button, PageBar } from '../components/ui'

/**
 * The landing page. There is nothing to list until Phase 2 brings templates and
 * the server runtime into Core, so for now it is its own empty state.
 */
export function ServersPage() {
  return (
    <>
      <PageBar title="Servers" />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col p-4 md:p-6">
        <section className="flex flex-1 flex-col items-center justify-center py-16 text-center">
          <LayoutGrid aria-hidden size={32} className="mb-3 text-ink-faint" />
          <h2 className="text-section text-ink">No servers yet.</h2>
          <p className="mt-1">Pick a game and Homewarp handles the rest.</p>
          <Button variant="primary" disabled className="mt-4">
            <Plus aria-hidden size={16} />
            New server
          </Button>
          <p className="mt-2 text-small text-ink-subtle">Creating servers is not built yet.</p>
        </section>
      </main>
    </>
  )
}
