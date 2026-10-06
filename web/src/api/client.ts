import createClient from 'openapi-fetch'

// Generated from ../../openapi.json by `npm run gen`; that file is written by
// `scripts/dev.sh gen` from the Rust handlers.
import type { components, paths } from './schema'

export type Session = components['schemas']['Session']
export type TemplateSummary = components['schemas']['TemplateSummary']
export type Template = components['schemas']['Template']
export type Catalogue = components['schemas']['Catalogue']
export type ServerSummary = components['schemas']['ServerSummary']
export type Server = components['schemas']['Server']
export type ServerState = components['schemas']['State']
export type NewServer = components['schemas']['NewServer']
export type ServerSettings = components['schemas']['ServerSettings']
export type Power = components['schemas']['Power']
export type ServerEvent = components['schemas']['Event']
export type Usage = components['schemas']['Usage']
export type Gate = components['schemas']['GateView']
export type PortProtocol = components['schemas']['PortProtocol']
export type ExtraPort = components['schemas']['ExtraPort']
export type FileEntry = components['schemas']['FileEntry']
export type DiskUse = components['schemas']['DiskUse']
export type Unpacked = components['schemas']['Unpacked']
export type Account = components['schemas']['Account']
export type Permission = components['schemas']['Permission']
export type ServerUser = components['schemas']['ServerUser']
export type ActivityEntry = components['schemas']['ActivityEntry']
type NewAccount = components['schemas']['NewAccount']
export type Backup = components['schemas']['Backup']
export type Backups = components['schemas']['Backups']
export type Schedule = components['schemas']['Schedule']
export type ScheduleSettings = components['schemas']['ScheduleSettings']
export type Task = components['schemas']['Task']
export type TaskAction = components['schemas']['Action']
export type Settings = components['schemas']['Settings']
type SettingsChange = components['schemas']['SettingsChange']
export type Panel = components['schemas']['PanelView']
export type VpsGuard = components['schemas']['VpsGuard']
export type Passkeys = components['schemas']['Passkeys']
export type MakeOptions = components['schemas']['MakeOptions']
export type NewPasskey = components['schemas']['NewPasskey']
export type SignInOptions = components['schemas']['SignInOptions']
export type PasskeySignIn = components['schemas']['PasskeySignIn']
type NewGate = components['schemas']['NewGate']
type PanelChange = components['schemas']['PanelChange']
type SetupRequest = components['schemas']['SetupRequest']
type LoginRequest = components['schemas']['LoginRequest']

const api = createClient<paths>()

/** Throws the sentence the server sent, or one of our own if it sent none. */
function fail(problem: unknown): never {
  const sentence =
    typeof problem === 'object' && problem !== null && 'error' in problem && typeof problem.error === 'string'
      ? problem.error
      : 'Homewarp gave an answer this page does not understand.'
  throw new Error(sentence)
}

/** A request that never reached Homewarp fails here, with words a person can act on. */
async function reach<T>(request: () => Promise<T>): Promise<T> {
  try {
    return await request()
  } catch {
    throw new Error('Homewarp is not answering. Check that it is running.')
  }
}

export async function getSession(): Promise<Session> {
  const { data, error } = await reach(() => api.GET('/api/v1/session'))
  return data ?? fail(error)
}

export async function setUp(body: SetupRequest): Promise<Session> {
  const { data, error } = await reach(() => api.POST('/api/v1/setup', { body }))
  return data ?? fail(error)
}

/** The password was right, and the second step of the sign-in is still to do, or was got wrong. */
export class CodeNeeded extends Error {}

export async function signIn(body: LoginRequest): Promise<Session> {
  const { data, error } = await reach(() => api.POST('/api/v1/login', { body }))
  if (error?.code_required) throw new CodeNeeded(error.error)
  return data ?? fail(error)
}

export type TwoSteps = components['schemas']['TwoSteps']
export type TwoStepsSetup = components['schemas']['TwoStepsSetup']

/** Whether the account signed in here has a second step, and how many recovery codes it has left. */
export async function getTwoSteps(): Promise<TwoSteps> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/account/two-steps'))
  return data ?? fail(error)
}

/** Makes a secret for an authenticator app. Nothing changes until a code from the app is typed back. */
export async function beginTwoSteps(): Promise<TwoStepsSetup> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/account/two-steps'))
  return data ?? fail(error)
}

