import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { getRouteApi } from '@tanstack/react-router'
import { Trash2 } from 'lucide-react'
import { useState } from 'react'

import { findMods, installMod, modReleases, removeFiles, type FoundMod, type ModRelease } from '../api/client'
import { Button, Confirm, Field, Problem, Select } from '../components/ui'
import { filesQuery } from '../files'
import { bytes } from '../format'

const route = getRouteApi('/shell/servers/$serverId')

/** What loads mods or plugins, and the folder each of them reads. */
const LOADERS = [
  { value: 'paper', label: 'Paper', folder: 'plugins' },
  { value: 'purpur', label: 'Purpur', folder: 'plugins' },
  { value: 'folia', label: 'Folia', folder: 'plugins' },
  { value: 'spigot', label: 'Spigot', folder: 'plugins' },
  { value: 'bukkit', label: 'Bukkit', folder: 'plugins' },
  { value: 'velocity', label: 'Velocity', folder: 'plugins' },
  { value: 'bungeecord', label: 'BungeeCord', folder: 'plugins' },
  { value: 'waterfall', label: 'Waterfall', folder: 'plugins' },
  { value: 'fabric', label: 'Fabric', folder: 'mods' },
  { value: 'quilt', label: 'Quilt', folder: 'mods' },
  { value: 'forge', label: 'Forge', folder: 'mods' },
  { value: 'neoforge', label: 'NeoForge', folder: 'mods' },
]

/** What a server runs, as far as finding mods for it goes. */
type Runs = { loader: string; game: string }

const COUNT = new Intl.NumberFormat(undefined, { notation: 'compact' })

/** What was chosen for this server the last time, in this browser. A convenience, and nothing depends on it. */
function remembered(id: number): Runs {
  try {
    const kept = JSON.parse(window.localStorage.getItem(`homewarp.mods.${id}`) ?? 'null') as Partial<Runs> | null
    if (kept && LOADERS.some(({ value }) => value === kept.loader)) return { loader: kept.loader ?? 'paper', game: kept.game ?? '' }
  } catch {
    // No storage in this browser, or something else's in it.
  }
  return { loader: 'paper', game: '' }
}

function remember(id: number, runs: Runs) {
  try {
    window.localStorage.setItem(`homewarp.mods.${id}`, JSON.stringify(runs))
  } catch {
    // Not kept, then.
  }
}

/**
 * Mods and plugins for a Minecraft server: what is installed, and what
 * Modrinth has for what the server runs. Homewarp asks Modrinth; this page
 * talks to Homewarp alone.
 */
export function ModsTab() {
  const { serverId } = route.useParams()
  const id = Number(serverId)
  const [runs, setRuns] = useState(() => remembered(id))
  const [typed, setTyped] = useState('')
  // What was last looked for. Nothing is asked of Modrinth before somebody asks.
  const [asked, setAsked] = useState<(Runs & { query: string }) | null>(null)
  const found = useQuery({
    queryKey: ['servers', id, 'mods', asked],
    queryFn: () => findMods(id, asked ?? { ...runs, query: '' }),
    enabled: asked !== null,
    staleTime: 60_000,
  })
  const folder = LOADERS.find(({ value }) => value === runs.loader)?.folder ?? 'plugins'

  return (
    <>
      <p className="max-w-140 text-small text-ink-subtle">
        A mod or a plugin is somebody else's program. Installed, it runs with the server, inside the server's container,
        and can read and change the server's files. Install what you trust. Each is fetched from Modrinth and checked
        against the checksum Modrinth gives for it, and is loaded when the server next starts.
      </p>
      <Installed id={id} folder={folder} />
      <section className="flex flex-col gap-3">
        <h2 className="text-section text-ink">Find on Modrinth</h2>
        <form
          className="flex flex-wrap items-end gap-3"
          onSubmit={(event) => {
            event.preventDefault()
            remember(id, runs)
            setAsked({ ...runs, query: typed.trim() })
          }}
        >
          <Select
            label="The server runs"
            value={runs.loader}
            onChange={(event) => setRuns({ ...runs, loader: event.currentTarget.value })}
          >
            {LOADERS.map(({ value, label }) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </Select>
          <div className="w-32">
            <Field
              label="Minecraft version"
              mono
              autoComplete="off"
              spellCheck={false}
              placeholder="1.21.1"
              value={runs.game}
              onChange={(event) => setRuns({ ...runs, game: event.currentTarget.value.trim() })}
            />
          </div>
          <div className="min-w-48 flex-1">
            <Field
              label="Look for"
              autoComplete="off"
              spellCheck={false}
              placeholder="Leave empty for the most downloaded"
              value={typed}
              onChange={(event) => setTyped(event.currentTarget.value)}
            />
          </div>
          <Button type="submit" variant="primary" busy={found.isFetching}>
            Search
          </Button>
        </form>
        {found.error && <Problem>{found.error.message}</Problem>}
        {found.data && asked && (
          <>
            {found.data.length === 0 ? (
              <p className="text-small text-ink-subtle">Modrinth has nothing by that name for what this server runs.</p>
            ) : (
              <ul className="overflow-hidden rounded-lg border border-hairline bg-surface-1">
                {found.data.map((project) => (
                  <Project key={project.id} id={id} project={project} runs={asked} />
                ))}
              </ul>
            )}
          </>
        )}
      </section>
    </>
  )
}

/** The `.jar` files in the folder the server's loader reads, each with a way to take it out. */
function Installed({ id, folder }: { id: number; folder: string }) {
  const queryClient = useQueryClient()
  const { data: entries } = useQuery(filesQuery(id, folder))
  const [doomed, setDoomed] = useState<string | null>(null)
  const removing = useMutation({
    mutationFn: (name: string) => removeFiles(id, [`${folder}/${name}`]),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: filesQuery(id, folder).queryKey })
      setDoomed(null)
    },
  })
  const jars = (entries ?? []).filter((entry) => entry.kind === 'file' && entry.name.endsWith('.jar'))

  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-section text-ink">
        Installed, in <code className="font-mono text-mono">{folder}/</code>
      </h2>
      {jars.length === 0 ? (
        <p className="text-small text-ink-subtle">Nothing yet.</p>
      ) : (
        <ul className="max-w-140 overflow-hidden rounded-lg border border-hairline bg-surface-1">
          {jars.map((jar) => (
            <li key={jar.name} className="flex items-center gap-3 border-b border-hairline px-4 py-1.5 last:border-b-0">
              <div className="min-w-0 flex-1">
                <p className="truncate font-mono text-mono text-ink">{jar.name}</p>
                <p className="text-small text-ink-subtle">{bytes(jar.size)}</p>
              </div>
              <Button variant="ghost" title={`Remove ${jar.name}`} onClick={() => setDoomed(jar.name)}>
                <Trash2 aria-hidden size={16} />
                <span className="sr-only">Remove {jar.name}</span>
              </Button>
            </li>
          ))}
        </ul>
      )}
      <Confirm
        open={doomed !== null}
        onClose={() => {
          setDoomed(null)
          removing.reset()
        }}
        title={`Remove ${doomed ?? 'this file'}?`}
        action={
          <Button variant="danger" busy={removing.isPending} onClick={() => doomed && removing.mutate(doomed)}>
            Remove
          </Button>
        }
      >
        <p>The file is deleted. A server that is running goes on with it until it is next started.</p>
        {removing.error && <Problem>{removing.error.message}</Problem>}
      </Confirm>
    </section>
  )
}

