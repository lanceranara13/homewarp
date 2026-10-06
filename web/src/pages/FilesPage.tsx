import { useMutation, useQuery, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, getRouteApi, useBlocker, useNavigate } from '@tanstack/react-router'
import {
  Archive,
  ChevronRight,
  Download,
  Ellipsis,
  File as FileIcon,
  FileArchive,
  FilePlus,
  FileText,
  Folder,
  FolderPlus,
  House,
  Link2,
  PackageOpen,
  Pencil,
  RotateCw,
  Trash2,
  Upload,
  X,
} from 'lucide-react'
import { DropdownMenu } from 'radix-ui'
import { Fragment, useRef, useState, type DragEvent } from 'react'

import {
  downloadUrl,
  makeFolder,
  moveFile,
  packFiles,
  removeFiles,
  unpackFile,
  writeFile,
  type FileEntry,
} from '../api/client'
import { Button, Confirm, CopyChip, FLOATING, Field, MENU_ITEM, Problem, buttonClass } from '../components/ui'
import { diskUseQuery, fileTextQuery, filesQuery, joined, nameOf, parentOf } from '../files'
import { bytes, when } from '../format'
import { sessionQuery } from '../session'

const filesRoute = getRouteApi('/shell/servers/$serverId/files')
const editRoute = getRouteApi('/shell/servers/$serverId/files/edit')

/** The longest file Homewarp sends as text. A longer one is for downloading. */
const LONGEST_TEXT = 4 * 1024 * 1024
/** Names that are not text whatever is in them: a click on one saves it. Anything else is tried in the editor. */
const NOT_TEXT =
  /\.(jar|zip|gz|tgz|tar|bz2|xz|zst|7z|rar|png|jpe?g|gif|webp|ico|bmp|dat|dat_old|mca|mcr|nbt|schem|schematic|db|sqlite3?|so|dll|exe|bin|class|ogg|mp3|wav|pak|vpk|bsp|pdf|woff2?|ttf|otf)$/i
/** What Homewarp unpacks. */
const ARCHIVE = /\.(zip|tar|tar\.gz|tgz)$/i

const CHECK = 'size-4 align-middle accent-accent'
const NAME = 'flex min-w-0 items-center gap-2 font-medium text-ink'
const ICON_BUTTON =
  'inline-flex size-8 shrink-0 items-center justify-center rounded-md text-ink-subtle transition-colors duration-120 ease-out hover:bg-surface-3 hover:text-ink'

/** A server's files, one folder at a time (DESIGN.md, Inside a server). */
export function FilesTab() {
  const { serverId } = filesRoute.useParams()
  const { path = '' } = filesRoute.useSearch()
  // What is chosen in a folder, and what is being typed about it, is that folder's: another one starts afresh.
  return <FolderView key={path} serverId={serverId} folder={path} />
}

/** What a person can ask for in a folder that takes the server a moment. */
type Doing = 'folder' | 'move' | 'delete' | 'pack' | 'unpack'

