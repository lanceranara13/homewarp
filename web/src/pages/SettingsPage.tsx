import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { KeyRound, Plus, Trash2 } from 'lucide-react'
import { useState, type ReactNode } from 'react'

import { accountsQuery, settingsQuery, twoStepsQuery } from '../accounts'
import {
  beginTwoSteps,
  changeOwnPassword,
  changeSettings,
  confirmTwoSteps,
  createAccount,
  endTwoSteps,
  removeAccount,
  setPassword,
  type Account,
} from '../api/client'
import { Button, Confirm, CopyChip, Field, PageBar, Problem } from '../components/ui'
import { when } from '../format'
import { sessionQuery } from '../session'

/** What is about this Homewarp and not about one server: the account signed in here and, for the owner, the others. */
export function SettingsPage() {
  const { data: session } = useSuspenseQuery(sessionQuery)

  return (
    <>
      <PageBar title="Settings" />
      <main className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-10 p-4 md:p-6">
        <OwnPassword username={session.user?.username ?? ''} />
        <TwoSteps />
        {session.user?.owner && <Accounts />}
        {session.user?.owner && <Resolvers />}
      </main>
    </>
  )
}

function Section({ title, lead, children }: { title: string; lead: ReactNode; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-4">
      <div>
        <h2 className="text-section text-ink">{title}</h2>
        <p className="mt-1 max-w-140 text-small text-ink-subtle">{lead}</p>
      </div>
      {children}
    </section>
  )
}

/** Whoever is signed in changes their own password, knowing the one they have. */
function OwnPassword({ username }: { username: string }) {
  const changing = useMutation({
    mutationFn: ({ current, password }: { current: string; password: string }) => changeOwnPassword(current, password),
  })

  return (
    <Section
      title="Your account"
      lead={
        <>
          You are signed in as <span className="font-medium text-ink">{username}</span>. Changing the password signs this
          account out everywhere but here.
        </>
      }
    >
      <form
        className="flex max-w-80 flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          const form = event.currentTarget
          const typed = (name: string) => String(new FormData(form).get(name) ?? '')
          changing.mutate({ current: typed('current'), password: typed('password') }, { onSuccess: () => form.reset() })
        }}
      >
        {/* For the password manager, which files a new password under the name beside it. */}
        <input type="text" name="username" value={username} readOnly hidden autoComplete="username" />
        <Field label="Password now" name="current" type="password" required autoComplete="current-password" />
        <Field
          label="New password"
          name="password"
          type="password"
          required
          minLength={10}
          autoComplete="new-password"
          hint="10 characters or more."
        />
        {changing.error && <Problem>{changing.error.message}</Problem>}
        {changing.isSuccess && (
          <p role="status" className="text-small text-ink-muted">
            Your password is changed.
          </p>
        )}
        <div>
          <Button type="submit" busy={changing.isPending}>
            Change password
          </Button>
        </div>
      </form>
    </Section>
  )
}

/**
 * A second step for this account's sign-in: a code from an authenticator app.
 * It is on only once the app has shown, with a code, that it has the secret.
 */
