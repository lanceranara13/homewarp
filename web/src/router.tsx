import type { QueryClient } from '@tanstack/react-query'
import {
  Link,
  Outlet,
  createRootRouteWithContext,
  createRoute,
  createRouter,
  redirect,
  type ErrorComponentProps,
} from '@tanstack/react-router'

import { AppShell } from './components/AppShell'
import { Button, Doorway, Problem } from './components/ui'
import { LoginPage } from './pages/LoginPage'
import { ServersPage } from './pages/ServersPage'
import { SetupPage } from './pages/SetupPage'
import { sessionQuery } from './session'

interface Context {
  queryClient: QueryClient
}

/**
 * Where a visitor belongs, given the session: setup while there is no account,
 * sign-in while nobody is signed in, the panel otherwise. Every guard asks the
 * same cached session, so a page load makes that request once.
 */
async function home({ queryClient }: Context): Promise<'/setup' | '/login' | '/'> {
  const session = await queryClient.ensureQueryData(sessionQuery)
  if (session.setup_required) return '/setup'
  return session.user ? '/' : '/login'
}

const rootRoute = createRootRouteWithContext<Context>()({
  component: Outlet,
  errorComponent: Unreachable,
  notFoundComponent: NotFound,
})

const setupRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/setup',
  beforeLoad: async ({ context }) => {
    const to = await home(context)
    if (to !== '/setup') throw redirect({ to })
  },
  component: SetupPage,
})

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/login',
  beforeLoad: async ({ context }) => {
    const to = await home(context)
    if (to !== '/login') throw redirect({ to })
  },
  component: LoginPage,
})

const shellRoute = createRoute({
  getParentRoute: () => rootRoute,
  id: 'shell',
  beforeLoad: async ({ context }) => {
    const to = await home(context)
    if (to !== '/') throw redirect({ to })
  },
  component: AppShell,
})

const serversRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/',
  component: ServersPage,
})

const routeTree = rootRoute.addChildren([setupRoute, loginRoute, shellRoute.addChildren([serversRoute])])

export function createAppRouter(queryClient: QueryClient) {
  return createRouter({ routeTree, context: { queryClient } })
}

declare module '@tanstack/react-router' {
  interface Register {
    router: ReturnType<typeof createAppRouter>
  }
}

function Unreachable({ error }: ErrorComponentProps) {
  return (
    <Doorway title="Homewarp is not answering" lead="The page could not reach the panel it came from.">
      <div className="flex flex-col items-start gap-4">
        <Problem>{error instanceof Error ? error.message : String(error)}</Problem>
        <Button onClick={() => window.location.reload()}>Try again</Button>
      </div>
    </Doorway>
  )
}

function NotFound() {
  return (
    <Doorway title="Nothing here" lead="This address does not lead to a page.">
      <Link to="/" className="text-accent hover:underline">
        Back to your servers
      </Link>
    </Doorway>
  )
}
