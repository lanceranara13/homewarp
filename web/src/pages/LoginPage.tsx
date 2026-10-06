import { KeyRound } from 'lucide-react'

import { CodeNeeded, beginPasskeySignIn, passkeySignIn, signIn } from '../api/client'
import { Button, Doorway, Field, Problem } from '../components/ui'
import { passkeysHere, signWithPasskey } from '../passkeys'
import { useEnter } from '../session'

export function LoginPage() {
  const login = useEnter(signIn)
  // Nobody is named: the device says which key it has, and the key says whose it is.
  const withPasskey = useEnter(async () => passkeySignIn(await signWithPasskey(await beginPasskeySignIn())))

  return (
    <Doorway title="Sign in" lead="Your game servers are on the other side.">
      <form
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          const form = new FormData(event.currentTarget)
          const typed = (name: string) => String(form.get(name) ?? '')
          login.mutate({ username: typed('username'), password: typed('password'), code: typed('code') })
        }}
      >
        <Field label="Username" name="username" required autoFocus autoComplete="username" spellCheck={false} />
        <Field label="Password" name="password" type="password" required autoComplete="current-password" />
        {/* Asked for once the password has been found right, for an account that has a second step. */}
        {login.error instanceof CodeNeeded && (
          <Field
            label="Code"
            name="code"
            required
            autoFocus
            mono
            inputMode="numeric"
            autoComplete="one-time-code"
            hint="From your authenticator app, or one of your recovery codes."
          />
        )}
        {login.error && <Problem>{login.error.message}</Problem>}
        <Button type="submit" variant="primary" busy={login.isPending}>
          Sign in
        </Button>
      </form>
      {/* Only where a browser has passkeys at all: on the panel's own name, over TLS. */}
      {passkeysHere() && (
        <div className="mt-4 flex flex-col gap-3 border-t border-hairline pt-4">
          <Button busy={withPasskey.isPending} onClick={() => withPasskey.mutate(undefined)}>
            <KeyRound aria-hidden size={16} />
            Sign in with a passkey
          </Button>
          {withPasskey.error && <Problem>{withPasskey.error.message}</Problem>}
        </div>
      )}
    </Doorway>
  )
}
