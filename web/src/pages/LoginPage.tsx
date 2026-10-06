import { CodeNeeded, signIn } from '../api/client'
import { Button, Doorway, Field, Problem } from '../components/ui'
import { useEnter } from '../session'

export function LoginPage() {
  const login = useEnter(signIn)

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
    </Doorway>
  )
}