/** Turns the second step on. The answer is the recovery codes, which are shown this once. */
export async function confirmTwoSteps(code: string): Promise<string[]> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/account/two-steps/confirm', { body: { code } }))
  return data ? data.recovery_codes : fail(error)
}

/** Turns the second step off, given the account's password. */
export async function endTwoSteps(password: string): Promise<void> {
  const { error, response } = await signedIn(() => api.POST('/api/v1/account/two-steps/off', { body: { password } }))
  if (!response.ok) fail(error)
}

/** The passkeys of the account signed in here, and whether one can be made where this page is. */
export async function listPasskeys(): Promise<Passkeys> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/account/passkeys'))
  return data ?? fail(error)
}

/** Begins making a passkey, given the account's password. The answer is what the browser asks its device for. */
export async function beginPasskey(password: string): Promise<MakeOptions> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/account/passkeys/begin', { body: { password } }))
  return data ?? fail(error)
}

/** Keeps a passkey the browser has just had made. */
export async function addPasskey(body: NewPasskey): Promise<void> {
  const { error, response } = await signedIn(() => api.POST('/api/v1/account/passkeys', { body }))
  if (!response.ok) fail(error)
}

export async function removePasskey(id: number): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.DELETE('/api/v1/account/passkeys/{id}', { params: { path: { id } } }),
  )
  if (!response.ok) fail(error)
}

/** Begins a sign-in with a passkey. Nobody is named: the device says which key it has. */
export async function beginPasskeySignIn(): Promise<SignInOptions> {
  const { data, error } = await reach(() => api.POST('/api/v1/login/passkey/begin'))
  return data ?? fail(error)
}

/** Signs in with what a device signed. */
export async function passkeySignIn(body: PasskeySignIn): Promise<Session> {
  const { data, error } = await reach(() => api.POST('/api/v1/login/passkey', { body }))
  return data ?? fail(error)
}

export async function signOut(): Promise<void> {
  const { error, response } = await reach(() => api.POST('/api/v1/logout'))
  if (!response.ok) fail(error)
}

/** A session that ended while its page was open: it ran out, or was signed out in another tab. */
export class SignedOut extends Error {
  constructor() {
    super('Your session has ended. Sign in again.')
  }
}

/** For the endpoints that answer only someone signed in. */
async function signedIn<T extends { response: Response }>(request: () => Promise<T>): Promise<T> {
  const answer = await reach(request)
  if (answer.response.status === 401) throw new SignedOut()
  return answer
}

export async function listTemplates(): Promise<TemplateSummary[]> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/templates'))
  return data ?? fail(error)
}

/** Null where there is no template with that id. */
export async function getTemplate(id: number): Promise<Template | null> {
  const { data, error, response } = await signedIn(() => api.GET('/api/v1/templates/{id}', { params: { path: { id } } }))
  if (response.status === 404) return null
  return data ?? fail(error)
}

/** Sends an egg's text; the answer is the template Homewarp made of it. */
/** An egg's text, fetched by Homewarp from the address it is published at. Nothing is imported by this. */
export async function fetchEgg(url: string): Promise<string> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/templates/fetch', { body: { url } }))
  return data ? data.egg : fail(error)
}

/** The eggs there are to be had, as the list was last fetched. Empty until it has been. */
export async function getCatalogue(): Promise<Catalogue> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/catalogue'))
  return data ?? fail(error)
}

/** Fetches the list afresh from where the eggs are published. */
export async function refreshCatalogue(): Promise<Catalogue> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/catalogue'))
  return data ?? fail(error)
}

export async function importTemplate(egg: string): Promise<Template> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/templates', { body: { egg } }))
  return data ?? fail(error)
}

export async function removeTemplate(id: number): Promise<void> {
  const { error, response } = await signedIn(() => api.DELETE('/api/v1/templates/{id}', { params: { path: { id } } }))
  if (!response.ok) fail(error)
}

export async function listServers(): Promise<ServerSummary[]> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/servers'))
  return data ?? fail(error)
}

/** Null where there is no server with that id. */
export async function getServer(id: number): Promise<Server | null> {
  const { data, error, response } = await signedIn(() => api.GET('/api/v1/servers/{id}', { params: { path: { id } } }))
  if (response.status === 404) return null
  return data ?? fail(error)
}

