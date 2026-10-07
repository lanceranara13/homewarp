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

import { accountsQuery, activityQuery, serverUsersQuery, settingsQuery, twoStepsQuery } from './accounts'
import { AppShell } from './components/AppShell'
import { Button, Doorway, Problem } from './components/ui'
import { filesQuery } from './files'
import { gateQuery } from './gate'
import { ActivityPage } from './pages/ActivityPage'
import { BackupsTab } from './pages/BackupsPage'
import { EditFileTab, FilesTab } from './pages/FilesPage'
import { ImportTemplatePage } from './pages/ImportTemplatePage'
import { LoginPage } from './pages/LoginPage'
import { ConnectPage, NetworkPage } from './pages/NetworkPage'
import { ChooseTemplatePage, NewServerPage } from './pages/NewServerPage'
import { ModsTab } from './pages/ModsPage'
import { SchedulesTab } from './pages/SchedulesPage'
import { ConsoleTab, ServerLayout, ServerSettingsTab } from './pages/ServerPage'
import { ServerUsersTab } from './pages/ServerUsersPage'
import { ServersPage } from './pages/ServersPage'
import { SettingsPage } from './pages/SettingsPage'
import { SetupPage } from './pages/SetupPage'
import { TemplatePage } from './pages/TemplatePage'
import { TemplatesPage } from './pages/TemplatesPage'
import { backupsQuery, schedulesQuery, serverQuery, serversQuery } from './servers'
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

/**
 * Sends anyone but the owner back to their servers: for the pages that are the
 * owner's alone. Homewarp refuses what such a page would ask anyway; this
 * keeps an account from being shown a page that could do nothing for it.
 */
async function ownersOnly({ queryClient }: Context) {
  const session = await queryClient.ensureQueryData(sessionQuery)
  if (session.user && !session.user.owner) throw redirect({ to: '/' })
}

/** A whole number above nothing out of an address's search, where a number is what is there. */
function numberFrom(value: unknown): number | undefined {
  const number = Number(value)
  return Number.isSafeInteger(number) && number > 0 ? number : undefined
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
      for (const match of matches) match.staticData.reads?.(context.queryClient, match.params, match.search)
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
  beforeLoad: ({ context }) => ownersOnly(context),
  staticData: { reads: (queryClient) => void queryClient.prefetchQuery(templatesQuery) },
  loader: async ({ context }) => {
    await context.queryClient.ensureQueryData(templatesQuery)
  },
  component: ChooseTemplatePage,
})

const newServerRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/servers/new/$templateId',
  beforeLoad: ({ context }) => ownersOnly(context),
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
  component: ServerLayout,
})

/** The tabs of a server (DESIGN.md, Inside a server). The console is the one a server opens on. */
const consoleRoute = createRoute({
  getParentRoute: () => serverRoute,
  path: '/',
  component: ConsoleTab,
})

/** Which folder, or which file, of a server's files an address means. None is the top folder. */
function pathFrom(search: Record<string, unknown>): { path?: string } {
  return typeof search.path === 'string' && search.path !== '' ? { path: search.path } : {}
}

const filesRoute = createRoute({
  getParentRoute: () => serverRoute,
  path: 'files',
  validateSearch: pathFrom,
  staticData: {
    reads: (queryClient, params, search) => {
      const id = idFrom(params.serverId)
      if (id !== undefined) void queryClient.prefetchQuery(filesQuery(id, pathFrom(search).path ?? ''))
    },
  },
  loaderDeps: ({ search }) => ({ path: search.path ?? '' }),
  loader: async ({ context, params, deps }) => {
    const id = idFrom(params.serverId)
    // A folder that cannot be listed is the page's to say in words, not this loader's to fail on.
    if (id !== undefined) await context.queryClient.ensureQueryData(filesQuery(id, deps.path)).catch(() => null)
  },
  component: FilesTab,
})

const editFileRoute = createRoute({
  getParentRoute: () => serverRoute,
  path: 'files/edit',
  validateSearch: pathFrom,
  component: EditFileTab,
})

