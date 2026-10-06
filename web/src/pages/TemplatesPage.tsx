import { useSuspenseQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Blocks, Plus } from 'lucide-react'

import { PageBar, buttonClass } from '../components/ui'
import { templatesQuery } from '../templates'

/** The games and apps this Homewarp knows how to install and run. */
export function TemplatesPage() {
  const { data: templates } = useSuspenseQuery(templatesQuery)
  // The page's one primary action: in the bar once there is a gallery, in the empty state until then.
  const importLink = (
    <Link to="/templates/import" className={buttonClass('primary')}>
      <Plus aria-hidden size={16} />
      Import template
    </Link>
  )

  return (
    <>
      <PageBar title="Templates">{templates.length > 0 && importLink}</PageBar>
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col p-4 md:p-6">
        {templates.length === 0 ? (
          <section className="flex flex-1 flex-col items-center justify-center py-16 text-center">
            <Blocks aria-hidden size={32} className="mb-3 text-ink-faint" />
            <h2 className="text-section text-ink">No templates yet.</h2>
            <p className="mt-1">Import an egg from Pterodactyl or Pelican, and Homewarp can run that game.</p>
            <div className="mt-4">{importLink}</div>
          </section>
        ) : (
          <ul className="grid gap-4 md:grid-cols-2 wide:grid-cols-3">
            {templates.map((template) => (
              <li key={template.id} className="min-w-0">
                <Link
                  to="/templates/$templateId"
                  params={{ templateId: String(template.id) }}
                  className="flex h-full flex-col gap-1 rounded-lg border border-hairline bg-surface-1 p-4 transition-colors duration-120 ease-out hover:bg-surface-2"
                >
                  <h2 className="truncate text-body font-medium text-ink">{template.name}</h2>
                  <p className="line-clamp-2 text-small">{template.description || 'No description.'}</p>
                  <p className="mt-auto truncate pt-2 font-mono text-mono text-ink-subtle">{template.image}</p>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </main>
    </>
  )
}
