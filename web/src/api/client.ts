import createClient from 'openapi-fetch'

// Generated from ../../openapi.json by `npm run gen`; that file is written by
// `scripts/dev.sh gen` from the Rust handlers.
import type { components, paths } from './schema'

export type Session = components['schemas']['Session']
export type TemplateSummary = components['schemas']['TemplateSummary']
export type Template = components['schemas']['Template']
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
