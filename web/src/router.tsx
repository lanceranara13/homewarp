import type { QueryClient } from '@tanstack/react-query'
import {
  Link,
  Outlet,
  createRootRouteWithContext,
  createRoute,
  createRouter,
  notFound,
  redirect,
  type ErrorComponentProps,
} from '@tanstack/react-router'

import { AppShell } from './components/AppShell'
import { Button, Doorway, Problem } from './components/ui'
import { gateQuery } from './gate'
import { ImportTemplatePage } from './pages/ImportTemplatePage'
import { LoginPage } from './pages/LoginPage'
import { ConnectPage, NetworkPage } from './pages/NetworkPage'
import { ChooseTemplatePage, NewServerPage } from './pages/NewServerPage'
import { ServerPage, ServerSettingsPage } from './pages/ServerPage'
import { ServersPage } from './pages/ServersPage'
import { SetupPage } from './pages/SetupPage'
import { TemplatePage } from './pages/TemplatePage'
import { TemplatesPage } from './pages/TemplatesPage'
import { serverQuery, serversQuery } from './servers'
import { sessionQuery } from './session'
import { templateQuery, templatesQuery } from './templates'

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

/** The id in an address such as `/servers/7`, if a number is what is there. */
function idFrom(text: string | undefined): number | undefined {
  const id = Number(text)
  return Number.isSafeInteger(id) && id > 0 ? id : undefined
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
  beforeLoad: async ({ context, matches }) => {
    // What the page itself reads sets off now, beside the session check and not behind it,
    // unless that check has been answered already and the answer is that nobody is signed in.
    const known = context.queryClient.getQueryData(sessionQuery.queryKey)
    if (!known || known.user) {
      for (const match of matches) match.staticData.reads?.(context.queryClient, match.params)
    }
    // The sidebar's mark and the address chips want the Gate, and no page waits for it but the
    // Network pages. Where someone is known to be signed in it is asked for now, beside the rest;
    // on a first load the sidebar asks once it is there, which is no later than it could be used.
    if (known?.user) void context.queryClient.prefetchQuery(gateQuery)
    const to = await home(context)
    if (to !== '/') throw redirect({ to })
  },
  component: AppShell,
})

const serversRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/',
  staticData: { reads: (queryClient) => void queryClient.prefetchQuery(serversQuery) },
  loader: async ({ context }) => {
    await context.queryClient.ensureQueryData(serversQuery)
  },
  component: ServersPage,
})

const chooseTemplateRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/servers/new',
  staticData: { reads: (queryClient) => void queryClient.prefetchQuery(templatesQuery) },
  loader: async ({ context }) => {
    await context.queryClient.ensureQueryData(templatesQuery)
  },
  component: ChooseTemplatePage,
})

const newServerRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/servers/new/$templateId',
  staticData: {
    reads: (queryClient, params) => {
      const id = idFrom(params.templateId)
      if (id !== undefined) void queryClient.prefetchQuery(templateQuery(id))
      // For the first port that no server has.
      void queryClient.prefetchQuery(serversQuery)
    },
  },
  loader: async ({ context: { queryClient }, params }) => {
    const id = idFrom(params.templateId)
    if (id === undefined) throw notFound()
    const [template] = await Promise.all([
      queryClient.ensureQueryData(templateQuery(id)),
      queryClient.ensureQueryData(serversQuery),
    ])
    if (!template) throw notFound()
  },
  component: NewServerPage,
})

const serverRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/servers/$serverId',
  staticData: {
    reads: (queryClient, params) => {
      const id = idFrom(params.serverId)
      if (id !== undefined) void queryClient.prefetchQuery(serverQuery(id))
    },
  },
  loader: async ({ context, params }) => {
    const id = idFrom(params.serverId)
    if (id === undefined || !(await context.queryClient.ensureQueryData(serverQuery(id)))) throw notFound()
  },
  component: ServerPage,
})

const serverSettingsRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/servers/$serverId/settings',
  staticData: {
    reads: (queryClient, params) => {
      const id = idFrom(params.serverId)
      if (id !== undefined) void queryClient.prefetchQuery(serverQuery(id))
    },
  },
  loader: async ({ context: { queryClient }, params }) => {
    const id = idFrom(params.serverId)
    const server = id === undefined ? null : await queryClient.ensureQueryData(serverQuery(id))
    // Which template it is made from is known only once the server is: the one read that has to wait.
    // Reached from the server's own page, the server is known already and this is the only request.
    if (!server || !(await queryClient.ensureQueryData(templateQuery(server.template_id)))) throw notFound()
  },
  component: ServerSettingsPage,
})

const templatesRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/templates',
  staticData: { reads: (queryClient) => void queryClient.prefetchQuery(templatesQuery) },
  loader: async ({ context }) => {
    await context.queryClient.ensureQueryData(templatesQuery)
  },
  component: TemplatesPage,
})

const importTemplateRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/templates/import',
  component: ImportTemplatePage,
})

const templateRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/templates/$templateId',
  staticData: {
    reads: (queryClient, params) => {
      const id = idFrom(params.templateId)
      if (id !== undefined) void queryClient.prefetchQuery(templateQuery(id))
    },
  },
  loader: async ({ context, params }) => {
    const id = idFrom(params.templateId)
    if (id === undefined || !(await context.queryClient.ensureQueryData(templateQuery(id)))) throw notFound()
  },
  component: TemplatePage,
})

const networkRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/network',
  staticData: { reads: (queryClient) => void queryClient.prefetchQuery(gateQuery) },
  loader: async ({ context }) => {
    await context.queryClient.ensureQueryData(gateQuery)
  },
  component: NetworkPage,
})

const connectRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/network/connect',
  staticData: { reads: (queryClient) => void queryClient.prefetchQuery(gateQuery) },
  loader: async ({ context }) => {
    await context.queryClient.ensureQueryData(gateQuery)
  },
  component: ConnectPage,
})

const routeTree = rootRoute.addChildren([
  setupRoute,
  loginRoute,
  shellRoute.addChildren([
    serversRoute,
    chooseTemplateRoute,
    newServerRoute,
    serverRoute,
    serverSettingsRoute,
    templatesRoute,
    importTemplateRoute,
    templateRoute,
    networkRoute,
    connectRoute,
  ]),
])

export function createAppRouter(queryClient: QueryClient) {
  return createRouter({ routeTree, context: { queryClient } })
}

declare module '@tanstack/react-router' {
  interface Register {
    router: ReturnType<typeof createAppRouter>
  }

  interface StaticDataRouteOption {
    /**
     * Starts the requests a page cannot paint without. The shell calls it while
     * the session is still being checked; the page's loader then waits for the
     * same requests, which are already on their way.
     */
    reads?: (queryClient: QueryClient, params: Record<string, string | undefined>) => void
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
