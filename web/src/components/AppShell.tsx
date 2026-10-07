import { useSuspenseQuery } from '@tanstack/react-query'
import { Link, Outlet, useRouterState } from '@tanstack/react-router'
import { Blocks, Ellipsis, LayoutGrid, LogOut, ScrollText, Settings, Waypoints, type LucideIcon } from 'lucide-react'
import { DropdownMenu, Popover } from 'radix-ui'

import { gateLook, useNetwork } from '../gate'
import { sessionQuery, useSignOut } from '../session'
import { DonationHeart, HeartMessage } from './DonationHeart'
import { Button, FLOATING, MENU_ITEM, Mark, StateMark } from './ui'

type Destination = {
  label: string
  icon: LucideIcon
  to: '/' | '/templates' | '/network' | '/activity' | '/settings'
  /** For the owner alone: what is about the machine and not about one server. */
  owners?: true
}

/** The five destinations of DESIGN.md. An account that is not the owner's has two of them. */
const DESTINATIONS: Destination[] = [
  { label: 'Servers', icon: LayoutGrid, to: '/' },
  { label: 'Templates', icon: Blocks, to: '/templates', owners: true },
  { label: 'Network', icon: Waypoints, to: '/network', owners: true },
  { label: 'Activity', icon: ScrollText, to: '/activity', owners: true },
  { label: 'Settings', icon: Settings, to: '/settings' },
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

/**
 * The frame every signed-in page sits in: a sidebar that is 232px wide from
 * 1200px up and 56px of icons below that, and under 768px a floating bottom bar.
 */
export function AppShell() {
  const { data: session } = useSuspenseQuery(sessionQuery)
  const username = session.user?.username ?? ''
  const owner = session.user?.owner ?? false
  const destinations = DESTINATIONS.filter((destination) => owner || !destination.owners)
  const within = useWithin()

  return (
    <div className="flex min-h-dvh">
      <aside className="sticky top-0 hidden h-dvh w-14 shrink-0 flex-col border-r border-hairline p-2 md:flex wide:w-58">
        <div className="flex h-8 items-center gap-3 px-2.5 text-ink">
          <Mark className="size-5 shrink-0" />
          <span className="sr-only text-section wide:not-sr-only">Homewarp</span>
        </div>
        <nav aria-label="Main" className="mt-4 flex flex-col gap-1">
          {destinations.map(({ label, icon: Icon, to }) => (
            <Link key={label} to={to} aria-current={within(to) ? 'page' : undefined} className={within(to) ? ROW_HERE : ROW_LIVE}>
              <Icon aria-hidden className="w-5 shrink-0" size={16} />
              <span className="sr-only wide:not-sr-only">{label}</span>
            </Link>
          ))}
        </nav>
        <div className="mt-auto flex flex-col gap-1">
          {/* It leads to the Network page, which is the owner's. */}
          {owner && <GateMark />}
          <DonationHeart />
          <AccountMenu username={username} />
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col pb-20 md:pb-0">
        <Outlet />
      </div>

      <BottomBar username={username} destinations={destinations} />
    </div>
  )
}

/**
 * How the tunnel is doing, on every page: it is the one thing that affects
 * every server (DESIGN.md, App shell). It leads to the Network page. Nothing
 * waits for it: it is asked for beside the page and painted when it answers.
 */
function GateMark() {
  const { word, tone } = gateLook(useNetwork())
  return (
    <Link to="/network" title={word} className={ROW_LIVE}>
      <span className="flex w-5 shrink-0 justify-center">
        <StateMark tone={tone} />
      </span>
      <span className="sr-only truncate wide:not-sr-only">{word}</span>
    </Link>
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
          <DropdownMenu.Item onSelect={() => signOut.mutate()} className={MENU_ITEM}>
            <LogOut aria-hidden size={16} />
            Sign out
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}

/** The phone's navigation: the first three destinations, and "More" for the others and the rest of the sidebar. */
function BottomBar({ username, destinations }: { username: string; destinations: Destination[] }) {
  const signOut = useSignOut()
  const within = useWithin()
  const tab ='flex h-11 min-w-16 flex-col items-center justify-center gap-0.5 rounded-md px-2 text-caption'

  return (
    <nav aria-label="Main" className={`${FLOATING} fixed inset-x-3 bottom-3 flex h-14 items-center justify-around md:hidden`}>
      {destinations.slice(0, 3).map(({ label, icon: Icon, to }) => (
        <Link key={label} to={to} aria-current={within(to) ? 'page' : undefined} className={`${tab} ${within(to) ? 'text-ink' : 'text-ink-muted'}`}>
          <Icon aria-hidden size={18} />
          {label}
        </Link>
      ))}
      <Popover.Root>
        <Popover.Trigger className={`${tab} text-ink-muted`}>
          <Ellipsis aria-hidden size={18} />
          More
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content side="top" align="end" sideOffset={12} className={`${FLOATING} w-72 p-4`}>
            {destinations.length > 3 && (
              <div className="mb-4 flex flex-col gap-1 border-b border-hairline pb-3">
                {destinations.slice(3).map(({ label, icon: Icon, to }) => (
                  <Popover.Close asChild key={label}>
                    <Link to={to} className={ROW_LIVE}>
                      <Icon aria-hidden className="w-5 shrink-0" size={16} />
                      {label}
                    </Link>
                  </Popover.Close>
                ))}
              </div>
            )}
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
