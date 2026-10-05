// Both fonts ship inside the app: the panel has to work on a LAN with no internet.
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import './styles.css'

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import { createAppRouter } from './router'

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } },
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