function TwoSteps() {
  const queryClient = useQueryClient()
  const { data: now } = useSuspenseQuery(twoStepsQuery)
  const again = () => queryClient.invalidateQueries({ queryKey: twoStepsQuery.queryKey })
  const beginning = useMutation({ mutationFn: beginTwoSteps })
  const confirming = useMutation({ mutationFn: confirmTwoSteps, onSuccess: again })
  const ending = useMutation({
    mutationFn: endTwoSteps,
    onSuccess: async () => {
      beginning.reset()
      confirming.reset()
      await again()
    },
  })
  const typed = (form: HTMLFormElement, name: string) => String(new FormData(form).get(name) ?? '')

  return (
    <Section
      title="Two-step sign-in"
      lead="With it on, signing in takes your password and then a code from an authenticator app on your phone. Over SFTP the code is typed straight after the password, as one word."
    >
      {confirming.data ? (
        <div className="flex max-w-140 flex-col gap-3">
          <p role="status" className="text-ink">
            It is on. Keep these recovery codes somewhere safe: each signs you in once if the app is lost, and they are
            not shown again.
          </p>
          <ul className="grid grid-cols-2 gap-x-6 gap-y-1 font-mono text-mono text-ink">
            {confirming.data.map((code) => (
              <li key={code}>{code}</li>
            ))}
          </ul>
        </div>
      ) : now.on ? (
        <form
          className="flex max-w-80 flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            ending.mutate(typed(event.currentTarget, 'password'))
          }}
        >
          <p className="text-ink">
            It is on. {now.recovery_codes === 1 ? '1 recovery code is' : `${now.recovery_codes} recovery codes are`} left.
          </p>
          <Field label="Your password, to turn it off" name="password" type="password" required autoComplete="current-password" />
          {ending.error && <Problem>{ending.error.message}</Problem>}
          <div>
            <Button type="submit" busy={ending.isPending}>
              Turn off
            </Button>
          </div>
        </form>
      ) : beginning.data ? (
        <form
          className="flex max-w-140 flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            confirming.mutate(typed(event.currentTarget, 'code'))
          }}
        >
          <p>
            Add an account in the app with this secret, or{' '}
            <a href={beginning.data.uri} className="text-accent hover:underline">
              open it in an app on this device
            </a>
            . Then type the code the app shows.
          </p>
          <div>
            <CopyChip text={beginning.data.secret} />
          </div>
          <div className="max-w-80">
            <Field label="Code from the app" name="code" required mono inputMode="numeric" autoComplete="one-time-code" />
          </div>
          {confirming.error && <Problem>{confirming.error.message}</Problem>}
          <div>
            <Button type="submit" variant="primary" busy={confirming.isPending}>
              Turn on
            </Button>
          </div>
        </form>
      ) : (
        <div className="flex flex-col items-start gap-3">
          {beginning.error && <Problem>{beginning.error.message}</Problem>}
          <Button busy={beginning.isPending} onClick={() => beginning.mutate()}>
            Set up an authenticator app
          </Button>
        </div>
      )}
    </Section>
  )
}

/** Where servers look names up: what a server asks when it fetches a plugin, or checks a player's account. */
function Resolvers() {
  const queryClient = useQueryClient()
  const { data: settings } = useSuspenseQuery(settingsQuery)
  const changing = useMutation({
    mutationFn: changeSettings,
    onSuccess: (kept) => queryClient.setQueryData(settingsQuery.queryKey, kept),
  })

  return (
    <Section
      title="Name lookups"
      lead="Servers are kept from the home network, the router included, so they ask resolvers on the internet for the addresses behind names. A change counts from each server's next start."
    >
      <form
        // What was kept is what the field starts from again.
        key={settings.resolvers.join()}
        className="flex max-w-80 flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          const typed = String(new FormData(event.currentTarget).get('resolvers') ?? '')
          changing.mutate({ resolvers: typed.split(/[\s,]+/).filter(Boolean) })
        }}
      >
        <Field
          label="Resolvers"
          name="resolvers"
          mono
          required
          defaultValue={settings.resolvers.join(', ')}
          autoComplete="off"
          spellCheck={false}
          hint="One to three IPv4 addresses, with commas between."
        />
        {changing.error && <Problem>{changing.error.message}</Problem>}
        {changing.isSuccess && (
          <p role="status" className="text-small text-ink-muted">
            Kept. Servers ask there from their next start.
          </p>
        )}
        <div>
          <Button type="submit" busy={changing.isPending}>
            Save
          </Button>
        </div>
      </form>
    </Section>
  )
}

