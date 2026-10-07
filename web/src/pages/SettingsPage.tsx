import { useMutation, useQuery, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import { Link, getRouteApi } from '@tanstack/react-router'
import { KeyRound, Plus, Trash2 } from 'lucide-react'
import { useState, type ReactNode } from 'react'

import { SETTINGS_GROUPS, accountsQuery, passkeysQuery, settingsQuery, twoStepsQuery } from '../accounts'
import {
  addPasskey,
  beginPasskey,
  beginTwoSteps,
  changeOwnPassword,
  changeSettings,
  confirmTwoSteps,
  createAccount,
  endTwoSteps,
  removeAccount,
  removeNotices,
  removePasskey,
  removeStore,
  setNotices,
  setPassword,
  setStore,
  testNotices,
  type Account,
} from '../api/client'
import { Button, Confirm, CopyChip, Field, PageBar, Problem } from '../components/ui'
import { when } from '../format'
import { makePasskey, passkeysHere } from '../passkeys'
import { sessionQuery } from '../session'
import { Updates } from './UpdatesSettings'

const route = getRouteApi('/shell/settings')

/**
 * What is about this Homewarp and not about one server: the account signed in
 * here and, for the owner, the others and the machine. One group of it is open
 * at a time, and the address says which.
 */
export function SettingsPage() {
  const { data: session } = useSuspenseQuery(sessionQuery)
  const { group = 'account' } = route.useSearch()

  return (
    <>
      <PageBar title="Settings" />
      <div className="mx-auto flex w-full max-w-300 flex-1 flex-col gap-4 p-4 md:flex-row md:gap-8 md:p-6">
        <Groups owner={session.user?.owner ?? false} />
        <main className="flex min-w-0 flex-1 flex-col gap-10">
          {group === 'account' && <OwnPassword username={session.user?.username ?? ''} />}
          {group === 'security' && (
            <>
              <TwoSteps />
              <Passkeys />
            </>
          )}
          {group === 'users' && <Accounts />}
          {group === 'backups' && <BackupStore />}
          {group === 'system' && (
            <>
              <Resolvers />
              <NewConnections />
              <Notices />
            </>
          )}
          {group === 'updates' && <Updates />}
        </main>
      </div>
    </>
  )
}

const GROUP =
  'flex h-10 shrink-0 items-center border-b-2 px-3 text-body font-medium transition-colors duration-120 ease-out md:h-8 md:rounded-md md:border-b-0 md:px-2.5'
const GROUP_HERE = { className: 'border-accent text-ink md:bg-accent-soft' }
const GROUP_ELSEWHERE = {
  className: 'border-transparent text-ink-subtle hover:text-ink md:text-ink-muted md:hover:bg-surface-2',
}

/**
 * The groups, down the left of the page as rows like the sidebar's. A phone has
 * no room beside the page, so there they are underline tabs that scroll sideways
 * (DESIGN.md, Tabs). An account that is not the owner's has its own two.
 */
function Groups({ owner }: { owner: boolean }) {
  return (
    <div className="border-b border-hairline md:w-48 md:shrink-0 md:border-b-0">
      <nav aria-label="Settings" className="-mb-px flex overflow-x-auto md:sticky md:top-6 md:mb-0 md:flex-col md:gap-1 md:overflow-visible">
        {SETTINGS_GROUPS.filter((group) => owner || !group.owners).map(({ name, label }) => (
          <Link
            key={name}
            to="/settings"
            search={name === 'account' ? {} : { group: name }}
            // Exact, or the first group, which asks for nothing in the address, would be open beside every other.
            activeOptions={{ exact: true }}
            className={GROUP}
            activeProps={GROUP_HERE}
            inactiveProps={GROUP_ELSEWHERE}
          >
            {label}
          </Link>
        ))}
      </nav>
    </div>
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

/**
 * The passkeys of the account signed in here. Nothing on the page waits for
 * them: the section comes when its answer does.
 */
function Passkeys() {
  const queryClient = useQueryClient()
  const { data: kept } = useQuery(passkeysQuery)
  const again = () => queryClient.invalidateQueries({ queryKey: passkeysQuery.queryKey })
  const adding = useMutation({
    mutationFn: async ({ name, password }: { name: string; password: string }) => {
      const options = await beginPasskey(password)
      return addPasskey({ name, ...(await makePasskey(options)) })
    },
    onSuccess: again,
  })
  const removing = useMutation({ mutationFn: removePasskey, onSuccess: again })

  if (!kept) return null
  const here = kept.available && passkeysHere()

  return (
    <Section
      title="Passkeys"
      lead="A passkey lives on a device of yours: a phone, a laptop, a security key. With one, signing in is the device asking for your fingerprint, face or PIN, and nothing is typed. It works on this panel’s own address and on no page that only looks like it."
    >
      {kept.passkeys.length > 0 && (
        <ul className="flex max-w-140 flex-col rounded-lg border border-hairline bg-surface-1">
          {kept.passkeys.map((passkey) => (
            <li key={passkey.id} className="flex items-center gap-3 border-b border-hairline px-4 py-2 last:border-b-0">
              <KeyRound aria-hidden className="size-4 shrink-0 text-ink-subtle" />
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium text-ink">{passkey.name}</div>
                <div className="text-small text-ink-subtle">
                  Made {when(passkey.created_at)}
                  {passkey.last_used_at ? ` · last used ${when(passkey.last_used_at)}` : ' · not used yet'}
                </div>
              </div>
              <Button
                variant="ghost"
                aria-label={`Remove ${passkey.name}`}
                busy={removing.isPending && removing.variables === passkey.id}
                onClick={() => removing.mutate(passkey.id)}
              >
                <Trash2 aria-hidden size={16} />
              </Button>
            </li>
          ))}
        </ul>
      )}
      {removing.error && <Problem>{removing.error.message}</Problem>}
      {here ? (
        <form
          className="flex max-w-80 flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            const form = event.currentTarget
            const typed = (name: string) => String(new FormData(form).get(name) ?? '')
            adding.mutate({ name: typed('name'), password: typed('password') }, { onSuccess: () => form.reset() })
          }}
        >
          <Field label="What to call it" name="name" required maxLength={40} autoComplete="off" placeholder="Laptop" />
          <Field
            label="Your password"
            name="password"
            type="password"
            required
            autoComplete="current-password"
            hint="A passkey is another way in, so adding one takes the password."
          />
          {adding.error && <Problem>{adding.error.message}</Problem>}
          <div>
            <Button type="submit" busy={adding.isPending}>
              <Plus aria-hidden size={16} />
              Add a passkey
            </Button>
          </div>
        </form>
      ) : (
        <p className="max-w-140 text-small text-ink-subtle">
          A browser makes a passkey only on a page it trusts. That is this panel reached by its name, which Network sets up,
          and not by an address on the home network, as it is now.
        </p>
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

/** How many new connections a second a connected VPS lets one address open to the servers. */
function NewConnections() {
  const queryClient = useQueryClient()
  const { data: settings } = useSuspenseQuery(settingsQuery)
  const changing = useMutation({
    mutationFn: changeSettings,
    onSuccess: (kept) => queryClient.setQueryData(settingsQuery.queryKey, kept),
  })

  return (
    <Section
      title="New connections"
      lead="A connected VPS lets each address on the internet open this many new connections a second to your servers, and twice as many at once. More are dropped at the VPS, so that one address cannot crowd the others out. Players who are already connected are not counted."
    >
      <form
        // What was kept is what the field starts from again.
        key={settings.new_connections}
        className="flex max-w-80 flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          changing.mutate({ new_connections: Number(new FormData(event.currentTarget).get('new_connections')) })
        }}
      >
        <Field
          label="A second, from one address"
          name="new_connections"
          type="number"
          mono
          required
          min={1}
          max={10000}
          defaultValue={settings.new_connections}
          hint="30 unless you say otherwise."
        />
        {changing.error && <Problem>{changing.error.message}</Problem>}
        {changing.isSuccess && (
          <p role="status" className="text-small text-ink-muted">
            Kept. A connected VPS is told at once.
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

/**
 * Where Homewarp tells what happens to it while nobody is at the panel: an
 * address that takes a message, as a Discord or a Slack webhook does. The
 * address is a secret of the site's making, so what is kept is not shown whole.
 */
function Notices() {
  const queryClient = useQueryClient()
  const { data: settings } = useSuspenseQuery(settingsQuery)
  const kept = (fresh: typeof settings) => queryClient.setQueryData(settingsQuery.queryKey, fresh)
  const setting = useMutation({ mutationFn: setNotices, onSuccess: kept })
  const removing = useMutation({ mutationFn: removeNotices, onSuccess: kept })
  const testing = useMutation({
    mutationFn: testNotices,
    onSuccess: kept,
    // How it went is kept either way, and shown below.
    onError: () => queryClient.invalidateQueries({ queryKey: settingsQuery.queryKey }),
  })
  const notices = settings.notices

  return (
    <Section
      title="Notices"
      lead="Homewarp can tell you what happens while you are not here: a server that crashed, one that was put to sleep or woken by a player, what a schedule did, a VPS that stopped answering. Give it the address of a webhook, as Discord, Slack and their like make them for a channel. Whoever has that address can post there, so it is kept and not shown again."
    >
      {notices && (
        <div className="flex max-w-140 flex-col gap-2">
          <p>
            Told to <code className="font-mono text-mono text-ink">{notices.address}</code>
            {notices.everything ? ': everything on the Activity page.' : ': what happens by itself.'}
          </p>
          {notices.last && (
            <p role="status" className="text-small text-ink-subtle">
              {notices.last.problem
                ? `The last one, ${when(notices.last.at)}, was not taken: ${notices.last.problem}`
                : `The last one was taken ${when(notices.last.at)}.`}
            </p>
          )}
          {testing.error && <Problem>{testing.error.message}</Problem>}
          {removing.error && <Problem>{removing.error.message}</Problem>}
          <div className="flex flex-wrap gap-2">
            <Button busy={testing.isPending} onClick={() => testing.mutate()}>
              Send one now
            </Button>
            <Button variant="ghost" busy={removing.isPending} onClick={() => removing.mutate()}>
              Stop telling it
            </Button>
          </div>
        </div>
      )}
      <form
        // Emptied once what was typed has been kept: the address is not shown again.
        key={notices?.address ?? ''}
        className="flex max-w-140 flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          const form = new FormData(event.currentTarget)
          setting.mutate({ url: String(form.get('url') ?? ''), everything: form.get('everything') === 'on' })
        }}
      >
        <Field
          label={notices ? 'Another address' : 'Address to tell'}
          name="url"
          mono
          required
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          placeholder="https://discord.com/api/webhooks/…"
          hint="It has to begin with https:// and be on the internet: Homewarp sends nothing into a home network."
        />
        <label className="flex items-start gap-2">
          <input type="checkbox" name="everything" defaultChecked={notices?.everything ?? false} className="mt-0.5 size-4 accent-accent" />
          <span>
            Everything on the Activity page
            <span className="block text-small text-ink-subtle">And not only what happens by itself: every sign-in, start, stop and change, as it is done.</span>
          </span>
        </label>
        {setting.error && <Problem>{setting.error.message}</Problem>}
        <div>
          <Button type="submit" busy={setting.isPending}>
            {notices ? 'Tell this one instead' : 'Save'}
          </Button>
        </div>
      </form>
    </Section>
  )
}

/**
 * A store elsewhere for backups: a bucket that is spoken to as Amazon's S3 is.
 * A backup beside the server it is of is lost with the disk they are both on.
 */
function BackupStore() {
  const queryClient = useQueryClient()
  const { data: settings } = useSuspenseQuery(settingsQuery)
  const kept = (fresh: typeof settings) => queryClient.setQueryData(settingsQuery.queryKey, fresh)
  const setting = useMutation({ mutationFn: setStore, onSuccess: kept })
  const removing = useMutation({ mutationFn: removeStore, onSuccess: kept })
  const store = settings.store

  return (
    <Section
      title="A store elsewhere for backups"
      lead="A backup kept beside its server is lost with the disk they are both on. Give Homewarp a bucket somewhere else and every backup that is made is copied there too: Amazon S3, or anything that is spoken to the same way, such as Cloudflare R2, Backblaze B2 or a MinIO of your own. A copy goes when its backup goes. Each is the backup's own file, a tar packed with Zstandard, and can be fetched with any S3 tool."
    >
      {store && (
        <div className="flex max-w-140 flex-col gap-2">
          <p>
            Copied to the bucket <code className="font-mono text-mono text-ink">{store.bucket}</code> at{' '}
            <code className="font-mono text-mono text-ink wrap-anywhere">{store.endpoint}</code>
            {store.prefix && (
              <>
                , under <code className="font-mono text-mono text-ink">{store.prefix}/</code>
              </>
            )}
            , with the key <code className="font-mono text-mono text-ink">{store.key_id}</code>.
          </p>
          {removing.error && <Problem>{removing.error.message}</Problem>}
          <div>
            <Button variant="ghost" busy={removing.isPending} onClick={() => removing.mutate()}>
              Stop copying
            </Button>
          </div>
        </div>
      )}
      <form
        // Emptied once what was typed has been kept: the secret is not shown again.
        key={store ? `${store.endpoint}/${store.bucket}/${store.key_id}` : ''}
        className="flex max-w-140 flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          const form = new FormData(event.currentTarget)
          const typed = (name: string) => String(form.get(name) ?? '')
          setting.mutate({
            endpoint: typed('endpoint'),
            region: typed('region'),
            bucket: typed('bucket'),
            prefix: typed('prefix'),
            key_id: typed('key_id'),
            secret: typed('secret'),
          })
        }}
      >
        <Field
          label={store ? 'Another store: its address' : 'Address'}
          name="endpoint"
          mono
          required
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          placeholder="https://s3.eu-central-1.amazonaws.com"
          hint="Where the store is, without the bucket. One at home may be http://192.168.1.20:9000."
        />
        <Field label="Bucket" name="bucket" mono required autoComplete="off" spellCheck={false} placeholder="my-backups" />
        <Field
          label="Folder in the bucket"
          name="prefix"
          mono
          autoComplete="off"
          spellCheck={false}
          placeholder="homewarp"
          hint="Left empty, backups go at the top of the bucket."
        />
        <Field
          label="Region"
          name="region"
          mono
          autoComplete="off"
          spellCheck={false}
          placeholder="us-east-1"
          hint="Left empty it is us-east-1, which most stores other than Amazon's take whatever they are."
        />
        <Field label="Key id" name="key_id" mono required autoComplete="off" spellCheck={false} />
        <Field
          label="Key secret"
          name="secret"
          type="password"
          mono
          required
          autoComplete="off"
          hint="A key that may write to this one bucket and to nothing else is the one to give. It is kept and not shown again."
        />
        {setting.error && <Problem>{setting.error.message}</Problem>}
        <div>
          <Button type="submit" busy={setting.isPending}>
            Try it, and keep it
          </Button>
        </div>
        <p className="text-small text-ink-subtle">
          Homewarp writes a few bytes to the bucket and takes them away again. Only a store that took them is kept.
        </p>
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