/** Makes a server. The answer comes at once, while it is still being installed. */
export async function createServer(body: NewServer): Promise<Server> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/servers', { body }))
  return data ?? fail(error)
}

/** Changes what a stopped server is made of. The answer is the server as it now is. */
export async function changeServer(id: number, body: ServerSettings): Promise<Server> {
  const { data, error } = await signedIn(() => api.PUT('/api/v1/servers/{id}', { params: { path: { id } }, body }))
  return data ?? fail(error)
}

/** Asks a server to start, stop and so on. It has been asked, not yet obeyed, when this returns. */
export async function powerServer(id: number, action: Power): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/power', { params: { path: { id } }, body: { action } }),
  )
  if (!response.ok) fail(error)
}

/** Types one line into a server's console. */
export async function commandServer(id: number, command: string): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/command', { params: { path: { id } }, body: { command } }),
  )
  if (!response.ok) fail(error)
}

/**
 * Opens the socket a page follows a server by. Each message on it is a
 * `ServerEvent` in JSON: first where things stand, then what happens.
 */
export function followServer(id: number): WebSocket {
  const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws'
  return new WebSocket(`${scheme}://${window.location.host}/api/v1/servers/${id}/console`)
}

export async function removeServer(id: number): Promise<void> {
  const { error, response } = await signedIn(() => api.DELETE('/api/v1/servers/{id}', { params: { path: { id } } }))
  if (!response.ok) fail(error)
}

/** What is in one folder of a server's files, folders first. Null where there is no such folder. */
export async function listFiles(id: number, path: string): Promise<FileEntry[] | null> {
  const { data, error, response } = await signedIn(() =>
    api.GET('/api/v1/servers/{id}/files', { params: { path: { id }, query: { path } } }),
  )
  if (response.status === 404) return null
  return data ?? fail(error)
}

/** A file's text. Null where there is no such file. What is not text, or is too long, is refused in words. */
export async function readFile(id: number, path: string): Promise<string | null> {
  const { data, error, response } = await signedIn(() =>
    api.GET('/api/v1/servers/{id}/files/content', { params: { path: { id }, query: { path } } }),
  )
  if (response.status === 404) return null
  return data ? data.text : fail(error)
}

/**
 * Sends a file to a server: an upload, or what the editor saves. It takes the
 * place of a file that is there by that name. Not sent with `fetch`, which
 * does not say how far an upload has got.
 */
export function writeFile(
  id: number,
  path: string,
  body: Blob | string,
  sent?: (bytes: number) => void,
  stop?: AbortSignal,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest()
    request.open('PUT', `/api/v1/servers/${id}/files/content?path=${encodeURIComponent(path)}`)
    request.upload.onprogress = (event) => sent?.(event.loaded)
    request.onload = () => {
      if (request.status >= 200 && request.status < 300) return resolve()
      if (request.status === 401) return reject(new SignedOut())
      try {
        fail(JSON.parse(request.responseText))
      } catch (error) {
        // What `fail` threw, or what an answer that is not JSON made of itself.
        reject(error instanceof SyntaxError ? new Error('Homewarp gave an answer this page does not understand.') : error)
      }
    }
    request.onerror = () => reject(new Error('Homewarp is not answering. Check that it is running.'))
    request.onabort = () => reject(new Error('The upload was stopped.'))
    stop?.addEventListener('abort', () => request.abort())
    request.send(body)
  })
}

/** Where the browser fetches a file from to save it. */
export function downloadUrl(id: number, path: string): string {
  return `/api/v1/servers/${id}/files/download?path=${encodeURIComponent(path)}`
}

export async function makeFolder(id: number, path: string): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/files/folder', { params: { path: { id } }, body: { path } }),
  )
  if (!response.ok) fail(error)
}

/** Moves a file or a folder, which is also how it gets another name. Never onto something that is there. */
export async function moveFile(id: number, from: string, to: string): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/files/move', { params: { path: { id } }, body: { from, to } }),
  )
  if (!response.ok) fail(error)
}

/** Deletes files, and folders with all that is in them. There is no bringing them back. */
export async function removeFiles(id: number, paths: string[]): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/files/remove', { params: { path: { id } }, body: { paths } }),
  )
  if (!response.ok) fail(error)
}

