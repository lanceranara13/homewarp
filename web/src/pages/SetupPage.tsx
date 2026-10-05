import { setUp } from '../api/client'
import { Button, CopyChip, Doorway, Field, Problem } from '../components/ui'
import { useEnter } from '../session'

/** First run: the one account is created by whoever can read Homewarp's log. */
export function SetupPage() {
  const setup = useEnter(setUp)

  return (
    <Doorway title="Create your account" lead="This Homewarp is new. Its first account runs everything on it.">
      <form
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          const form = new FormData(event.currentTarget)
          const typed = (name: string) => String(form.get(name) ?? '')
          setup.mutate({ code: typed('code'), username: typed('username'), password: typed('password') })
        }}
      >
        <Field
          label="Setup code"
          name="code"
          mono
          required
          autoFocus
          autoComplete="off"
          autoCapitalize="characters"
          spellCheck={false}
          placeholder="XXXX-XXXX-XXXX"
          hint={
            <div className="flex flex-col items-start gap-1.5">
              <span>Homewarp wrote it to its log when it started. With Docker:</span>
              <CopyChip text="docker logs homewarp" />
            </div>
          }
        />
        <Field label="Username" name="username" required maxLength={32} autoComplete="username" spellCheck={false} />
        <Field
          label="Password"
          name="password"
          type="password"
          required
          minLength={10}
          maxLength={256}
          autoComplete="new-password"
          hint="At least 10 characters."
        />
        {setup.error && <Problem>{setup.error.message}</Problem>}
        <Button type="submit" variant="primary" busy={setup.isPending}>
          Create account
        </Button>
      </form>
    </Doorway>
  )
}