function FolderView({ serverId, folder }: { serverId: string; folder: string }) {
  const id = Number(serverId)
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const listing = useQuery(filesQuery(id, folder))
  const [chosen, setChosen] = useState<ReadonlySet<string>>(new Set())
  const [asking, setAsking] = useState<{ for: 'folder' | 'file' } | { for: 'move'; name: string } | null>(null)
  const [doomed, setDoomed] = useState<string[] | null>(null)
  const [said, setSaid] = useState<string | null>(null)
  const [over, setOver] = useState(false)
  const picker = useRef<HTMLInputElement>(null)

  // A running server changes its files by itself, so the folder is asked after again rather than patched.
  const again = () => {
    void queryClient.invalidateQueries({ queryKey: filesQuery(id, folder).queryKey })
    void queryClient.invalidateQueries({ queryKey: diskUseQuery(id).queryKey })
  }
  const uploads = useUploads(id, folder, again)
  const act = useMutation({
    mutationFn: ({ work }: { doing: Doing; work: () => Promise<string | null> }) => work(),
    onSuccess: (notice) => {
      setSaid(notice)
      setChosen(new Set())
      setAsking(null)
      setDoomed(null)
      again()
    },
  })
  const doing = act.isPending ? act.variables.doing : null
  const run = (doing: Doing, work: () => Promise<string | null>) => {
    setSaid(null)
    act.mutate({ doing, work })
  }

  const entries = listing.data ?? []
  const everything = entries.length > 0 && entries.every((entry) => chosen.has(entry.name))
  const choose = (name: string) =>
    setChosen((before) => {
      const after = new Set(before)
      if (!after.delete(name)) after.add(name)
      return after
    })

  const dropped = (event: DragEvent) => {
    event.preventDefault()
    setOver(false)
    const files: File[] = []
    let folders = 0
    for (const item of event.dataTransfer.items) {
      if (item.kind !== 'file') continue
      const file = item.webkitGetAsEntry()?.isDirectory ? null : item.getAsFile()
      if (file) files.push(file)
      else folders += 1
    }
    setSaid(folders > 0 ? 'A folder is not uploaded as it is. Pack it into a zip, upload that, and unpack it here.' : null)
    uploads.add(files)
  }

  return (
    <section
      aria-label="Files"
      className={`flex flex-1 flex-col gap-4 rounded-lg ${over ? 'outline-2 outline-offset-4 outline-accent outline-dashed' : ''}`}
      onDragOver={(event) => {
        if (!event.dataTransfer.types.includes('Files')) return
        event.preventDefault()
        setOver(true)
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOver(false)
      }}
      onDrop={dropped}
    >
      <div className="flex flex-wrap items-center gap-2">
        <Crumbs serverId={serverId} path={folder} />
        <div className="ml-auto flex flex-wrap items-center gap-2">
          {chosen.size > 0 ? (
            <>
              <span className="text-small text-ink-subtle">{chosen.size} chosen</span>
              <Button
                busy={doing === 'pack'}
                disabled={act.isPending}
                onClick={() => run('pack', async () => `Packed into ${await packFiles(id, folder, [...chosen])}.`)}
              >
                <Archive aria-hidden size={16} />
                Pack
              </Button>
              <Button disabled={act.isPending} onClick={() => setDoomed([...chosen])}>
                <Trash2 aria-hidden size={16} />
                Delete
              </Button>
            </>
          ) : (
            <>
              <Button onClick={() => picker.current?.click()}>
                <Upload aria-hidden size={16} />
                Upload
              </Button>
              <Button title="New folder" onClick={() => setAsking({ for: 'folder' })}>
                <FolderPlus aria-hidden size={16} />
                <span className="sr-only md:not-sr-only">New folder</span>
              </Button>
              <Button title="New file" onClick={() => setAsking({ for: 'file' })}>
                <FilePlus aria-hidden size={16} />
                <span className="sr-only md:not-sr-only">New file</span>
              </Button>
            </>
          )}
          <button type="button" title="Look again" onClick={again} className={ICON_BUTTON}>
            <RotateCw aria-hidden size={16} className={listing.isFetching ? 'animate-spin motion-reduce:animate-none' : ''} />
            <span className="sr-only">Look again</span>
          </button>
          <input
            ref={picker}
            type="file"
            multiple
            hidden
            onChange={(event) => {
              uploads.add([...(event.currentTarget.files ?? [])])
              // So that choosing the same file again is a change again.
              event.currentTarget.value = ''
            }}
          />
        </div>
      </div>

      {asking?.for === 'folder' && (
        <NameForm
          label="Name of the new folder"
          submit="Create"
          busy={doing === 'folder'}
          onCancel={() => setAsking(null)}
          onSubmit={(name) =>
            run('folder', async () => {
              await makeFolder(id, joined(folder, name))
              return null
            })
          }
        />
      )}
      {asking?.for === 'file' && (
        <NameForm
          label="Name of the new file"
          submit="Open"
          busy={false}
          onCancel={() => setAsking(null)}
          // Nothing is made yet: the editor makes the file when it first saves.
          onSubmit={(name) =>
            void navigate({ to: '/servers/$serverId/files/edit', params: { serverId }, search: { path: joined(folder, name) } })
          }
        />
      )}
      {asking?.for === 'move' && (
        <NameForm
          key={asking.name}
          label={`New name for ${asking.name}`}
          hint="To move it as well, give a path: one that starts with / is from the top folder."
          start={asking.name}
          submit="Rename"
          busy={doing === 'move'}
          onCancel={() => setAsking(null)}
          onSubmit={(name) =>
            run('move', async () => {
              await moveFile(id, joined(folder, asking.name), name.startsWith('/') ? name.slice(1) : joined(folder, name))
              return null
            })
          }
        />
      )}

      {said && (
        <p role="status" className="text-small text-ink-muted">
          {said}
        </p>
      )}
      {act.error && doomed === null && <Problem>{act.error.message}</Problem>}
      <UploadList uploads={uploads} />

      {listing.error ? (
        <Problem>{listing.error.message}</Problem>
      ) : listing.data === null ? (
        <p>There is no such folder. It may have been deleted, or renamed, since this page was opened.</p>
      ) : entries.length === 0 ? (
        listing.isPending || (
          <div className="flex flex-1 flex-col items-center justify-center rounded-lg border border-dashed border-hairline-strong py-16 text-center">
            <Folder aria-hidden size={32} className="mb-3 text-ink-faint" />
            <h2 className="text-section text-ink">This folder is empty.</h2>
            <p className="mt-1">Drop files here, or use Upload.</p>
          </div>
        )
      ) : (
        <div className="overflow-hidden rounded-lg border border-hairline bg-surface-1">
          <table className="w-full table-fixed text-left">
            <colgroup>
              <col className="w-10" />
              <col />
              <col className="w-20 md:w-24" />
              <col className="hidden w-48 md:table-column" />
              <col className="w-11" />
            </colgroup>
            <thead className="border-b border-hairline text-caption text-ink-subtle">
              <tr className="h-9">
                <th className="pl-3">
                  <input
                    type="checkbox"
                    aria-label="Choose everything in this folder"
                    checked={everything}
                    onChange={() => setChosen(everything ? new Set() : new Set(entries.map((entry) => entry.name)))}
                    className={CHECK}
                  />
                </th>
                <th>Name</th>
                <th className="text-right">Size</th>
                <th className="hidden pl-6 md:table-cell">Changed</th>
                <th>
                  <span className="sr-only">More</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {entries.map((entry) => (
                <Row
                  key={entry.name}
                  serverId={serverId}
                  folder={folder}
                  entry={entry}
                  chosen={chosen.has(entry.name)}
                  onChoose={() => choose(entry.name)}
                  onMove={() => setAsking({ for: 'move', name: entry.name })}
                  onDelete={() => setDoomed([entry.name])}
                  onUnpack={() =>
                    run('unpack', async () => {
                      const { files, skipped } = await unpackFile(id, joined(folder, entry.name))
                      const left = skipped === 1 ? '1 entry was left out' : `${skipped} entries were left out`
                      return `Unpacked ${files === 1 ? '1 file' : `${files} files`} from ${entry.name}.${
                        skipped > 0 ? ` ${left}: what would have gone outside this server's files, and what is not a file.` : ''
                      }`
                    })
                  }
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
      {doing === 'unpack' && (
        <p role="status" className="text-small text-ink-subtle">
          Unpacking. A big archive takes a while.
        </p>
      )}
      <DiskLine id={id} />
      <SftpLine id={id} />

      <Confirm
        open={doomed !== null}
        onClose={() => {
          setDoomed(null)
          act.reset()
        }}
        title={doomed?.length === 1 ? `Delete ${doomed[0]}?` : `Delete ${doomed?.length ?? 0} things?`}
        action={
          <Button
            variant="danger"
            busy={doing === 'delete'}
            onClick={() =>
              run('delete', async () => {
                await removeFiles(id, (doomed ?? []).map((name) => joined(folder, name)))
                return null
              })
            }
          >
            Delete
          </Button>
        }
      >
        <p>It goes for good, a folder with everything in it. Homewarp keeps no bin to bring it back from.</p>
        {act.error && (
          <div className="mt-3">
            <Problem>{act.error.message}</Problem>
          </div>
        )}
      </Confirm>
    </section>
  )
}

type RowProps = {
  serverId: string
  folder: string
  entry: FileEntry
  chosen: boolean
  onChoose: () => void
  onMove: () => void
  onDelete: () => void
  onUnpack: () => void
}

/** One file or folder. Its name leads to what is most likely wanted of it; the rest is in its menu. */
function Row({ serverId, folder, entry, chosen, onChoose, onMove, onDelete, onUnpack }: RowProps) {
  const path = joined(folder, entry.name)
  const saved = downloadUrl(Number(serverId), path)
  const archive = entry.kind === 'file' && ARCHIVE.test(entry.name)
  const text = entry.kind === 'file' && entry.size <= LONGEST_TEXT && !NOT_TEXT.test(entry.name)
  const Icon = entry.kind === 'folder' ? Folder : entry.kind === 'link' ? Link2 : archive ? FileArchive : text ? FileText : FileIcon
  const name = (
    <>
      <Icon aria-hidden size={16} className="shrink-0 text-ink-subtle" />
      <span className="truncate">{entry.name}</span>
    </>
  )

  return (
    <tr className={`group h-11 border-b border-hairline last:border-b-0 md:h-10 ${chosen ? 'bg-accent-soft' : 'hover:bg-surface-2'}`}>
      <td className="pl-3">
        <input type="checkbox" aria-label={`Choose ${entry.name}`} checked={chosen} onChange={onChoose} className={CHECK} />
      </td>
      <td>
        {entry.kind === 'folder' ? (
          <Link to="/servers/$serverId/files" params={{ serverId }} search={{ path }} className={`${NAME} hover:underline`}>
            {name}
          </Link>
        ) : text ? (
          <Link to="/servers/$serverId/files/edit" params={{ serverId }} search={{ path }} className={`${NAME} hover:underline`}>
            {name}
          </Link>
        ) : entry.kind === 'file' ? (
          <a href={saved} download className={`${NAME} hover:underline`}>
            {name}
          </a>
        ) : (
          <span className={NAME}>{name}</span>
        )}
      </td>
      <td className="text-right text-small text-ink-subtle tabular-nums">{entry.kind === 'file' && bytes(entry.size)}</td>
      <td className="hidden truncate pl-6 text-small text-ink-subtle md:table-cell">{when(entry.modified)}</td>
      <td className="pr-1.5 text-right">
        {/* Not modal: a modal menu locks scrolling with a style element, which the content policy refuses. */}
        <DropdownMenu.Root modal={false}>
          <DropdownMenu.Trigger
            aria-label={`More for ${entry.name}`}
            className={`${ICON_BUTTON} opacity-0 group-hover:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100 [@media(hover:none)]:opacity-100`}
          >
            <Ellipsis aria-hidden size={16} />
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content align="end" sideOffset={4} className={`${FLOATING} min-w-44 p-1`}>
              {(entry.kind === 'file' || entry.kind === 'link') && (
                <DropdownMenu.Item asChild>
                  <a href={saved} download className={MENU_ITEM}>
                    <Download aria-hidden size={16} />
                    Download
                  </a>
                </DropdownMenu.Item>
              )}
              {archive && (
                <DropdownMenu.Item onSelect={onUnpack} className={MENU_ITEM}>
                  <PackageOpen aria-hidden size={16} />
                  Unpack here
                </DropdownMenu.Item>
              )}
              <DropdownMenu.Item onSelect={onMove} className={MENU_ITEM}>
                <Pencil aria-hidden size={16} />
                Rename or move
              </DropdownMenu.Item>
              <DropdownMenu.Item onSelect={onDelete} className={MENU_ITEM}>
                <Trash2 aria-hidden size={16} />
                Delete
              </DropdownMenu.Item>
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      </td>
    </tr>
  )
}

/** Where in the server's files this is, each folder on the way a link to itself. */
function Crumbs({ serverId, path }: { serverId: string; path: string }) {
  const parts = path ? path.split('/') : []
  const link = 'rounded-sm text-ink-subtle transition-colors duration-120 ease-out hover:text-ink'

  return (
    <nav aria-label="Folder" className="flex min-w-0 flex-wrap items-center gap-1 font-mono text-mono">
      {parts.length === 0 ? (
        <span aria-current="page" title="Top folder" className="text-ink">
          <House aria-hidden size={16} />
          <span className="sr-only">Top folder</span>
        </span>
      ) : (
        <Link to="/servers/$serverId/files" params={{ serverId }} search={{}} title="Top folder" className={link}>
          <House aria-hidden size={16} />
          <span className="sr-only">Top folder</span>
        </Link>
      )}
      {parts.map((part, index) => {
        const to = parts.slice(0, index + 1).join('/')
        return (
          <Fragment key={to}>
            <ChevronRight aria-hidden size={14} className="shrink-0 text-ink-faint" />
            {index === parts.length - 1 ? (
              <span aria-current="page" className="wrap-anywhere text-ink">
                {part}
              </span>
            ) : (
              <Link to="/servers/$serverId/files" params={{ serverId }} search={{ path: to }} className={`${link} wrap-anywhere`}>
                {part}
              </Link>
            )}
          </Fragment>
        )
      })}
    </nav>
  )
}

type NameFormProps = {
  label: string
  hint?: string
  start?: string
  submit: string
  busy: boolean
  onSubmit: (name: string) => void
  onCancel: () => void
}

/** One line to type a name into, in the page and not over it: a dialog is for a question (DESIGN.md, Don't). */
function NameForm({ label, hint, start = '', submit, busy, onSubmit, onCancel }: NameFormProps) {
  const [name, setName] = useState(start)
  return (
    <form
      className="flex flex-wrap items-start gap-2"
      onSubmit={(event) => {
        event.preventDefault()
        if (name.trim()) onSubmit(name.trim())
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') onCancel()
      }}
    >
      <div className="min-w-0 flex-1 md:max-w-96">
        <Field
          label={label}
          hint={hint}
          mono
          autoFocus
          value={name}
          onChange={(event) => setName(event.currentTarget.value)}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
        />
      </div>
      {/* Beside the field, under its label. */}
      <div className="mt-5.5 flex gap-2">
        <Button type="submit" variant="primary" busy={busy} disabled={!name.trim()}>
          {submit}
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  )
}

/** A file on its way to the server. */
type Uploading = { key: number; name: string; size: number; sent: number; problem?: string; stop: () => void }

/**
 * Sends files into a folder one after another, and says how far each has got.
 * One at a time: a home's upload is the narrow end, and two files at once
 * would only share it.
 */
function useUploads(id: number, folder: string, onArrived: () => void) {
  const [all, setAll] = useState<Uploading[]>([])
  const last = useRef<Promise<void>>(Promise.resolve())
  const keys = useRef(0)
  const dismiss = (key: number) => setAll((before) => before.filter((one) => one.key !== key))

  const add = (files: File[]) => {
    for (const file of files) {
      const key = keys.current++
      const stopper = new AbortController()
      const change = (to: Partial<Uploading>) => setAll((before) => before.map((one) => (one.key === key ? { ...one, ...to } : one)))
      setAll((before) => [...before, { key, name: file.name, size: file.size, sent: 0, stop: () => stopper.abort() }])
      last.current = last.current.then(async () => {
        // Stopped while it waited its turn.
        if (stopper.signal.aborted) return dismiss(key)
        try {
          await writeFile(id, joined(folder, file.name), file, (sent) => change({ sent }), stopper.signal)
          dismiss(key)
          onArrived()
        } catch (error) {
          if (stopper.signal.aborted) dismiss(key)
          else change({ problem: error instanceof Error ? error.message : String(error) })
        }
      })
    }
  }

  return { all, add, dismiss }
}

function UploadList({ uploads }: { uploads: ReturnType<typeof useUploads> }) {
  if (uploads.all.length === 0) return null
  return (
    <ul aria-label="Uploads" className="flex flex-col gap-3 rounded-lg border border-hairline bg-surface-1 p-3">
      {uploads.all.map((upload) => (
        <li key={upload.key} className="flex flex-col gap-1.5">
          <div className="flex items-center gap-3 text-small">
            <span className="min-w-0 flex-1 truncate font-mono text-ink">{upload.name}</span>
            {!upload.problem && (
              <span className="shrink-0 text-ink-subtle tabular-nums">
                {bytes(upload.sent)} of {bytes(upload.size)}
              </span>
            )}
            <button
              type="button"
              title={upload.problem ? 'Dismiss' : 'Stop'}
              onClick={() => (upload.problem ? uploads.dismiss(upload.key) : upload.stop())}
              className={`${ICON_BUTTON} size-6`}
            >
              <X aria-hidden size={14} />
              <span className="sr-only">{upload.problem ? 'Dismiss' : 'Stop'}</span>
            </button>
          </div>
          {upload.problem ? (
            <Problem>{upload.problem}</Problem>
          ) : (
            <div className="h-1.5 overflow-hidden rounded-full bg-surface-3">
              <div
                className="h-full rounded-full bg-ink-subtle"
                style={{ width: `${upload.size > 0 ? Math.min(upload.sent / upload.size, 1) * 100 : 100}%` }}
              />
            </div>
          )}
        </li>
      ))}
    </ul>
  )
}

/** What the server's files take of the disk. Wanted, not needed: it is asked for once the folder is painted. */
function DiskLine({ id }: { id: number }) {
  const { data } = useQuery(diskUseQuery(id))
  if (!data) return null
  return (
    <p className="text-small text-ink-subtle">
      This server&rsquo;s files take {bytes(data.used_bytes)}. {bytes(data.free_bytes)} are free on the disk they are on.
    </p>
  )
}

/**
 * Where a file-transfer program connects, for whoever has many files to move:
 * the account's name and this server's number, with the account's own password.
 * Not shown where this Homewarp serves no SFTP.
 */
function SftpLine({ id }: { id: number }) {
  const { data: session } = useSuspenseQuery(sessionQuery)
  if (!session.sftp_port || !session.user) return null
  const login = `${session.user.username}.${id}`
  // Opened by the panel's name, this page came through the VPS, and the VPS
  // passes SFTP on to nobody: the address in the bar is not where it is.
  if (window.location.protocol === 'https:') {
    return (
      <p className="text-small text-ink-subtle">
        For many files at once there is SFTP, from your home network only: port {session.sftp_port} of this machine there, as{' '}
        <code className="font-mono text-ink">{login}</code> with this account&rsquo;s password.
      </p>
    )
  }
  return (
    <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-small text-ink-subtle">
      For many files at once, SFTP
      <CopyChip text={`sftp://${login}@${window.location.hostname}:${session.sftp_port}`} />
      with this account&rsquo;s password.
    </p>
  )
}

/** A file's text, to read and to change. */
export function EditFileTab() {
  const { serverId } = editRoute.useParams()
  const { path = '' } = editRoute.useSearch()
  const id = Number(serverId)
  const loaded = useQuery(fileTextQuery(id, path))

  if (loaded.error) {
    return (
      <>
        <Crumbs serverId={serverId} path={path} />
        <Problem>{loaded.error.message}</Problem>
        <div>
          <a href={downloadUrl(id, path)} download className={buttonClass()}>
            <Download aria-hidden size={16} />
            Download
          </a>
        </div>
      </>
    )
  }
  if (loaded.isPending) return <Crumbs serverId={serverId} path={path} />
  // Keyed by the file, so that what was typed into one is never shown as another's.
  return <Editor key={path} serverId={serverId} path={path} start={loaded.data} />
}

/** `start` is the file's text as it was read, or null for a file that is not there yet. */
function Editor({ serverId, path, start }: { serverId: string; path: string; start: string | null }) {
  const id = Number(serverId)
  const queryClient = useQueryClient()
  // A text field keeps line ends as \n whatever it is given. A file from Windows gets its own back when saved.
  const windows = (start ?? '').includes('\r\n')
  const [saved, setSaved] = useState(() => (start ?? '').replaceAll('\r\n', '\n'))
  const [text, setText] = useState(saved)
  const [there, setThere] = useState(start !== null)
  const changed = text !== saved

  const saving = useMutation({
    mutationFn: (typed: string) => writeFile(id, path, windows ? typed.replaceAll('\n', '\r\n') : typed),
    onSuccess: (_, typed) => {
      setSaved(typed)
      setThere(true)
      void queryClient.invalidateQueries({ queryKey: filesQuery(id, parentOf(path)).queryKey })
      void queryClient.invalidateQueries({ queryKey: diskUseQuery(id).queryKey })
    },
  })
  const save = () => {
    if ((changed || !there) && !saving.isPending) saving.mutate(text)
  }
  // Leaving, by a link or by closing the tab, is asked about while there is something to lose.
  const leaving = useBlocker({ shouldBlockFn: () => changed, enableBeforeUnload: changed, withResolver: true })

  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <Crumbs serverId={serverId} path={path} />
        <div className="ml-auto flex items-center gap-2">
          {changed && <span className="text-small text-ink-subtle">Not saved</span>}
          {there && (
            <a href={downloadUrl(id, path)} download title="Download" className={buttonClass()}>
              <Download aria-hidden size={16} />
              <span className="sr-only md:not-sr-only">Download</span>
            </a>
          )}
          <Button variant="primary" busy={saving.isPending} disabled={there && !changed} onClick={save}>
            Save
          </Button>
        </div>
      </div>
      {!there && <p className="text-small text-ink-subtle">There is no such file yet. Saving makes it.</p>}
      {saving.error && <Problem>{saving.error.message}</Problem>}
      <textarea
        aria-label={`The text of ${nameOf(path)}`}
        value={text}
        onChange={(event) => setText(event.currentTarget.value)}
        onKeyDown={(event) => {
          if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
            event.preventDefault()
            save()
          }
        }}
        wrap="off"
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        className="h-[65dvh] min-h-64 w-full resize-y rounded-lg border border-hairline bg-surface-1 p-3 font-mono text-mono text-ink"
      />

      <Confirm
        open={leaving.status === 'blocked'}
        onClose={() => leaving.reset?.()}
        title="Leave without saving?"
        action={
          <Button variant="danger" onClick={() => leaving.proceed?.()}>
            Leave
          </Button>
        }
      >
        <p>What was changed in {nameOf(path)} has not been saved, and leaving drops it.</p>
      </Confirm>
    </>
  )
}