/** Packs what is named, in a folder, into one archive there. The answer is the archive's name. */
export async function packFiles(id: number, folder: string, names: string[]): Promise<string> {
  const { data, error } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/files/pack', { params: { path: { id } }, body: { folder, names } }),
  )
  return data ? data.name : fail(error)
}

/** Unpacks an archive into the folder it is in. */
export async function unpackFile(id: number, path: string): Promise<Unpacked> {
  const { data, error } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/files/unpack', { params: { path: { id } }, body: { path } }),
  )
  return data ?? fail(error)
}

/** What a server's files take of the disk, and what is left of it. */
export async function getDiskUse(id: number): Promise<DiskUse> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/servers/{id}/files/usage', { params: { path: { id } } }))
  return data ?? fail(error)
}

/** A server's backups, the newest first, and how many are kept. */
export async function listBackups(id: number): Promise<Backups> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/servers/{id}/backups', { params: { path: { id } } }))
  return data ?? fail(error)
}

/** Begins a backup. The answer comes at once, with the backup still being made. */
export async function makeBackup(id: number, name: string): Promise<Backup> {
  const { data, error } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/backups', { params: { path: { id } }, body: { name } }),
  )
  return data ?? fail(error)
}

/** Says how many finished backups of a server are kept, from the next one that is done. */
export async function keepBackups(id: number, kept: number): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.PUT('/api/v1/servers/{id}/backups/kept', { params: { path: { id } }, body: { kept } }),
  )
  if (!response.ok) fail(error)
}

/** Where the browser fetches a backup's file from to save it. */
export function backupUrl(id: number, backup: number): string {
  return `/api/v1/servers/${id}/backups/${backup}/download`
}

/** Begins putting a backup back: the server's files become what it says they were. */
export async function restoreBackup(id: number, backup: number): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/backups/{backup_id}/restore', { params: { path: { id, backup_id: backup } } }),
  )
  if (!response.ok) fail(error)
}

export async function removeBackup(id: number, backup: number): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.DELETE('/api/v1/servers/{id}/backups/{backup_id}', { params: { path: { id, backup_id: backup } } }),
  )
  if (!response.ok) fail(error)
}

/** What a server does by the clock, by name. */
export async function listSchedules(id: number): Promise<Schedule[]> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/servers/{id}/schedules', { params: { path: { id } } }))
  return data ?? fail(error)
}

export async function createSchedule(id: number, body: ScheduleSettings): Promise<Schedule> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/servers/{id}/schedules', { params: { path: { id } }, body }))
  return data ?? fail(error)
}

export async function changeSchedule(id: number, schedule: number, body: ScheduleSettings): Promise<Schedule> {
  const { data, error } = await signedIn(() =>
    api.PUT('/api/v1/servers/{id}/schedules/{schedule_id}', { params: { path: { id, schedule_id: schedule } }, body }),
  )
  return data ?? fail(error)
}

export async function removeSchedule(id: number, schedule: number): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.DELETE('/api/v1/servers/{id}/schedules/{schedule_id}', { params: { path: { id, schedule_id: schedule } } }),
  )
  if (!response.ok) fail(error)
}

/** Sets a schedule off now, whatever its times. What the run came to is then in the schedule. */
export async function runSchedule(id: number, schedule: number): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.POST('/api/v1/servers/{id}/schedules/{schedule_id}/run', { params: { path: { id, schedule_id: schedule } } }),
  )
  if (!response.ok) fail(error)
}

/** What is set for this Homewarp as a whole. Only the owner may ask. */
export async function getSettings(): Promise<Settings> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/settings'))
  return data ?? fail(error)
}

/** Changes what is named, and leaves the rest. The answer is the settings as they are kept. */
export async function changeSettings(body: SettingsChange): Promise<Settings> {
  const { data, error } = await signedIn(() => api.PUT('/api/v1/settings', { body }))
  return data ?? fail(error)
}

/** Every account, the owner's first. Only the owner may ask. */
export async function listAccounts(): Promise<Account[]> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/users'))
  return data ?? fail(error)
}

/** Makes an account. It sees no server until it is let into one. */
export async function createAccount(body: NewAccount): Promise<Account> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/users', { body }))
  return data ?? fail(error)
}

