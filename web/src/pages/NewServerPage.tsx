import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, getRouteApi, notFound, useNavigate } from '@tanstack/react-router'
import { X } from 'lucide-react'
import { useId, useState, type ReactNode } from 'react'

import { createServer, type PortProtocol, type ServerSettings, type Template } from '../api/client'
import { Button, Field, PageBar, Problem, Steps, buttonClass } from '../components/ui'
import { useGate } from '../gate'
import { serverQuery, serversQuery } from '../servers'
import { templateQuery, templatesQuery } from '../templates'

const route = getRouteApi('/shell/servers/new/$templateId')

/** Where Minecraft listens, and so where the search for a free port starts. */
const FIRST_PORT = 25565

/** The three steps of DESIGN.md. The third is the new server's own page, which shows its install. */
const STEPS = ['Choose a game', 'Configure', 'Install']

const PROTOCOLS: { value: PortProtocol; label: string }[] = [
  { value: 'both', label: 'TCP and UDP' },
  { value: 'tcp', label: 'TCP' },
  { value: 'udp', label: 'UDP' },
]

const SELECT = 'h-10 rounded-md border border-hairline-strong bg-surface-1 px-2 text-body text-ink md:h-8'

/** Step one: which template the server is made from. */
export function ChooseTemplatePage() {
  const { data: templates } = useSuspenseQuery(templatesQuery)

  return (
    <>
      <PageBar title="New server" crumb={<Link to="/">Servers</Link>} />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <Steps steps={STEPS} at={0} />
        {templates.length === 0 ? (
          <div className="flex max-w-140 flex-col items-start gap-3">
            <p>
              Homewarp runs a game from its egg: a small file that says how the game is installed and started. There is
              none here yet. The Pelican community publishes eggs for hundreds of games, and Homewarp fetches the one you
              pick.
            </p>
            <Link to="/templates/import" search={{ then: 'server' }} className={buttonClass('primary')}>
              Find a game
            </Link>
          </div>
        ) : (
          <ul className="grid gap-4 md:grid-cols-2 wide:grid-cols-3">
            {templates.map((template) => (
              <li key={template.id} className="min-w-0">
                <Link
                  to="/servers/new/$templateId"
                  params={{ templateId: String(template.id) }}
                  className="flex h-full flex-col gap-1 rounded-lg border border-hairline bg-surface-1 p-4 transition-colors duration-120 ease-out hover:bg-surface-2"
                >
                  <h2 className="truncate text-body font-medium text-ink">{template.name}</h2>
                  <p className="line-clamp-2 text-small">{template.description || 'No description.'}</p>
                </Link>
              </li>
            ))}
          </ul>
        )}
        {templates.length > 0 && (
          <p className="text-small text-ink-subtle">
            Not here?{' '}
            <Link to="/templates/import" search={{ then: 'server' }} className="text-accent hover:underline">
              Find another game
            </Link>{' '}
            in the catalogue.
          </p>
        )}
      </main>
    </>
  )
}

/** Step two: a name, how much of the machine it may use, and what the template asks. */
export function NewServerPage() {
  const { templateId } = route.useParams()
  const { data: template } = useSuspenseQuery(templateQuery(Number(templateId)))
  const { data: servers } = useSuspenseQuery(serversQuery)
  if (!template) throw notFound()

  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const creating = useMutation({
    mutationFn: createServer,
    onSuccess: async (server) => {
      // The page this opens has its first answer already.
      queryClient.setQueryData(serverQuery(server.id).queryKey, server)
      void queryClient.invalidateQueries({ queryKey: serversQuery.queryKey, exact: true })
      await navigate({ to: '/servers/$serverId', params: { serverId: String(server.id) } })
    },
  })

  // The first port from Minecraft's own upward that no server has. The servers' further ports
  // are known once the Gate has answered; a clash with one of those is refused in words anyway.
  const gate = useGate()
  const taken = new Set([...servers.map((server) => server.port), ...(gate?.ports.map((published) => published.port) ?? [])])
  let port = FIRST_PORT
  while (taken.has(port)) port += 1

  return (
    <>
      <PageBar title={template.name} crumb={<Link to="/servers/new">New server</Link>} />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <Steps steps={STEPS} at={1} />
        <ServerForm
          template={template}
          start={{ name: '', memory_mb: 2048, port }}
          submit="Create server"
          pending={creating.isPending}
          problem={creating.error?.message}
          onSubmit={(settings) => creating.mutate({ ...settings, template_id: template.id })}
          cancel={
            <Link to="/" className={buttonClass('ghost')}>
              Cancel
            </Link>
          }
        />
      </main>
    </>
  )
}

