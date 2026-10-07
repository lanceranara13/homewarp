import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, getRouteApi } from '@tanstack/react-router'
import { Archive, CloudUpload, Download, History, Trash2 } from 'lucide-react'
import { useState } from 'react'

import { backupUrl, copyBackup, keepBackups, makeBackup, removeBackup, restoreBackup, type Backup } from '../api/client'
import { Button, Confirm, Field, Pill, Problem, Select, buttonClass } from '../components/ui'
import { bytes, when } from '../format'
import { backupsQuery, serverQuery } from '../servers'

const route = getRouteApi('/shell/servers/$serverId')

/** How often the list is asked for again while a backup is being made, in milliseconds. */
const WHILE_MAKING = 2_000

/**
 * A server's backups: making one, saving one, putting one back (DESIGN.md,
 * Inside a server). Each is every file the server has, in one file kept
 * beside the server's own where the server cannot reach it.
 */
export function BackupsTab() {
  const { serverId } = route.useParams()
  const id = Number(serverId)
  const queryClient = useQueryClient()
  const { data: server } = useSuspenseQuery(serverQuery(id))
  const { data } = useSuspenseQuery({
    ...backupsQuery(id),
    // Making one is answered at once and done later: the list is asked again until it is.
    refetchInterval: (query) =>
      query.state.data?.backups.some((backup) => backup.state === 'running' || backup.stored === 'copying') ? WHILE_MAKING : false,
  })
  const [restoring, setRestoring] = useState<Backup | null>(null)
  const [doomed, setDoomed] = useState<Backup | null>(null)
  const [typed, setTyped] = useState('')
  const [said, setSaid] = useState(false)
  const again = () => queryClient.invalidateQueries({ queryKey: backupsQuery(id).queryKey })

  const making = useMutation({ mutationFn: (name: string) => makeBackup(id, name), onSuccess: again })
  const keeping = useMutation({ mutationFn: (kept: number) => keepBackups(id, kept), onSettled: again })
  const putting = useMutation({
    mutationFn: (backup: number) => restoreBackup(id, backup),
    onSuccess: () => {
      setRestoring(null)
      setTyped('')
      setSaid(true)
    },
  })
  const copying = useMutation({ mutationFn: (backup: number) => copyBackup(id, backup), onSettled: again })
  const removing = useMutation({
    mutationFn: (backup: number) => removeBackup(id, backup),
    onSuccess: async () => {
      await again()
      setDoomed(null)
    },
  })
  const busy = data.backups.some((backup) => backup.state === 'running')
  const name = server?.name ?? ''

  return (
    <>
      <div className="flex flex-wrap items-end gap-x-6 gap-y-3">
        <form
          className="flex min-w-0 flex-1 flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            const form = event.currentTarget
            setSaid(false)
            making.mutate(String(new FormData(form).get('name') ?? ''), { onSuccess: () => form.reset() })
          }}
        >
          <div className="min-w-0 flex-1 md:max-w-80">
            <Field label="Name for a new backup" name="name" maxLength={60} placeholder="Backup" autoComplete="off" />
          </div>
          <Button type="submit" variant="primary" busy={making.isPending} disabled={busy}>
            <Archive aria-hidden size={16} />
            Back up now
          </Button>
        </form>
        <Select
          label="Backups kept"
          value={data.kept}
          disabled={keeping.isPending}
          onChange={(event) => keeping.mutate(Number(event.currentTarget.value))}
        >
          {Array.from({ length: 20 }, (_, index) => index + 1).map((count) => (
            <option key={count} value={count}>
              The newest {count}
            </option>
          ))}
        </Select>
      </div>
      <p className="max-w-140 text-small text-ink-subtle">
        A backup is every file this server has, as they are when it is made. The server can go on running meanwhile; a game
        that keeps its world in many files is best told to save first. When one more is done, the oldest beyond those kept
        are deleted.
        {data.store && ' Each is also copied to the store the owner has set, and its copy goes when it goes.'}
      </p>
      {(making.error ?? keeping.error ?? copying.error) && (
        <Problem>{(making.error ?? keeping.error ?? copying.error)?.message}</Problem>
      )}
      {said && (
        <p role="status" className="text-small text-ink-muted">
          The backup is being put back. The{' '}
          <Link to="/servers/$serverId" params={{ serverId }} className="text-accent hover:underline">
            console
          </Link>{' '}
          says how it goes, and the server can be started once it is done.
        </p>
      )}

      {data.backups.length === 0 ? (
        <div className="flex flex-1 flex-col items-center justify-center rounded-lg border border-dashed border-hairline-strong py-16 text-center">
          <Archive aria-hidden size={32} className="mb-3 text-ink-faint" />
          <h2 className="text-section text-ink">No backups yet.</h2>
          <p className="mt-1">Make one before an update, or have a schedule make one every night.</p>
        </div>
      ) : (
        <ul className="overflow-hidden rounded-lg border border-hairline bg-surface-1">
          {data.backups.map((backup) => (
            <li key={backup.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-hairline px-4 py-2.5 last:border-b-0">
              <div className="min-w-0 flex-1">
                <p className="truncate font-medium text-ink">{backup.name}</p>
                <p className="text-small text-ink-subtle">
                  {when(backup.created_at)}
                  {backup.state === 'done' && ` · ${bytes(backup.size_bytes)}`}
                  {backup.stored === 'copied' && ' · copied to the store'}
                </p>
                {backup.problem && <p className="text-small text-danger">{backup.problem}</p>}
                {backup.stored === 'failed' && (
                  <p className="text-small text-danger">Not copied to the store. {backup.stored_problem}</p>
                )}
              </div>
              {backup.state === 'running' && <Pill tone="installing">Being made</Pill>}
              {backup.stored === 'copying' && <Pill tone="installing">Being copied</Pill>}
              {backup.state === 'failed' && <Pill tone="crashed">Failed</Pill>}
              <span className="flex gap-1">
                {/* One made before there was a store, or whose copy did not arrive. */}
                {data.store && backup.state === 'done' && backup.stored !== 'copied' && backup.stored !== 'copying' && (
                  <Button variant="ghost" title="Copy to the store" disabled={copying.isPending} onClick={() => copying.mutate(backup.id)}>
                    <CloudUpload aria-hidden size={16} />
                    <span className="sr-only wide:not-sr-only">Copy to the store</span>
                  </Button>
                )}
                {backup.state === 'done' && (
                  <>
                    <a href={backupUrl(id, backup.id)} download title="Download" className={buttonClass('ghost')}>
                      <Download aria-hidden size={16} />
                      <span className="sr-only wide:not-sr-only">Download</span>
                    </a>
                    <Button variant="ghost" title="Put back" onClick={() => setRestoring(backup)}>
                      <History aria-hidden size={16} />
                      <span className="sr-only wide:not-sr-only">Put back</span>
                    </Button>
                  </>
                )}
                {backup.state !== 'running' && backup.stored !== 'copying' && (
                  <Button variant="ghost" title="Delete" onClick={() => setDoomed(backup)}>
                    <Trash2 aria-hidden size={16} />
                    <span className="sr-only wide:not-sr-only">Delete</span>
                  </Button>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}

      <Confirm
        open={restoring !== null}
        onClose={() => {
          setRestoring(null)
          setTyped('')
          putting.reset()
        }}
        title={`Put ${restoring?.name ?? 'this backup'} back?`}
        action={
          <Button variant="danger" disabled={typed !== name} busy={putting.isPending} onClick={() => restoring && putting.mutate(restoring.id)}>
            Put back
          </Button>
        }
      >
        <p>
          Every file {name} has now is deleted, and the files of this backup are put in their place: what the server has
          done since {restoring ? when(restoring.created_at) : 'then'} is lost. The server has to be stopped.
        </p>
        <div className="mt-3">
          <Field
            label={`Type ${name} to go ahead`}
            value={typed}
            onChange={(event) => setTyped(event.currentTarget.value)}
            autoComplete="off"
            spellCheck={false}
          />
        </div>
        {putting.error && (
          <div className="mt-3">
            <Problem>{putting.error.message}</Problem>
          </div>
        )}
      </Confirm>

      <Confirm
        open={doomed !== null}
        onClose={() => {
          setDoomed(null)
          removing.reset()
        }}
        title={`Delete ${doomed?.name ?? 'this backup'}?`}
        action={
          <Button variant="danger" busy={removing.isPending} onClick={() => doomed && removing.mutate(doomed.id)}>
            Delete
          </Button>
        }
      >
        <p>The backup goes for good. The server&rsquo;s files as they are now are not touched.</p>
        {removing.error && (
          <div className="mt-3">
            <Problem>{removing.error.message}</Problem>
          </div>
        )}
      </Confirm>
    </>
  )
}