const serverSettingsRoute = createRoute({
  getParentRoute: () => serverRoute,
  path: 'settings',
  loader: async ({ context: { queryClient }, params }) => {
    const id = idFrom(params.serverId)
    const server = id === undefined ? null : await queryClient.ensureQueryData(serverQuery(id))
    // Which template it is made from is known only once the server is: the one read that has to wait.
    // Reached from the server's own page, the server is known already and this is the only request.
    if (!server || !(await queryClient.ensureQueryData(templateQuery(server.template_id)))) throw notFound()
  },
  component: ServerSettingsTab,
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
  beforeLoad: ({ context }) => ownersOnly(context),
  // Come to from the new-server wizard, the page leads back into it once the egg is imported.
  validateSearch: (search: Record<string, unknown>): { then?: 'server' } =>
    search.then === 'server' ? { then: 'server' } : {},
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
  beforeLoad: ({ context }) => ownersOnly(context),
  staticData: { reads: (queryClient) => void queryClient.prefetchQuery(gateQuery) },
  loader: async ({ context }) => {
    await context.queryClient.ensureQueryData(gateQuery)
  },
  component: ConnectPage,
})

const backupsRoute = createRoute({
  getParentRoute: () => serverRoute,
  path: 'backups',
  loader: async ({ context, params }) => {
    const id = idFrom(params.serverId)
    if (id === undefined) throw notFound()
    await context.queryClient.ensureQueryData(backupsQuery(id))
  },
  component: BackupsTab,
})

/** Nothing is asked for before it is shown: Modrinth is asked when somebody searches. */
const modsRoute = createRoute({
  getParentRoute: () => serverRoute,
  path: 'mods',
  component: ModsTab,
})

const schedulesRoute = createRoute({
  getParentRoute: () => serverRoute,
  path: 'schedules',
  loader: async ({ context, params }) => {
    const id = idFrom(params.serverId)
    if (id === undefined) throw notFound()
    await context.queryClient.ensureQueryData(schedulesQuery(id))
  },
  component: SchedulesTab,
})

/** Who else is let into a server, and what they may do there: the owner's to say. */
const serverUsersRoute = createRoute({
  getParentRoute: () => serverRoute,
  path: 'users',
  beforeLoad: ({ context }) => ownersOnly(context),
  loader: async ({ context: { queryClient }, params }) => {
    const id = idFrom(params.serverId)
    if (id === undefined) throw notFound()
    await Promise.all([queryClient.ensureQueryData(serverUsersQuery(id)), queryClient.ensureQueryData(accountsQuery)])
  },
  component: ServerUsersTab,
})

const activityRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/activity',
  validateSearch: (search: Record<string, unknown>): { server?: number; user?: number } => ({
    server: numberFrom(search.server),
    user: numberFrom(search.user),
  }),
  beforeLoad: ({ context }) => ownersOnly(context),
  loaderDeps: ({ search }) => search,
  loader: async ({ context, deps }) => {
    await context.queryClient.ensureInfiniteQueryData(activityQuery(deps.server, deps.user))
  },
  component: ActivityPage,
})

const settingsRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: '/settings',
  loader: async ({ context: { queryClient } }) => {
    // The other accounts are the owner's to see, and so only the owner's page waits for them.
    // What each account has of its own, and what the owner has of everyone's, set off together.
    const session = await queryClient.ensureQueryData(sessionQuery)
    const owners = session.user?.owner ? [queryClient.ensureQueryData(accountsQuery), queryClient.ensureQueryData(settingsQuery)] : []
    await Promise.all([queryClient.ensureQueryData(twoStepsQuery), ...owners])
  },
  component: SettingsPage,
})

const routeTree = rootRoute.addChildren([
  setupRoute,
  loginRoute,
  shellRoute.addChildren([
    serversRoute,
    chooseTemplateRoute,
    newServerRoute,
    serverRoute.addChildren([
      consoleRoute,
      filesRoute,
      editFileRoute,
      backupsRoute,
      modsRoute,
      schedulesRoute,
      serverUsersRoute,
      serverSettingsRoute,
    ]),
    activityRoute,
    settingsRoute,
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
    reads?: (queryClient: QueryClient, params: Record<string, string | undefined>, search: Record<string, unknown>) => void
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
