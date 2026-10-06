import createClient from 'openapi-fetch'

// Generated from ../../openapi.json by `npm run gen`; that file is written by
// `scripts/dev.sh gen` from the Rust handlers.
import type { components, paths } from './schema'

export type Session = components['schemas']['Session']
export type TemplateSummary = components['schemas']['TemplateSummary']
export type Template = components['schemas']['Template']
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
type NewGate = components['schemas']['NewGate']
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

export async function signIn(body: LoginRequest): Promise<Session> {
  const { data, error } = await reach(() => api.POST('/api/v1/login', { body }))
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
