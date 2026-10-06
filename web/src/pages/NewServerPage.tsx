import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, getRouteApi, notFound, useNavigate } from '@tanstack/react-router'
import { useId } from 'react'

import { createServer, type Template } from '../api/client'
import { Button, Field, PageBar, Problem, buttonClass } from '../components/ui'
import { serverQuery, serversQuery } from '../servers'
import { templateQuery, templatesQuery } from '../templates'

const route = getRouteApi('/shell/servers/new/$templateId')

/** Where Minecraft listens, and so where the search for a free port starts. */
const FIRST_PORT = 25565

/** The three steps of DESIGN.md. The third is the new server's own page, which shows its install. */
function Steps({ at }: { at: 0 | 1 }) {
  return (
    <ol className="flex flex-wrap gap-x-6 gap-y-1 text-small">
      {['Choose a game', 'Configure', 'Install'].map((step, index) => (
        <li key={step} aria-current={index === at ? 'step' : undefined} className={index === at ? 'font-medium text-ink' : 'text-ink-subtle'}>
          {index + 1}. {step}
        </li>
      ))}
    </ol>
  )
}

/** Step one: which template the server is made from. */
export function ChooseTemplatePage() {
  const { data: templates } = useSuspenseQuery(templatesQuery)

  return (
    <>
      <PageBar title="New server" crumb={<Link to="/">Servers</Link>} />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <Steps at={0} />
        {templates.length === 0 ? (
          <p>
            There is no template to make a server from yet.{' '}
            <Link to="/templates/import" className="text-accent hover:underline">
              Import an egg
            </Link>{' '}
            first.
          </p>
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
      </main>
    </>
  )
}

/** Step two: a name, how much of the machine it may use, and what the template asks. */
export function NewServerPage() {
  const id = useId()
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

  const taken = new Set(servers.map((server) => server.port))
  let port = FIRST_PORT
  while (taken.has(port)) port += 1
  // What a person is meant to set comes first, with what has to be set because the template
  // leaves it empty. What the template sets for itself is tucked away.
  const comesFirst = (variable: Template['variables'][number]) =>
    variable.user_editable || (variable.default === '' && variable.rules.includes('required'))
  const asked = template.variables.filter(comesFirst)
  const tucked = template.variables.filter((variable) => !comesFirst(variable))

  return (
    <>
      <PageBar title={template.name} crumb={<Link to="/servers/new">New server</Link>} />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <Steps at={1} />
        <form
          className="flex max-w-140 flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            const form = new FormData(event.currentTarget)
            const typed = (name: string) => String(form.get(name) ?? '')
            creating.mutate({
              name: typed('name'),
              template_id: template.id,
              image: typed('image') || null,
              memory_mb: Number(typed('memory_mb')),
              cpu_percent: Number(typed('cpu_percent')) || 0,
              port: Number(typed('port')),
              variables: Object.fromEntries(template.variables.map(({ env }) => [env, typed(`variable.${env}`)])),
              eula: form.get('eula') === 'on',
            })
          }}
        >
          <Field label="Name" name="name" required autoFocus maxLength={60} autoComplete="off" placeholder="Survival" />
          {template.images.length > 1 && (
            <div className="flex flex-col gap-1.5">
              <label htmlFor={`${id}-image`} className="text-caption text-ink-subtle">
                Version
              </label>
              <select
                id={`${id}-image`}
                name="image"
                className="h-10 rounded-md border border-hairline-strong bg-surface-1 px-2 text-body text-ink md:h-8"
              >
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
            defaultValue={2048}
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
            defaultValue={port}
            hint="Where players reach it on this machine. The first one no other server has."
          />
          {asked.map((variable) => (
            <VariableField key={variable.env} variable={variable} />
          ))}
          {template.features.includes('eula') && (
            <label className="flex items-start gap-2">
              <input type="checkbox" name="eula" className="mt-0.5 size-4 accent-accent" />
              <span>
                I agree to the{' '}
                <a href="https://aka.ms/MinecraftEULA" target="_blank" rel="noreferrer" className="text-accent hover:underline">
                  Minecraft EULA
                </a>
                .
                <span className="block text-small text-ink-subtle">
                  Without it the server stops at its first start and asks for it.
                </span>
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
                defaultValue={0}
                hint="150 is a core and a half. 0 is no limit."
              />
              {tucked.map((variable) => (
                <VariableField key={variable.env} variable={variable} />
              ))}
            </div>
          </details>
          {creating.error && <Problem>{creating.error.message}</Problem>}
          <div className="flex gap-2">
            <Button type="submit" variant="primary" busy={creating.isPending}>
              Create server
            </Button>
            <Link to="/" className={buttonClass('ghost')}>
              Cancel
            </Link>
          </div>
        </form>
      </main>
    </>
  )
}

function VariableField({ variable }: { variable: Template['variables'][number] }) {
  return (
    <Field
      label={variable.name}
      name={`variable.${variable.env}`}
      mono
      autoComplete="off"
      spellCheck={false}
      defaultValue={variable.default}
      hint={variable.description || undefined}
    />
  )
}
