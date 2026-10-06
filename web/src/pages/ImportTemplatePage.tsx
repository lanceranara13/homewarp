import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { useId, useState } from 'react'

import { importTemplate } from '../api/client'
import { Button, PageBar, Problem, buttonClass } from '../components/ui'
import { templateQuery, templatesQuery } from '../templates'

/** The server takes no more than this, and a file that is larger is not an egg. */
const LARGEST_EGG = 1 << 20

/** Takes an egg, from a file or pasted, and opens the template it becomes. */
export function ImportTemplatePage() {
  const id = useId()
  const [egg, setEgg] = useState('')
  const [tooLarge, setTooLarge] = useState(false)
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const importing = useMutation({
    mutationFn: importTemplate,
    onSuccess: async (template) => {
      // The page this opens has its answer already, and the gallery asks afresh when next shown.
      queryClient.setQueryData(templateQuery(template.id).queryKey, template)
      queryClient.removeQueries({ queryKey: templatesQuery.queryKey, exact: true })
      await navigate({ to: '/templates/$templateId', params: { templateId: String(template.id) } })
    },
  })

  /** Puts the file's text where a pasted egg would be, so what is sent is what is shown. */
  async function read(file: File | undefined) {
    if (!file) return
    setTooLarge(file.size > LARGEST_EGG)
    if (file.size <= LARGEST_EGG) setEgg(await file.text())
  }

  return (
    <>
      <PageBar title="Import" crumb={<Link to="/templates">Templates</Link>} />
      <main className="mx-auto w-full max-w-300 flex-1 p-4 md:p-6">
        <form
          className="flex max-w-140 flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            importing.mutate(egg)
          }}
        >
          <p>Homewarp reads the eggs that Pterodactyl and Pelican export, as JSON or YAML.</p>
          <div className="flex flex-col gap-1.5">
            <label htmlFor={`${id}-file`} className="text-caption text-ink-subtle">
              Egg file
            </label>
            <input
              id={`${id}-file`}
              type="file"
              accept=".json,.yaml,.yml"
              onChange={(event) => void read(event.currentTarget.files?.[0])}
              className="text-small file:mr-3 file:h-10 file:rounded-md file:border file:border-hairline-strong file:bg-surface-1 file:px-3 file:text-body file:font-medium file:text-ink md:file:h-8"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor={`${id}-egg`} className="text-caption text-ink-subtle">
              Or paste it here
            </label>
            <textarea
              id={`${id}-egg`}
              required
              rows={12}
              spellCheck={false}
              value={egg}
              onChange={(event) => {
                setEgg(event.currentTarget.value)
                setTooLarge(false)
              }}
              aria-describedby={`${id}-hint`}
              className="rounded-md border border-hairline-strong bg-surface-1 p-2.5 font-mono text-mono text-ink"
            />
            <p id={`${id}-hint`} className="text-small text-ink-subtle">
              A template's install script and startup command run on this machine, inside containers. Import eggs only
              from a source you trust.
            </p>
          </div>
          {tooLarge ? (
            <Problem>That file is over 1 MB, which is more than an egg is.</Problem>
          ) : (
            importing.error && <Problem>{importing.error.message}</Problem>
          )}
          <div className="flex gap-2">
            <Button type="submit" variant="primary" busy={importing.isPending}>
              Import template
            </Button>
            <Link to="/templates" className={buttonClass('ghost')}>
              Cancel
            </Link>
          </div>
        </form>
      </main>
    </>
  )
}