/** The owner makes the other accounts here. What each may do is said server by server, on a server's Users tab. */
function Accounts() {
  const queryClient = useQueryClient()
  const { data: accounts } = useSuspenseQuery(accountsQuery)
  const [adding, setAdding] = useState(false)
  const [resetting, setResetting] = useState<Account | null>(null)
  const [doomed, setDoomed] = useState<Account | null>(null)
  const again = () => queryClient.invalidateQueries({ queryKey: accountsQuery.queryKey })

  const creating = useMutation({
    mutationFn: createAccount,
    onSuccess: async () => {
      await again()
      setAdding(false)
    },
  })
  const passing = useMutation({
    mutationFn: ({ id, password }: { id: number; password: string }) => setPassword(id, password),
    onSuccess: () => setResetting(null),
  })
  const removing = useMutation({
    mutationFn: removeAccount,
    onSuccess: async () => {
      await again()
      setDoomed(null)
    },
  })
  const typed = (form: HTMLFormElement, name: string) => String(new FormData(form).get(name) ?? '')

  return (
    <Section
      title="Accounts"
      lead="Another account sees no server until it is let into one. That is done on a server's Users tab, where it is also said what the account may do there."
    >
      <ul className="overflow-hidden rounded-lg border border-hairline bg-surface-1">
        {accounts.map((account) => (
          <li key={account.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-hairline px-4 py-2.5 last:border-b-0">
            <span className="min-w-0 flex-1 truncate font-medium text-ink">{account.username}</span>
            <span className="text-small text-ink-subtle">
              {account.owner
                ? 'Owner'
                : account.servers === 0
                  ? 'In no server yet'
                  : account.servers === 1
                    ? 'In 1 server'
                    : `In ${account.servers} servers`}
              {` · made ${when(account.created_at)}`}
            </span>
            {!account.owner && (
              <span className="flex gap-1">
                <Button variant="ghost" onClick={() => setResetting(account)}>
                  <KeyRound aria-hidden size={16} />
                  <span className="sr-only md:not-sr-only">New password</span>
                </Button>
                <Button variant="ghost" onClick={() => setDoomed(account)}>
                  <Trash2 aria-hidden size={16} />
                  <span className="sr-only md:not-sr-only">Remove</span>
                </Button>
              </span>
            )}
          </li>
        ))}
      </ul>

      {resetting && (
        <form
          key={resetting.id}
          className="flex max-w-80 flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            passing.mutate({ id: resetting.id, password: typed(event.currentTarget, 'password') })
          }}
        >
          <Field
            label={`New password for ${resetting.username}`}
            name="password"
            type="password"
            required
            minLength={10}
            autoFocus
            autoComplete="new-password"
            hint="It is signed out wherever it is signed in, and signs in again with this."
          />
          {passing.error && <Problem>{passing.error.message}</Problem>}
          <div className="flex gap-2">
            <Button type="submit" busy={passing.isPending}>
              Set password
            </Button>
            <Button
              variant="ghost"
              onClick={() => {
                setResetting(null)
                passing.reset()
              }}
            >
              Cancel
            </Button>
          </div>
        </form>
      )}

      {adding ? (
        <form
          className="flex max-w-80 flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            creating.mutate({ username: typed(event.currentTarget, 'username'), password: typed(event.currentTarget, 'password') })
          }}
        >
          <Field label="Username" name="username" required autoFocus autoComplete="off" spellCheck={false} />
          <Field
            label="Password"
            name="password"
            type="password"
            required
            minLength={10}
            autoComplete="new-password"
            hint="10 characters or more. Whoever it is for can change it once signed in."
          />
          {creating.error && <Problem>{creating.error.message}</Problem>}
          <div className="flex gap-2">
            <Button type="submit" variant="primary" busy={creating.isPending}>
              Create account
            </Button>
            <Button
              variant="ghost"
              onClick={() => {
                setAdding(false)
                creating.reset()
              }}
            >
              Cancel
            </Button>
          </div>
        </form>
      ) : (
        <div>
          <Button onClick={() => setAdding(true)}>
            <Plus aria-hidden size={16} />
            New account
          </Button>
        </div>
      )}

      <Confirm
        open={doomed !== null}
        onClose={() => {
          setDoomed(null)
          removing.reset()
        }}
        title={`Remove ${doomed?.username ?? 'this account'}?`}
        action={
          <Button variant="danger" busy={removing.isPending} onClick={() => doomed && removing.mutate(doomed.id)}>
            Remove
          </Button>
        }
      >
        <p>It is signed out everywhere and taken out of every server. What it did stays written down under Activity.</p>
        {removing.error && (
          <div className="mt-3">
            <Problem>{removing.error.message}</Problem>
          </div>
        )}
      </Confirm>
    </Section>
  )
}
