import createClient from 'openapi-fetch'

// Generated from ../../openapi.json by `npm run gen`; that file is written by
// `scripts/dev.sh gen` from the Rust handlers.
import type { components, paths } from './schema'

export type Session = components['schemas']['Session']
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
