import { Popover } from 'radix-ui'

/**
 * Where a heart can be given: the same places README.md names under Support.
 * Were there none, the popover would say so rather than show dead buttons.
 */
const LINKS: { label: string; href: string }[] = [{ label: 'Give on Ko-fi', href: 'https://ko-fi.com/pixelsthecoder' }]

/** The pixel heart of DESIGN.md: a 7 × 6 grid, drawn at whole multiples so the pixels stay sharp. */
export function PixelHeart({ className = '' }: { className?: string }) {
  return (
    <svg viewBox="0 0 7 6" width="14" height="12" shapeRendering="crispEdges" aria-hidden className={className}>
      <path className="fill-heart" d="M1 0h2v1h1V0h2v1h1v2h-1v1h-1v1h-1v1H3V5H2V4H1V3H0V1h1z" />
      <rect x="1" y="1" width="1" height="1" fill="#fff" opacity=".65" />
    </svg>
  )
}

/** The whole of the ask. It appears in a popover and nowhere else: never a modal, never a banner. */
export function HeartMessage() {
  return (
    <>
      <p className="text-section text-ink">Give Homewarp a heart</p>
      <p className="mt-1 text-small text-ink-muted">
        Homewarp is free and stays free. If it saved you a hosting bill, a heart keeps it going.
      </p>
      {LINKS.length > 0 ? (
        <div className="mt-3 flex flex-wrap gap-2">
          {LINKS.map((link) => (
            <a
              key={link.href}
              href={link.href}
              target="_blank"
              rel="noreferrer"
              className="inline-flex h-8 items-center rounded-md border border-hairline-strong bg-surface-1 px-3 text-body font-medium text-ink hover:bg-surface-2"
            >
              {link.label}
            </a>
          ))}
        </div>
      ) : (
        <p className="mt-3 text-small text-ink-subtle">The donation links are not set up yet.</p>
      )}
    </>
  )
}

/** The heart at the foot of the sidebar. It beats twice when pointed at, and opens the ask. */
export function DonationHeart() {
  return (
    <Popover.Root>
      <Popover.Trigger className="group flex h-8 w-full items-center gap-3 rounded-md px-2.5 text-body font-medium text-ink-muted transition-colors duration-120 ease-out hover:bg-surface-2 hover:text-ink">
        <span className="flex w-5 shrink-0 justify-center">
          <PixelHeart className="group-hover:animate-heartbeat group-focus-visible:animate-heartbeat motion-reduce:animate-none" />
        </span>
        <span className="sr-only wide:not-sr-only">Give a heart</span>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="start"
          sideOffset={8}
          className="z-50 w-72 rounded-lg border border-hairline-strong bg-surface-3 p-4 shadow-float"
        >
          <HeartMessage />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  )
}
