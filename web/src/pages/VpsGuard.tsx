import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { hardenVps, keepGuard, unhardenVps, type VpsGuard as Guard } from '../api/client'
import { Button, Pill, Problem } from '../components/ui'
import { guardQuery } from '../gate'

/** Ports as a sentence has them: "TCP 22, 80 · UDP 51820". */
function said(ports: Guard['open']): string {
  const kinds = [
    ports.tcp.length > 0 && `TCP ${ports.tcp.join(', ')}`,
    ports.udp.length > 0 && `UDP ${ports.udp.join(', ')}`,
  ].filter(Boolean)
  return kinds.length > 0 ? kinds.join(' · ') : 'nothing'
}

/**
 * "Harden this VPS": shuts the VPS itself to the internet, but for what is
 * listening there. It is put in place on trial and undone by the VPS itself
 * unless it is kept, so that a mistake cannot lock anyone out. Nothing on the
 * page waits for this.
 */
export function VpsGuard({ id }: { id: number }) {
  const queryClient = useQueryClient()
  const { data: guard, error } = useQuery({
    ...guardQuery(id),
    // On trial every second counts, and is counted on the VPS, not here.
    refetchInterval: (query) => (query.state.data?.state === 'trial' ? 1_000 : 30_000),
  })
  const keepAnswer = (changed: Guard) => queryClient.setQueryData(guardQuery(id).queryKey, changed)
  const [tried, setTried] = useState(false)
  const hardening = useMutation({
    mutationFn: () => hardenVps(id),
    onSuccess: (changed) => {
      setTried(true)
      keepAnswer(changed)
    },
  })
  const keeping = useMutation({
    mutationFn: () => keepGuard(id),
    onSuccess: (changed) => {
      setTried(false)
      keepAnswer(changed)
    },
  })
  const undoing = useMutation({
    mutationFn: () => unhardenVps(id),
    onSuccess: (changed) => {
      setTried(false)
      keepAnswer(changed)
    },
  })
  const problem = hardening.error ?? keeping.error ?? undoing.error

  return (
    <section className="flex flex-col gap-3">
      <h3 className="font-medium text-ink">The VPS itself</h3>
      {!guard ? (
        error && <Problem>{error.message}</Problem>
      ) : guard.state === 'off' ? (
        <div className="flex max-w-140 flex-col items-start gap-3">
          <p>
            Homewarp passes on what arrives for your servers and leaves the rest of the VPS as it found it. Hardening shuts
            the VPS itself to the internet, but for what is listening there now:{' '}
            <span className="font-mono text-mono text-ink">{said(guard.listening)}</span>. New SSH connections from one
            address are held to twelve a minute. Your servers’ ports are not affected.
          </p>
          {/* A trial that ran out without being kept: said, so that nobody thinks it is still there. */}
          {tried && (
            <p role="status" className="text-small text-ink">
              It was not kept within its minute, and the VPS has undone it.
            </p>
          )}
          <Button busy={hardening.isPending} onClick={() => hardening.mutate()}>
            Harden this VPS
          </Button>
          <p className="text-small text-ink-subtle">
            It is on trial for a minute. Unless you keep it, the VPS undoes it by itself, so a mistake cannot lock you out.
          </p>
        </div>
      ) : guard.state === 'trial' ? (
        <div className="flex max-w-140 flex-col items-start gap-3">
          <Pill tone="starting">On trial · {guard.seconds_left ?? 0} s</Pill>
          <p>
            Check now that you still reach the VPS the ways you need to: open a <em>new</em> SSH session to it. Then keep it.
            If you do nothing, the VPS undoes it by itself when the time is up.
          </p>
          <p className="text-small text-ink-subtle">
            Open: <span className="font-mono text-mono text-ink">{said(guard.open)}</span>
          </p>
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" busy={keeping.isPending} onClick={() => keeping.mutate()}>
              Keep it
            </Button>
            <Button busy={undoing.isPending} onClick={() => undoing.mutate()}>
              Undo now
            </Button>
          </div>
        </div>
      ) : (
        <div className="flex max-w-140 flex-col items-start gap-3">
          <Pill tone="running">Hardened</Pill>
          <p className="text-small text-ink-subtle">
            Open on the VPS itself: <span className="font-mono text-mono text-ink">{said(guard.open)}</span>. Everything
            else that arrives there from the internet is dropped: {guard.dropped.toLocaleString()}{' '}
            {guard.dropped === 1 ? 'packet' : 'packets'} so far.
          </p>
          {(guard.shut.tcp.length > 0 || guard.shut.udp.length > 0) && (
            <p className="text-small">
              Listening on the VPS now, and shut: <span className="font-mono text-mono text-ink">{said(guard.shut)}</span>. It
              began after the VPS was hardened. Harden again to open what is listening now.
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button busy={hardening.isPending} onClick={() => hardening.mutate()}>
              Harden again
            </Button>
            <Button busy={undoing.isPending} onClick={() => undoing.mutate()}>
              Undo
            </Button>
          </div>
        </div>
      )}
      {problem && <Problem>{problem.message}</Problem>}
    </section>
  )
}