type ServerFormProps = {
  template: Template
  /** What the fields hold to begin with. A variable it does not name starts at the template's default. */
  start: ServerSettings
  /** What the button that sends the form says. */
  submit: string
  pending: boolean
  problem?: string
  onSubmit: (settings: ServerSettings) => void
  cancel: ReactNode
}

/** The form a server is made with, and changed with afterwards. */
export function ServerForm({ template, start, submit, pending, problem, onSubmit, cancel }: ServerFormProps) {
  const id = useId()
  // The further ports are rows that come and go, so they are kept here and not read off the form.
  const [further, setFurther] = useState(() =>
    (start.ports ?? []).map((one, key) => ({ key, port: String(one.port), protocol: one.protocol ?? ('both' as PortProtocol) })),
  )
  const changeFurther = (key: number, change: { port?: string; protocol?: PortProtocol }) =>
    setFurther((rows) => rows.map((row) => (row.key === key ? { ...row, ...change } : row)))
  // What a person is meant to set comes first, with what has to be set because the template
  // leaves it empty. What the template sets for itself is tucked away.
  const comesFirst = (variable: Template['variables'][number]) =>
    variable.user_editable || (variable.default === '' && variable.rules.includes('required'))
  const asked = template.variables.filter(comesFirst)
  const tucked = template.variables.filter((variable) => !comesFirst(variable))
  const field = (variable: Template['variables'][number]) => (
    <Field
      key={variable.env}
      label={variable.name}
      name={`variable.${variable.env}`}
      mono
      autoComplete="off"
      spellCheck={false}
      defaultValue={start.variables?.[variable.env] ?? variable.default}
      hint={variable.description || undefined}
    />
  )

  return (
    <form
      className="flex max-w-140 flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault()
        const form = new FormData(event.currentTarget)
        const typed = (name: string) => String(form.get(name) ?? '')
        onSubmit({
          name: typed('name'),
          image: typed('image') || null,
          memory_mb: Number(typed('memory_mb')),
          cpu_percent: Number(typed('cpu_percent')) || 0,
          port: Number(typed('port')),
          protocol: typed('protocol') as PortProtocol,
          ports: further.filter((row) => row.port !== '').map((row) => ({ port: Number(row.port), protocol: row.protocol })),
          variables: Object.fromEntries(template.variables.map(({ env }) => [env, typed(`variable.${env}`)])),
          eula: form.get('eula') === 'on',
          sleep_minutes: Number(typed('sleep_minutes')) || 0,
        })
      }}
    >
      <Field label="Name" name="name" required autoFocus maxLength={60} autoComplete="off" placeholder="Survival" defaultValue={start.name} />
      {template.images.length > 1 && (
        <div className="flex flex-col gap-1.5">
          <label htmlFor={`${id}-image`} className="text-caption text-ink-subtle">
            Version
          </label>
          <select id={`${id}-image`} name="image" defaultValue={start.image ?? template.images[0]?.image} className={SELECT}>
            {template.images.map(({ label, image }) => (
              <option key={image} value={image}>
                {label}
              </option>
            ))}
          </select>
        </div>
      )}
      <Field
        label="Memory, in MB"
        name="memory_mb"
        type="number"
        required
        min={128}
        step={128}
        defaultValue={start.memory_mb}
        hint="The most the server may use. It is stopped if it takes more."
      />
      <Field
        label="Port"
        name="port"
        type="number"
        mono
        required
        min={1024}
        max={65535}
        defaultValue={start.port}
        hint="Where players reach it, here and at a connected VPS. No two servers have the same one."
      />
      {asked.map(field)}
      {template.features.includes('eula') && (
        <label className="flex items-start gap-2">
          <input type="checkbox" name="eula" defaultChecked={start.eula ?? false} className="mt-0.5 size-4 accent-accent" />
          <span>
            I agree to the{' '}
            <a href="https://aka.ms/MinecraftEULA" target="_blank" rel="noreferrer" className="text-accent hover:underline">
              Minecraft EULA
            </a>
            .
            <span className="block text-small text-ink-subtle">Without it the server stops at its first start and asks for it.</span>
          </span>
        </label>
      )}
      <details>
        <summary className="cursor-pointer text-ink">Advanced</summary>
        <div className="mt-4 flex flex-col gap-4">
          <Field
            label="Processor limit, in % of one core"
            name="cpu_percent"
            type="number"
            min={0}
            step={10}
            defaultValue={start.cpu_percent ?? 0}
            hint="150 is a core and a half. 0 is no limit."
          />
          <Field
            label="Sleep after, in minutes with nobody on it"
            name="sleep_minutes"
            type="number"
            min={0}
            max={10080}
            defaultValue={start.sleep_minutes ?? 0}
            hint="For a Minecraft server, which says who is on it: stopped when nobody has been for this long, and started again when a player joins. 0 is never."
          />
          <div className="flex flex-col gap-1.5">
            <label htmlFor={`${id}-protocol`} className="text-caption text-ink-subtle">
              What the port is open for
            </label>
            <select id={`${id}-protocol`} name="protocol" defaultValue={start.protocol ?? 'both'} className={SELECT}>
              {PROTOCOLS.map(({ value, label }) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
            <div className="text-small text-ink-subtle">A template does not say which its game speaks, so it starts as both.</div>
          </div>
          <fieldset className="flex flex-col gap-1.5">
            <legend className="mb-1.5 text-caption text-ink-subtle">Further ports</legend>
            {further.map((row) => (
              <div key={row.key} className="flex gap-2">
                <input
                  type="number"
                  aria-label="Port"
                  min={1024}
                  max={65535}
                  value={row.port}
                  onChange={(event) => changeFurther(row.key, { port: event.target.value })}
                  className="h-10 w-28 rounded-md border border-hairline-strong bg-surface-1 px-2.5 font-mono text-mono text-ink md:h-8"
                />
                <select
                  aria-label="What it is open for"
                  value={row.protocol}
                  onChange={(event) => changeFurther(row.key, { protocol: event.target.value as PortProtocol })}
                  className={SELECT}
                >
                  {PROTOCOLS.map(({ value, label }) => (
                    <option key={value} value={value}>
                      {label}
                    </option>
                  ))}
                </select>
                <Button variant="ghost" title="Remove this port" onClick={() => setFurther((rows) => rows.filter((other) => other.key !== row.key))}>
                  <X aria-hidden size={16} />
                  <span className="sr-only">Remove this port</span>
                </Button>
              </div>
            ))}
            <div>
              <Button
                onClick={() =>
                  setFurther((rows) => [...rows, { key: Math.max(-1, ...rows.map((row) => row.key)) + 1, port: '', protocol: 'udp' }])
                }
              >
                Add a port
              </Button>
            </div>
            <div className="text-small text-ink-subtle">
              For a game that listens on more than one: voice chat, a port it is queried on. Each is opened as the first is.
            </div>
          </fieldset>
          {tucked.map(field)}
        </div>
      </details>
      {problem && <Problem>{problem}</Problem>}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" busy={pending}>
          {submit}
        </Button>
        {cancel}
      </div>
    </form>
  )
}
