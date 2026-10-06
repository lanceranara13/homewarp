import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { useId, useRef, useState } from 'react'

import { fetchEgg, importTemplate, refreshCatalogue, type Catalogue } from '../api/client'
import { Button, Field, PageBar, Problem, buttonClass } from '../components/ui'
import { when } from '../format'
import { catalogueQuery, templateQuery, templatesQuery } from '../templates'

const route = getRouteApi('/shell/templates/import')

/** The server takes no more than this, and a file that is larger is not an egg. */
const LARGEST_EGG = 1 << 20
/** No more of the catalogue than this is put on the page at once: the rest is found by typing. */
const SHOWN = 60

/** Takes an egg, from a file, pasted, or fetched from where it is published, and opens the template it becomes. */
export function ImportTemplatePage() {
  const id = useId()
  const [egg, setEgg] = useState('')
  const [tooLarge, setTooLarge] = useState(false)
  const text = useRef<HTMLTextAreaElement>(null)
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  // Come to from the new-server wizard: the egg is on the way to being a server.
  const forServer = route.useSearch().then === 'server'
  const importing = useMutation({
    mutationFn: importTemplate,
    onSuccess: async (template) => {
      // The page this opens has its answer already, and the gallery asks afresh when next shown.
      queryClient.setQueryData(templateQuery(template.id).queryKey, template)
      queryClient.removeQueries({ queryKey: templatesQuery.queryKey, exact: true })
      const params = { templateId: String(template.id) }
      await navigate(forServer ? { to: '/servers/new/$templateId', params } : { to: '/templates/$templateId', params })
    },
  })
  // Fetched is not imported: the text is put where a pasted egg would be, to be read first.
  const fetching = useMutation({
    mutationFn: fetchEgg,
    onSuccess: (fetched) => {
      setEgg(fetched)
      setTooLarge(false)
      text.current?.scrollIntoView({ block: 'center' })
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
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-8 p-4 md:p-6">
        <form
          className="flex max-w-140 flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            importing.mutate(egg)
          }}
        >
          <p>
            Homewarp reads the eggs that Pterodactyl and Pelican export, as JSON or YAML.
            {forServer && ' Pick one from the catalogue below, or bring your own. Once it is imported you go on to setting the server up.'}
          </p>
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
          <FromAnAddress busy={fetching.isPending} onFetch={(url) => fetching.mutate(url)} />
          {fetching.error && <Problem>{fetching.error.message}</Problem>}
          <div className="flex flex-col gap-1.5">
            <label htmlFor={`${id}-egg`} className="text-caption text-ink-subtle">
              Or paste it here
            </label>
            <textarea
              ref={text}
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
              from a source you trust, and read what was fetched before you import it.
            </p>
          </div>
          {tooLarge ? (
            <Problem>That file is over 1 MB, which is more than an egg is.</Problem>
          ) : (
            importing.error && <Problem>{importing.error.message}</Problem>
          )}
          <div className="flex gap-2">
            <Button type="submit" variant="primary" busy={importing.isPending}>
              {forServer ? 'Import, and set the server up' : 'Import template'}
            </Button>
            <Link to={forServer ? '/servers/new' : '/templates'} className={buttonClass('ghost')}>
              Cancel
            </Link>
          </div>
        </form>
        <FromTheCatalogue busy={fetching.isPending} onFetch={(url) => fetching.mutate(url)} />
      </main>
    </>
  )
}

/** An address to fetch an egg from. Not a form of its own: it sits inside the one that imports. */
function FromAnAddress({ busy, onFetch }: { busy: boolean; onFetch: (url: string) => void }) {
  const [url, setUrl] = useState('')
  return (
    <div className="flex items-end gap-2">
      <div className="min-w-0 flex-1">
        <Field
          label="Or fetch it from an address"
          mono
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          placeholder="https://raw.githubusercontent.com/…/egg-paper.yaml"
          value={url}
          onChange={(event) => setUrl(event.currentTarget.value)}
          onKeyDown={(event) => {
            // Enter here fetches. It does not import what is in the box below.
            if (event.key === 'Enter') {
              event.preventDefault()
              if (url.trim()) onFetch(url)
            }
          }}
        />
      </div>
      <Button busy={busy} disabled={!url.trim()} onClick={() => onFetch(url)}>
        Fetch
      </Button>
    </div>
  )
}

/** One egg of the catalogue, as a line says it. */
function matches(egg: Catalogue['eggs'][number], words: string[]): boolean {
  const line = `${egg.name} ${egg.kind} ${egg.folder}`.toLowerCase()
  return words.every((word) => line.includes(word))
}

/**
 * The eggs the Pelican community publishes, to pick one from. Nothing on the
 * page waits for this, and nothing is fetched from the internet until asked.
 */
function FromTheCatalogue({ busy, onFetch }: { busy: boolean; onFetch: (url: string) => void }) {
  const queryClient = useQueryClient()
  const { data: catalogue } = useQuery(catalogueQuery)
  const refreshing = useMutation({
    mutationFn: refreshCatalogue,
    onSuccess: (fresh) => queryClient.setQueryData(catalogueQuery.queryKey, fresh),
  })
  const [typed, setTyped] = useState('')

  if (!catalogue) return null
  const words = typed.toLowerCase().split(/\s+/).filter(Boolean)
  const found = catalogue.eggs.filter((egg) => matches(egg, words))

  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-section text-ink">From the catalogue</h2>
      <p className="max-w-140 text-small text-ink-subtle">
        The eggs the Pelican community publishes, for hundreds of games and programs. Homewarp ships none of them: one is
        fetched from its authors when you pick it, and shown above for you to read.{' '}
        {catalogue.fetched_at
          ? `${catalogue.eggs.length} are listed, as of ${when(catalogue.fetched_at)}.`
          : 'The list has not been fetched yet.'}
      </p>
      <div className="flex flex-wrap items-end gap-2">
        {catalogue.eggs.length > 0 && (
          <div className="w-full max-w-80">
            <Field
              label="Find"
              autoComplete="off"
              spellCheck={false}
              placeholder="paper, valheim, steamcmd…"
              value={typed}
              onChange={(event) => setTyped(event.currentTarget.value)}
            />
          </div>
        )}
        <Button busy={refreshing.isPending} onClick={() => refreshing.mutate()}>
          {catalogue.fetched_at ? 'Fetch the list again' : 'Fetch the list'}
        </Button>
      </div>
      {refreshing.error && <Problem>{refreshing.error.message}</Problem>}
      {catalogue.eggs.length > 0 && (
        <>
          <ul className="max-w-140 rounded-lg border border-hairline bg-surface-1">
            {found.slice(0, SHOWN).map((egg) => (
              <li key={egg.url} className="flex items-center gap-3 border-b border-hairline px-4 py-1.5 last:border-b-0">
                <div className="min-w-0 flex-1">
                  <div className="truncate font-medium text-ink">{egg.name}</div>
                  <div className="truncate text-small text-ink-subtle">
                    {egg.kind}
                    {egg.folder && ` · ${egg.folder}`}
                  </div>
                </div>
                <Button variant="ghost" disabled={busy} aria-label={`Fetch ${egg.name}`} onClick={() => onFetch(egg.url)}>
                  Fetch
                </Button>
              </li>
            ))}
          </ul>
          <p className="text-small text-ink-subtle">
            {found.length === 0
              ? 'None is called that.'
              : found.length > SHOWN
                ? `${SHOWN} of ${found.length}. Type more of a name to find the rest.`
                : `${found.length} ${found.length === 1 ? 'egg' : 'eggs'}.`}
          </p>
        </>
      )}
    </section>
  )
}
