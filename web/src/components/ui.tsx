import { Check, CircleAlert, Copy, LoaderCircle } from 'lucide-react'
import { useEffect, useId, useState, type ComponentProps, type ReactNode } from 'react'

/** The Homewarp mark: a gate, seen from the front. */
export function Mark({ className = 'size-5' }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden className={className}>
      <path d="M8 1.5 14.5 8 8 14.5 1.5 8Z" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
      <path d="M8 5.5 10.5 8 8 10.5 5.5 8Z" fill="currentColor" />
    </svg>
  )
}

const BUTTON_VARIANTS = {
  // Ink on canvas. At most one of these per view.
  primary: 'bg-primary text-on-primary hover:opacity-90',
  secondary: 'border border-hairline-strong bg-surface-1 text-ink hover:bg-surface-2',
  ghost: 'text-ink-muted hover:bg-surface-2 hover:text-ink',
}

type ButtonProps = ComponentProps<'button'> & {
  variant?: keyof typeof BUTTON_VARIANTS
  /** Shows a spinner and refuses clicks while the action it started is running. */
  busy?: boolean
}

export function Button({ variant = 'secondary', busy = false, type = 'button', className = '', children, ...rest }: ButtonProps) {
  return (
    <button
      {...rest}
      type={type}
      disabled={rest.disabled || busy}
      className={`inline-flex h-10 items-center justify-center gap-2 rounded-md px-3 text-body font-medium transition-colors duration-120 ease-out disabled:opacity-50 md:h-8 ${BUTTON_VARIANTS[variant]} ${className}`}
    >
      {busy && <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />}
      {children}
    </button>
  )
}

type FieldProps = Omit<ComponentProps<'input'>, 'id'> & {
  label: string
  hint?: ReactNode
  /** Set in monospace: for values that are copied or typed exactly. */
  mono?: boolean
}

/** A labelled input: label above, help below. */
export function Field({ label, hint, mono = false, className = '', ...input }: FieldProps) {
  const id = useId()
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-caption text-ink-subtle">
        {label}
      </label>
      <input
        {...input}
        id={id}
        aria-describedby={hint ? `${id}-hint` : undefined}
        className={`h-10 rounded-md border border-hairline-strong bg-surface-1 px-2.5 text-ink placeholder:text-ink-faint md:h-8 ${mono ? 'font-mono text-mono' : 'text-body'} ${className}`}
      />
      {hint && (
        <div id={`${id}-hint`} className="text-small text-ink-subtle">
          {hint}
        </div>
      )}
    </div>
  )
}

/** What went wrong, in the server's own sentence. */
export function Problem({ children }: { children: ReactNode }) {
  return (
    <p role="alert" className="flex items-start gap-2 text-small text-danger">
      <CircleAlert aria-hidden className="mt-px size-4 shrink-0" />
      {children}
    </p>
  )
}

/** Anything a person would copy is monospace and one click away (DESIGN.md, Address chip). */
export function CopyChip({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const timer = window.setTimeout(() => setCopied(false), 1500)
    return () => window.clearTimeout(timer)
  }, [copied])

  return (
    <button
      type="button"
      title="Copy"
      onClick={() => void copy(text).then(setCopied)}
      className="inline-flex h-7 max-w-full items-center gap-2 rounded-sm bg-surface-2 px-2 font-mono text-mono text-ink transition-colors duration-120 ease-out hover:bg-surface-3"
    >
      <span aria-live="polite" className="truncate">
        {copied ? 'Copied' : text}
      </span>
      {copied ? (
        <Check aria-hidden className="size-3.5 shrink-0" />
      ) : (
        <Copy aria-hidden className="size-3.5 shrink-0 text-ink-subtle" />
      )}
    </button>
  )
}

/**
 * Browsers offer the clipboard API on HTTPS and localhost only, and a panel on
 * the home network is neither, so the old selection-based copy stays as a fallback.
 */
async function copy(text: string): Promise<boolean> {
  if (window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // Permission refused: try the other way.
    }
  }
  const scratch = document.createElement('textarea')
  scratch.value = text
  scratch.style.position = 'fixed'
  scratch.style.opacity = '0'
  document.body.append(scratch)
  scratch.select()
  const copied = document.execCommand('copy')
  scratch.remove()
  return copied
}

/** The top bar of a page inside the shell: its title, and room for its one primary action. */
export function PageBar({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <header className="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-hairline px-4 md:px-6">
      <h1 className="text-section text-ink">{title}</h1>
      {children}
    </header>
  )
}

/** The frame around the pages that come before the shell: setup and sign-in. */
export function Doorway({ title, lead, children }: { title: string; lead: string; children: ReactNode }) {
  return (
    <main className="flex min-h-dvh items-center justify-center p-4">
      <div className="w-full max-w-sm">
        <div className="mb-6 flex items-center gap-2 text-ink">
          <Mark />
          <span className="text-section">Homewarp</span>
        </div>
        <h1 className="text-title text-ink">{title}</h1>
        <p className="mt-1 mb-6">{lead}</p>
        {children}
      </div>
    </main>
  )
}