export async function removeAccount(id: number): Promise<void> {
  const { error, response } = await signedIn(() => api.DELETE('/api/v1/users/{id}', { params: { path: { id } } }))
  if (!response.ok) fail(error)
}

/** Gives an account another password, and signs it out wherever it was signed in. */
export async function setPassword(id: number, password: string): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.PUT('/api/v1/users/{id}/password', { params: { path: { id } }, body: { password } }),
  )
  if (!response.ok) fail(error)
}

/** Changes the password of whoever is signed in here. Their other sessions end. */
export async function changeOwnPassword(current: string, password: string): Promise<void> {
  const { error, response } = await signedIn(() => api.POST('/api/v1/account/password', { body: { current, password } }))
  if (!response.ok) fail(error)
}

/** The accounts that have been let into a server, and what each may do there. */
export async function listServerUsers(id: number): Promise<ServerUser[]> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/servers/{id}/users', { params: { path: { id } } }))
  return data ?? fail(error)
}

/** Lets an account into a server, or changes what it may do there. With nothing named it may look. */
export async function letIn(id: number, userId: number, permissions: Permission[]): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.PUT('/api/v1/servers/{id}/users/{user_id}', { params: { path: { id, user_id: userId } }, body: { permissions } }),
  )
  if (!response.ok) fail(error)
}

export async function turnOut(id: number, userId: number): Promise<void> {
  const { error, response } = await signedIn(() =>
    api.DELETE('/api/v1/servers/{id}/users/{user_id}', { params: { path: { id, user_id: userId } } }),
  )
  if (!response.ok) fail(error)
}

/** What was done through the panel, the newest first, a hundred lines at a time. */
export async function listActivity(asked: { server?: number; user?: number; before?: number }): Promise<ActivityEntry[]> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/activity', { params: { query: asked } }))
  return data ?? fail(error)
}

/** The Gate, how the tunnel to it is doing, and every port of every server. */
export async function getGate(): Promise<Gate> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/gate'))
  return data ?? fail(error)
}

/** Starts connecting a VPS. The answer carries the one command to run there. */
export async function connectGate(body: NewGate): Promise<Gate> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/gate', { body }))
  return data ?? fail(error)
}

/** Has Homewarp find out again whether servers see their players' own addresses. It takes a few seconds. */
export async function checkGate(): Promise<Gate> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/gate/check'))
  return data ?? fail(error)
}

export async function disconnectGate(): Promise<void> {
  const { error, response } = await signedIn(() => api.DELETE('/api/v1/gate'))
  if (!response.ok) fail(error)
}

/** The guard on the VPS itself, and what is listening there. Only the owner may ask. */
export async function getGuard(): Promise<VpsGuard> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/gate/guard'))
  return data ?? fail(error)
}

/** Hardens the VPS, on trial: it is undone by itself in a minute unless it is kept. */
export async function hardenVps(): Promise<VpsGuard> {
  const { data, error } = await signedIn(() => api.PUT('/api/v1/gate/guard'))
  return data ?? fail(error)
}

/** Keeps a guard that is on trial. */
export async function keepGuard(): Promise<VpsGuard> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/gate/guard/keep'))
  return data ?? fail(error)
}

/** Takes the guard off the VPS, kept or on trial. */
export async function unhardenVps(): Promise<VpsGuard> {
  const { data, error } = await signedIn(() => api.DELETE('/api/v1/gate/guard'))
  return data ?? fail(error)
}

/** The name the panel is reached by from the internet, and the certificate for it. Only the owner may ask. */
export async function getPanel(): Promise<Panel> {
  const { data, error } = await signedIn(() => api.GET('/api/v1/panel'))
  return data ?? fail(error)
}

/** Gives the panel a name, or with none takes it off the internet again. */
export async function changePanel(body: PanelChange): Promise<Panel> {
  const { data, error } = await signedIn(() => api.PUT('/api/v1/panel', { body }))
  return data ?? fail(error)
}

/** Asks for the certificate again now, after an asking that failed. */
export async function askForCertificate(): Promise<Panel> {
  const { data, error } = await signedIn(() => api.POST('/api/v1/panel/certificate'))
  return data ?? fail(error)
}