/** One project that was found, and its releases once they are asked for. */
function Project({ id, project, runs }: { id: number; project: FoundMod; runs: Runs }) {
  const [open, setOpen] = useState(false)

  return (
    <li className="flex flex-col gap-2 border-b border-hairline px-4 py-2.5 last:border-b-0">
      <div className="flex flex-wrap items-start gap-x-4 gap-y-1">
        <div className="min-w-0 flex-1">
          <p className="font-medium text-ink">
            {project.slug ? (
              <a
                href={`https://modrinth.com/project/${project.slug}`}
                target="_blank"
                rel="noreferrer"
                className="hover:underline"
              >
                {project.title}
              </a>
            ) : (
              project.title
            )}
            {project.author && <span className="font-normal text-ink-subtle"> by {project.author}</span>}
          </p>
          <p className="text-small">{project.description}</p>
          <p className="text-small text-ink-subtle">{COUNT.format(project.downloads)} downloads</p>
        </div>
        <Button aria-expanded={open} onClick={() => setOpen(!open)}>
          {open ? 'Hide releases' : 'Releases'}
        </Button>
      </div>
      {open && <Releases id={id} project={project} runs={runs} />}
    </li>
  )
}

/** The oldest and the newest of the versions of Minecraft a release is for. */
function games(release: ModRelease): string {
  const [first] = release.games
  const last = release.games.at(-1)
  if (!first || !last) return ''
  return first === last ? first : `${first} to ${last}`
}

/** A project's newest releases for what the server runs, each to be installed. */
function Releases({ id, project, runs }: { id: number; project: FoundMod; runs: Runs }) {
  const queryClient = useQueryClient()
  const releases = useQuery({
    queryKey: ['servers', id, 'mods', project.id, runs.loader, runs.game],
    queryFn: () => modReleases(id, project.id, runs),
    staleTime: 60_000,
  })
  const installing = useMutation({
    mutationFn: (release: string) => installMod(id, release, runs.loader),
    // The list of what is installed shows it without being asked.
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['servers', id, 'files'] }),
  })

  if (releases.isPending) return <p className="text-small text-ink-subtle">Asking Modrinth…</p>
  if (releases.error) return <Problem>{releases.error.message}</Problem>
  if (releases.data.length === 0) {
    return <p className="text-small text-ink-subtle">It has no release for what this server runs.</p>
  }
  return (
    <div className="flex flex-col gap-2">
      <ul className="rounded-md border border-hairline">
        {releases.data.map((release) => (
          <li key={release.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-hairline px-3 py-1.5 last:border-b-0">
            <div className="min-w-0 flex-1">
              <p className="truncate font-mono text-mono text-ink">
                {release.number || release.name}
                {release.channel && release.channel !== 'release' && <span className="text-ink-subtle"> ({release.channel})</span>}
              </p>
              <p className="text-small text-ink-subtle">
                {[games(release), bytes(release.size)].filter(Boolean).join(' · ')}
                {release.requires > 0 &&
                  ` · needs ${release.requires} other ${release.requires === 1 ? 'project' : 'projects'}, not installed with it`}
              </p>
            </div>
            <Button
              busy={installing.isPending && installing.variables === release.id}
              disabled={installing.isPending}
              aria-label={`Install ${project.title} ${release.number}`}
              onClick={() => installing.mutate(release.id)}
            >
              Install
            </Button>
          </li>
        ))}
      </ul>
      {installing.error && <Problem>{installing.error.message}</Problem>}
      {installing.isSuccess && (
        <p role="status" className="text-small text-ink-muted">
          Put in <code className="font-mono text-mono">{installing.data}</code>. It is loaded when the server next starts.
        </p>
      )}
    </div>
  )
}
