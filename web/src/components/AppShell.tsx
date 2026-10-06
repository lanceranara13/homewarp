import { useSuspenseQuery } from '@tanstack/react-query'
import { Link, Outlet, useRouterState } from '@tanstack/react-router'
import { Blocks, Ellipsis, LayoutGrid, LogOut, ScrollText, Settings, Waypoints, type LucideIcon } from 'lucide-react'
import { DropdownMenu, Popover } from 'radix-ui'

import { sessionQuery, useSignOut } from '../session'
import { DonationHeart, HeartMessage } from './DonationHeart'
import { Button, Mark } from './ui'

/**
 * The five destinations of DESIGN.md. One without `to` is not built yet: it is
 * shown, so the shape of the panel is there from the start, but it leads nowhere.
 */
const DESTINATIONS: { label: string; icon: LucideIcon; to?: '/' | '/templates' }[] = [
  { label: 'Servers', icon: LayoutGrid, to: '/' },
  { label: 'Templates', icon: Blocks, to: '/templates' },
  { label: 'Network', icon: Waypoints },
  { label: 'Activity', icon: ScrollText },
  { label: 'Settings', icon: Settings },
]

const ROW = 'flex h-8 w-full items-center gap-3 rounded-md px-2.5 text-body font-medium'
const ROW_LIVE = `${ROW} text-ink-muted transition-colors duration-120 ease-out hover:bg-surface-2 hover:text-ink`
const ROW_HERE = `${ROW} bg-accent-soft text-ink`

/**
 * Says whether a destination is where the open page belongs. Servers has the
 * front page and every page of a server; the others have what is under them.
 */
function useWithin(): (to: string) => boolean {
  const here = useRouterState({ select: (state) => state.location.pathname })
  return (to) => (to === '/' ? here === '/' || here.startsWith('/servers') : here.startsWith(to))
}
const FLOATING = 'z-50 rounded-lg border border-hairline-strong bg-surface-3 shadow-float'

/**
 * The frame every signed-in page sits in: a sidebar that is 232px wide from
 * 1200px up and 56px of icons below that, and under 768px a floating bottom bar.
 */
export function AppShell() {
  const { data: session } = useSuspenseQuery(sessionQuery)
  const username = session.user?.username ?? ''
  const within = useWithin()

  return (
    <div className="flex min-h-dvh">
      <aside className="sticky top-0 hidden h-dvh w-14 shrink-0 flex-col border-r border-hairline p-2 md:flex wide:w-58">
        <div className="flex h-8 items-center gap-3 px-2.5 text-ink">
          <Mark className="size-5 shrink-0" />
          <span className="sr-only text-section wide:not-sr-only">Homewarp</span>
        </div>
        <nav aria-label="Main" className="mt-4 flex flex-col gap-1">
          {DESTINATIONS.map(({ label, icon: Icon, to }) =>
            to ? (
              <Link key={label} to={to} aria-current={within(to) ? 'page' : undefined} className={within(to) ? ROW_HERE : ROW_LIVE}>
                <Icon aria-hidden className="w-5 shrink-0" size={16} />
                <span className="sr-only wide:not-sr-only">{label}</span>
              </Link>
            ) : (
              <span key={label} aria-disabled="true" title={`${label} is not built yet`} className={`${ROW} cursor-not-allowed text-ink-faint`}>
                <Icon aria-hidden className="w-5 shrink-0" size={16} />
                <span className="sr-only wide:not-sr-only">{label}</span>
              </span>
            ),
          )}
        </nav>
        <div className="mt-auto flex flex-col gap-1">
          <DonationHeart />
          <AccountMenu username={username} />
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col pb-20 md:pb-0">
        <Outlet />
      </div>

      <BottomBar username={username} />
    </div>
  )
}

function AccountMenu({ username }: { username: string }) {
  const signOut = useSignOut()
  return (
    // Not modal: a modal menu locks scrolling with a style element, which the content policy refuses.
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger className={ROW_LIVE}>
        <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-surface-3 text-caption text-ink uppercase">
          {username.slice(0, 1)}
        </span>
        <span className="sr-only truncate wide:not-sr-only">{username}</span>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content side="top" align="start" sideOffset={8} className={`${FLOATING} min-w-40 p-1`}>
          <DropdownMenu.Item
            onSelect={() => signOut.mutate()}
            className="flex h-8 items-center gap-2 rounded-sm px-2 text-body text-ink outline-none data-highlighted:bg-surface-2"
          >
            <LogOut aria-hidden size={16} />
            Sign out
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}

/** The phone's navigation: the first three destinations, and "More" for the rest of the sidebar. */
function BottomBar({ username }: { username: string }) {
  const signOut = useSignOut()
  const within = useWithin()
  const tab ='flex h-11 min-w-16 flex-col items-center justify-center gap-0.5 rounded-md px-2 text-caption'

  return (
    <nav aria-label="Main" className={`${FLOATING} fixed inset-x-3 bottom-3 flex h-14 items-center justify-around md:hidden`}>
      {DESTINATIONS.slice(0, 3).map(({ label, icon: Icon, to }) =>
        to ? (
          <Link key={label} to={to} aria-current={within(to) ? 'page' : undefined} className={`${tab} ${within(to) ? 'text-ink' : 'text-ink-muted'}`}>
            <Icon aria-hidden size={18} />
            {label}
          </Link>
        ) : (
          <span key={label} aria-disabled="true" className={`${tab} text-ink-faint`}>
            <Icon aria-hidden size={18} />
            {label}
          </span>
        ),
      )}
      <Popover.Root>
        <Popover.Trigger className={`${tab} text-ink-muted`}>
          <Ellipsis aria-hidden size={18} />
          More
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content side="top" align="end" sideOffset={12} className={`${FLOATING} w-72 p-4`}>
            <HeartMessage />
            <div className="mt-4 flex items-center justify-between gap-3 border-t border-hairline pt-3">
              <span className="truncate text-ink">{username}</span>
              <Button busy={signOut.isPending} onClick={() => signOut.mutate()}>
                Sign out
              </Button>
            </div>
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
    </nav>
  )
}
