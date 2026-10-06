import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, getRouteApi, notFound, useNavigate } from '@tanstack/react-router'
import { Trash2 } from 'lucide-react'
import { useState, type ReactNode } from 'react'

import { removeTemplate } from '../api/client'
import { Button, Confirm, PageBar, Problem } from '../components/ui'
import { templateQuery, templatesQuery } from '../templates'

const route = getRouteApi('/shell/templates/$templateId')

const CODE = 'font-mono text-mono text-ink'
const BLOCK = `${CODE} rounded-sm bg-surface-2 p-3`

/** One template, laid out: what its egg has told Homewarp to do on this machine. */
export function TemplatePage() {
  const { templateId } = route.useParams()
  const { data: template } = useSuspenseQuery(templateQuery(Number(templateId)))
  // The loader turns away an address with no template behind it; this is one removed since.
  if (!template) throw notFound()
  const imported = new Date(template.created_at * 1000).toLocaleDateString(undefined, { dateStyle: 'medium' })

  return (
    <>
      <PageBar title={template.name} crumb={<Link to="/templates">Templates</Link>}>
        <RemoveTemplate id={template.id} name={template.name} />
      </PageBar>
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:p-6">
        <div className="max-w-140">
          {template.description && <p>{template.description}</p>}
          <p className="mt-1 text-small text-ink-subtle">Imported {imported}</p>
        </div>

        <Section title="Images">
          <ul className="flex flex-col gap-2">
            {template.images.map(({ label, image }, index) => (
              <li key={`${label} ${image}`} className="flex flex-wrap items-baseline gap-x-3">
                {label !== image && <span className="text-ink">{label}</span>}
                <code className={`${CODE} wrap-anywhere`}>{image}</code>
                {index === 0 && template.images.length > 1 && (
                  <span className="text-small text-ink-subtle">used unless another is picked</span>
                )}
              </li>
            ))}
          </ul>
        </Section>

        <Section title="Startup">
          <pre className={`${BLOCK} break-words whitespace-pre-wrap`}>{template.startup}</pre>
          <dl className="mt-4 grid gap-x-6 gap-y-2 md:grid-cols-[14rem_1fr]">
            <Fact label="Has started once the console prints">
              {template.done.length > 0 ? <Codes items={template.done} /> : 'Nothing: the template does not say.'}
            </Fact>
            <Fact label="Is stopped by">
              {template.stop.by === 'command' ? 'typing ' : 'the signal '}
              <code className={CODE}>{template.stop.value}</code>
              {template.stop.by === 'command' && ' into the console'}
            </Fact>
            {template.config_files.length > 0 && (
              <Fact label="Files patched before each start">
                <Codes items={template.config_files} />
              </Fact>
            )}
            {template.features.length > 0 && (
              <Fact label="Features">
                <Codes items={template.features} />
              </Fact>
            )}
          </dl>
        </Section>

        <Section title="Variables">
          {template.variables.length === 0 ? (
            <p>This template has none.</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full min-w-140 text-left">
                <thead>
                  <tr className="text-caption text-ink-subtle">
                    <th className="pr-4 pb-2 font-medium">Name</th>
                    <th className="pr-4 pb-2 font-medium">Variable</th>
                    <th className="pr-4 pb-2 font-medium">Default</th>
                    <th className="pb-2 font-medium">Rules</th>
                  </tr>
                </thead>
                <tbody>
                  {template.variables.map((variable, index) => (
                    <tr key={index} className="border-t border-hairline align-top">
                      <td className="py-2.5 pr-4">
                        <div className="font-medium text-ink">{variable.name}</div>
                        {variable.description && <div className="text-small text-ink-subtle">{variable.description}</div>}
                      </td>
                      <td className={`${CODE} py-2.5 pr-4`}>{variable.env}</td>
                      <td className={`${CODE} py-2.5 pr-4 break-words`}>{variable.default}</td>
                      <td className="py-2.5 font-mono text-mono text-ink-subtle">{variable.rules.join(' | ')}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Section>

        <Section title="Install">
          {template.install ? (
            <>
              <p>
                Runs once, before a server's first start, in <code className={`${CODE} wrap-anywhere`}>{template.install.image}</code>{' '}
                with <code className={CODE}>{template.install.entrypoint}</code>.
              </p>
              <details className="mt-3">
                <summary className="cursor-pointer text-accent hover:underline">The script</summary>
                <pre className={`${BLOCK} mt-3 max-h-96 overflow-auto`}>{template.install.script}</pre>
              </details>
            </>
          ) : (
            <p>Nothing: the image holds all a server needs.</p>
          )}
        </Section>
      </main>
    </>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="rounded-lg border border-hairline bg-surface-1 p-4">
      <h2 className="mb-3 text-section text-ink">{title}</h2>
      {children}
    </section>
  )
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-ink-subtle">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  )
}

function Codes({ items }: { items: string[] }) {
  return (
    <div className="flex flex-wrap gap-x-3 gap-y-1">
      {items.map((item, index) => (
        <code key={index} className={`${CODE} wrap-anywhere`}>
          {item}
        </code>
      ))}
    </div>
  )
}

/** The way out of a template, behind a question. Nothing is built on templates yet, so nothing else goes with it. */
function RemoveTemplate({ id, name }: { id: number; name: string }) {
  const [asking, setAsking] = useState(false)
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const removing = useMutation({
    mutationFn: () => removeTemplate(id),
    onSuccess: async () => {
      // The gallery asks afresh; this page's own answer goes once the page has.
      queryClient.removeQueries({ queryKey: templatesQuery.queryKey, exact: true })
      await navigate({ to: '/templates' })
      queryClient.removeQueries({ queryKey: templateQuery(id).queryKey })
    },
  })

  return (
    <>
      <Button onClick={() => setAsking(true)}>
        <Trash2 aria-hidden size={16} />
        Remove
      </Button>
      <Confirm
        open={asking}
        onClose={() => {
          setAsking(false)
          removing.reset()
        }}
        title={`Remove ${name}?`}
        action={
          <Button variant="danger" busy={removing.isPending} onClick={() => removing.mutate()}>
            Remove
          </Button>
        }
      >
        <p>Homewarp forgets this template. Its egg can be imported again.</p>
        {removing.error && (
          <div className="mt-3">
            <Problem>{removing.error.message}</Problem>
          </div>
        )}
      </Confirm>
    </>
  )
}
