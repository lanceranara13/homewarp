// Both fonts ship inside the app: the panel has to work on a LAN with no internet.
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import './styles.css'

import { MutationCache, QueryCache, QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import { SignedOut } from './api/client'
import { createAppRouter } from './router'
import { sessionQuery } from './session'

/**
 * A request has found that the session this page was opened with is over.
 * Saying so in the cached session is enough: the route guards, asked again,
 * lead to the sign-in page.
 */
function sessionEnded(error: Error) {
  if (!(error instanceof SignedOut) || !queryClient.getQueryData(sessionQuery.queryKey)?.user) return
  queryClient.setQueryData(sessionQuery.queryKey, (session) => session && { ...session, user: null })
  void router.invalidate()
}

const queryClient = new QueryClient({
  queryCache: new QueryCache({ onError: sessionEnded }),
  mutationCache: new MutationCache({ onError: sessionEnded }),
  defaultOptions: {
    queries: {
      // Asking again does not bring a session back.
      retry: (failures, error) => failures < 1 && !(error instanceof SignedOut),
      refetchOnWindowFocus: false,
    },
  },
})
const router = createAppRouter(queryClient)

const root = document.getElementById('root')
if (!root) throw new Error('index.html has no #root element')

createRoot(root).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
