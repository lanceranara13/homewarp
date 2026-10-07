---
version: alpha
name: Homewarp
description: >
  A calm, dense, dark-first control panel for game servers hosted at home behind a
  VPS gate. Deepslate neutrals stacked as a surface ladder with hairline borders,
  ink-on-canvas primary actions, one violet accent for selection and focus, and
  colour reserved for server state. Monospace for everything a player or admin
  would copy: addresses, ports, commands, logs.

colors:
  # Dark theme (default)
  canvas: "#0B0C0E"
  surface-1: "#111316"
  surface-2: "#16191D"
  surface-3: "#1C2025"
  hairline: "#23272E"
  hairline-strong: "#323842"
  ink: "#F4F5F7"
  ink-muted: "#B6BCC6"
  ink-subtle: "#8A919E"
  ink-faint: "#5F6672"
  primary: "#F4F5F7"
  on-primary: "#0B0C0E"
  accent: "#8F7CFF"
  accent-solid: "#6E56F8"
  on-accent: "#FFFFFF"
  accent-soft: "rgba(143, 124, 255, 0.12)"
  state-running: "#3DD68C"
  state-starting: "#F5B73D"
  state-crashed: "#FF6369"
  state-installing: "#5EB0EF"
  state-offline: "#8A919E"
  danger: "#FF6369"
  danger-solid: "#D93D42"
  heart: "#F0559A"
  overlay: "rgba(0, 0, 0, 0.6)"

  # Light theme
  light-canvas: "#F7F8F9"
  light-surface-1: "#FFFFFF"
  light-surface-2: "#F1F2F4"
  light-surface-3: "#E9EBEE"
  light-hairline: "#E4E6EA"
  light-hairline-strong: "#CDD1D8"
  light-ink: "#0E1014"
  light-ink-muted: "#3D434D"
  light-ink-subtle: "#646B77"
  light-ink-faint: "#9AA1AC"
  light-primary: "#0E1014"
  light-on-primary: "#FFFFFF"
  light-accent: "#5B43E8"
  light-accent-soft: "rgba(91, 67, 232, 0.08)"
  light-state-running: "#1A7F4B"
  light-state-starting: "#A35A00"
  light-state-crashed: "#CF222E"
  light-state-installing: "#1D6FD1"
  light-state-offline: "#646B77"

typography:
  page-title:
    fontFamily: Inter, system-ui, -apple-system, "Segoe UI", sans-serif
    fontSize: 24px
    fontWeight: 600
    lineHeight: 32px
    letterSpacing: -0.4px
  section-title:
    fontFamily: Inter, system-ui, -apple-system, "Segoe UI", sans-serif
    fontSize: 16px
    fontWeight: 600
    lineHeight: 24px
    letterSpacing: -0.1px
  body:
    fontFamily: Inter, system-ui, -apple-system, "Segoe UI", sans-serif
    fontSize: 14px
    fontWeight: 400
    lineHeight: 20px
  body-strong:
    fontFamily: Inter, system-ui, -apple-system, "Segoe UI", sans-serif
    fontSize: 14px
    fontWeight: 500
    lineHeight: 20px
  small:
    fontFamily: Inter, system-ui, -apple-system, "Segoe UI", sans-serif
    fontSize: 13px
    fontWeight: 400
    lineHeight: 18px
  caption:
    fontFamily: Inter, system-ui, -apple-system, "Segoe UI", sans-serif
    fontSize: 12px
    fontWeight: 500
    lineHeight: 16px
    letterSpacing: 0.2px
  stat:
    fontFamily: Inter, system-ui, -apple-system, "Segoe UI", sans-serif
    fontSize: 20px
    fontWeight: 600
    lineHeight: 24px
    fontFeature: tabular-nums
  mono:
    fontFamily: "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace
    fontSize: 13px
    fontWeight: 400
    lineHeight: 20px
  mono-small:
    fontFamily: "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace
    fontSize: 12px
    fontWeight: 400
    lineHeight: 16px

rounded:
  xs: 4px
  sm: 6px
  md: 8px
  lg: 12px
  pill: 9999px

spacing:
  xxs: 4px
  xs: 8px
  sm: 12px
  md: 16px
  lg: 24px
  xl: 32px
  2xl: 48px

components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    typography: "{typography.body-strong}"
    rounded: "{rounded.md}"
    height: 32px
    padding: 0 12px
  button-secondary:
    backgroundColor: "{colors.surface-1}"
    textColor: "{colors.ink}"
    border: "1px solid {colors.hairline-strong}"
    rounded: "{rounded.md}"
    height: 32px
    padding: 0 12px
  button-danger:
    backgroundColor: "{colors.danger-solid}"
    textColor: "#FFFFFF"
    rounded: "{rounded.md}"
    height: 32px
    padding: 0 12px
  input:
    backgroundColor: "{colors.surface-1}"
    textColor: "{colors.ink}"
    border: "1px solid {colors.hairline-strong}"
    rounded: "{rounded.md}"
    height: 32px
    padding: 0 10px
  card:
    backgroundColor: "{colors.surface-1}"
    border: "1px solid {colors.hairline}"
    rounded: "{rounded.lg}"
    padding: "{spacing.md}"
  status-pill:
    typography: "{typography.caption}"
    rounded: "{rounded.pill}"
    height: 22px
    padding: 0 8px 0 6px
  address-chip:
    backgroundColor: "{colors.surface-2}"
    textColor: "{colors.ink}"
    typography: "{typography.mono}"
    rounded: "{rounded.sm}"
    height: 28px
    padding: 0 8px
  sidebar:
    backgroundColor: "{colors.canvas}"
    width: 232px
    collapsedWidth: 56px
  topbar:
    backgroundColor: "{colors.canvas}"
    height: 48px
  console:
    backgroundColor: "#08090A"
    textColor: "#D7DAE0"
    typography: "{typography.mono}"
    rounded: "{rounded.lg}"
---

# Homewarp — design system

## Overview

Homewarp is a tool people open for thirty seconds: start a server, read the last few log
lines, copy an address for a friend. The interface is built for that — **state first,
actions second, everything else out of the way**.

Three rules drive every decision:

1. **Colour means state.** Green, amber, red and blue appear only where a server or
   the tunnel is running, changing, broken or installing. Buttons and chrome are
   neutral, so any colour on screen is information.
2. **One accent.** Violet marks what is selected or focused, and nothing else.
3. **Anything you would copy is monospace and one click away.** Addresses, ports,
   commands, tokens.

Dark is the default theme; light is fully supported and follows the system setting.

### Where the ideas come from

| Source | What is borrowed |
|---|---|
| Linear (`awesome-design-md`) | Four-step surface ladder with hairline borders instead of drop shadows; a single reserved accent; tight negative tracking on titles. |
| Vercel (`awesome-design-md`, 2026 dashboard) | Ink-on-canvas primary button; mono captions for technical labels; resizable/collapsible sidebar; a floating bottom bar on mobile. |
| Pterodactyl / Pelican | The per-server tab set (Console · Files · Backups · Schedules · Network · Startup · Users · Settings), kept deliberately so migrating users are immediately at home. |
| MCSManager | "Which servers are up and how full are they" answerable from the first screen. |

Nothing is copied wholesale — tokens, palette and components here are Homewarp's own.

## Colors

### Neutrals ("deepslate")

Cool near-black with a faint blue cast. Depth comes from stepping up the ladder,
never from shadow.

| Token | Dark | Light | Use |
|---|---|---|---|
| canvas | `#0B0C0E` | `#F7F8F9` | Page, sidebar, top bar |
| surface-1 | `#111316` | `#FFFFFF` | Cards, tables, inputs |
| surface-2 | `#16191D` | `#F1F2F4` | Hover, selected row, chips |
| surface-3 | `#1C2025` | `#E9EBEE` | Popovers, menus, pressed |
| hairline | `#23272E` | `#E4E6EA` | Default 1px borders, dividers |
| hairline-strong | `#323842` | `#CDD1D8` | Input and secondary-button borders |

### Text

| Token | Dark | Light | Use |
|---|---|---|---|
| ink | `#F4F5F7` | `#0E1014` | Titles, values, primary text |
| ink-muted | `#B6BCC6` | `#3D434D` | Body copy |
| ink-subtle | `#8A919E` | `#646B77` | Labels, metadata, placeholders |
| ink-faint | `#5F6672` | `#9AA1AC` | Disabled and decorative only — never for text that must be read |

### Accent ("portal violet")

| Token | Dark | Light | Use |
|---|---|---|---|
| accent | `#8F7CFF` | `#5B43E8` | Links, focus ring, selected nav item, active tab underline, the tunnel line |
| accent-solid | `#6E56F8` | `#6E56F8` | Rare filled accents (wizard step in progress) with white text |
| accent-soft | 12 % violet | 8 % violet | Selected-row and selected-nav background |

### State

| State | Dark | Light | Shown as |
|---|---|---|---|
| Running | `#3DD68C` | `#1A7F4B` | Solid dot |
| Starting / Stopping | `#F5B73D` | `#A35A00` | Pulsing dot |
| Crashed / Error | `#FF6369` | `#CF222E` | Solid dot + icon |
| Installing | `#5EB0EF` | `#1D6FD1` | Spinner |
| Offline | `#8A919E` | `#646B77` | Hollow ring |

State pills use the state colour for dot and text over a 12 % tint of the same colour.

Contrast: text tokens target WCAG AA (4.5:1) on `canvas` and `surface-1` in both
themes. The values above were hand-checked approximately; verify with tooling when
the tokens are implemented, and adjust lightness rather than hue if any fall short.

## Typography

- **Inter** for the interface, **JetBrains Mono** for technical values. Both are
  bundled with the app — no requests to font CDNs, so the panel works on an offline LAN.
- Base size is **14px**. This is a dense tool, not a marketing page.
- Numbers that change (CPU, memory, players, latency) use `tabular-nums` so they
  do not jitter.

| Token | Size / line | Weight | Use |
|---|---|---|---|
| page-title | 24 / 32 | 600 | One per page |
| section-title | 16 / 24 | 600 | Card and section headings |
| body | 14 / 20 | 400 | Default |
| body-strong | 14 / 20 | 500 | Buttons, table primary column, nav |
| small | 13 / 18 | 400 | Helper text, table secondary |
| caption | 12 / 16 | 500 | Labels above values, pills |
| stat | 20 / 24 | 600 | Stat tiles |
| mono | 13 / 20 | 400 | Addresses, ports, console, code |
| mono-small | 12 / 16 | 400 | Inline IDs, timestamps |

Rules: sentence case everywhere; no all-caps labels; no more than two weights visible
in one card.

## Layout

### Spacing

4px base: `4 · 8 · 12 · 16 · 24 · 32 · 48`. Card padding 16, gap between cards 16,
page gutter 24 (16 on mobile), section spacing 32.

### App shell

```
┌──────────────┬──────────────────────────────────────────────────────────────┐
│  ◈ Homewarp  │  Servers                                  ⌘K Search   + New  │ 48
├──────────────┼──────────────────────────────────────────────────────────────┤
│ ▣ Servers    │                                                              │
│ ◫ Templates  │                     page content                             │
│ ⇄ Network    │                     max-width 1200, centred                  │
│ ≡ Activity   │                                                              │
│ ⚙ Settings   │                                                              │
│              │                                                              │
│              │                                                              │
├──────────────┤                                                              │
│ ● Gate 23 ms │                                                              │
│ RAM ▓▓▓░ 62% │                                                              │
│ (L) lance  ▾ │                                                              │
└──────────────┴──────────────────────────────────────────────────────────────┘
     232px
```

- **Five destinations, no nesting.** Sidebar collapses to 56px icons (`[`).
- The **Gate pill** is pinned bottom-left on every page: the tunnel's health is the
  one thing that affects everything, so it is always in view. Click → Network.
- Top bar: page title or breadcrumb on the left; search / command palette and the
  single page-level primary action on the right.

### Grid

- Content max-width 1200px. Server cards: 3 columns ≥ 1200, 2 ≥ 768, 1 below.
- Forms are a single 560px column. Never two-column forms.
- Tables are full width, 40px rows.

## Elevation & Depth

| Level | Treatment | Use |
|---|---|---|
| 0 | `canvas`, no border | Page, sidebar |
| 1 | `surface-1` + 1px `hairline` | Cards, tables |
| 2 | `surface-2` | Hover and selected inside level 1 |
| 3 | `surface-3` + 1px `hairline-strong` + shadow `0 8px 24px rgba(0,0,0,.4)` | Menus, popovers, dialogs, command palette |
| Focus | 2px `accent` outline, 2px offset | Any focused control |

Shadows exist only on floating layers (level 3). Nothing in the page flow casts one.

## Shapes

| Token | Value | Use |
|---|---|---|
| xs | 4px | Checkboxes, tiny tags |
| sm | 6px | Chips, menu items |
| md | 8px | Buttons, inputs |
| lg | 12px | Cards, dialogs, console |
| pill | 9999px | Status pills, avatars |

## Components

### Buttons

32px high (40px on touch), 8px radius, 14/500 label, optional 16px leading icon.

- **Primary** — ink on canvas (white button in dark, black in light). At most one per view.
- **Secondary** — `surface-1` with a strong hairline.
- **Ghost** — text only; `surface-2` on hover. Toolbars and table rows.
- **Danger** — red fill; appears only inside a confirmation dialog, never in a page.

### Power controls

The signature component. A segmented group in the server header, always visible:

```
[ ▶ Start ] [ ↻ Restart ] [ ■ Stop ]  ⋯
```

- Only valid actions are enabled for the current state; the rest are dimmed.
- Clicking flips the status pill immediately to *Starting…* / *Stopping…* and the
  button shows a spinner until the server confirms.
- **Kill** lives in the `⋯` menu and appears as a direct button only when a stop has
  been pending for more than 10 seconds.

### Status pill

`● Running` — dot + word, always both. Never colour alone.

### Address chip

```
┌─────────────────────────┐
│ play.example.com:25565 ⧉│
└─────────────────────────┘
```

Monospace, `surface-2`. Clicking anywhere copies and shows "Copied" for 1.5 s.
Used for server addresses, IPs, ports, tokens and install commands.

### Server card

```
┌────────────────────────────────────────┐
│ [icon] Survival             ● Running  │
│        Paper 1.21 · Minecraft          │
│                                        │
│ ┌────────────────────────────────────┐ │
│ │ 203.0.113.10:25565               ⧉ │ │
│ └────────────────────────────────────┘ │
│                                        │
│ CPU 34%   RAM 2.1 / 4 GB   Players 3/20│
│ ▁▂▃▅▃▂    ▅▅▆▆▆▆                       │
│                                        │
│                     [ ↻ ]  [ ■ Stop ]  │
└────────────────────────────────────────┘
```

Whole card opens the server; the address chip and power buttons are separate targets.
Stats and sparklines stream in after first paint; the card never waits for them.

### Stat tile

Caption label, `stat` value, optional 32px sparkline (uPlot), optional limit bar.
Bar turns amber above 80 % and red above 95 %.

### Console

Near-black panel in both themes (`#08090A`), 13px mono, ANSI colours supported.

- Toolbar: search, pause auto-scroll, clear view, download log.
- Input row pinned at the bottom with a `>` prompt and ↑/↓ history.
- When the user scrolls up, auto-scroll pauses and a "Jump to latest" pill appears.
- Lines matching a known prompt (e.g. the Minecraft EULA) raise an inline action
  banner above the input rather than a blocking modal.

### Tabs

Underline tabs on a hairline. Active tab: `ink` text + 2px `accent` underline.
Scroll horizontally on narrow screens; never wrap to two lines.

Settings is the one page whose groups are listed down its left side instead: a 192px
rail of rows drawn as the sidebar's are (`accent-soft` behind the open one), with the
open group's sections beside it. Below 768px, where there is no room for a rail, the
same groups are underline tabs.

### Tables

`surface-1`, hairline row dividers, no zebra striping. Primary column `body-strong`.
Row actions are ghost buttons revealed on hover and always present on touch.

### Forms

Label above field, helper text below, error replaces helper in the crashed colour with
an icon. Validate on blur, not on every keystroke. Advanced options sit in a collapsed
"Advanced" section — the default path is always short.

### Dialogs

480px, `surface-3`, 12px radius. Destructive confirmations require typing the server
name. The danger button sits on the right; Cancel is the default focus.

### Toasts

Bottom-right, one line, 4 s, with Undo where the action is reversible. Errors stay
until dismissed and include a "Details" disclosure.

### Command palette

`⌘K` / `Ctrl K`. Searches servers, pages and actions ("start survival", "new server",
"copy address creative"). 560px, centred, level-3 surface.

### Empty states

One muted icon, one sentence, one primary button. Example — Servers with nothing yet:

> **No servers yet.**
> Pick a game and Homewarp handles the rest.
> `[ + New server ]`

### Donation heart

Homewarp is free and supported by donations. The ask is one small pixel heart, drawn
the way games draw health.

```svg
<svg viewBox="0 0 7 6" width="14" height="12" shape-rendering="crispEdges" aria-hidden="true">
  <path fill="#F0559A" d="M1 0h2v1h1V0h2v1h1v2h-1v1h-1v1h-1v1H3V5H2V4H1V3H0V1h1z"/>
  <rect x="1" y="1" width="1" height="1" fill="#fff" opacity=".65"/>
</svg>
```

- A 7 × 6 pixel grid with one highlight pixel. Always rendered at whole multiples
  (14 × 12, 28 × 24 …) so the pixels stay sharp.
- Colour `heart` (`#F0559A`), the same in both themes. Pink sits outside the state palette,
  and it is **the only decorative colour in the product** — which is why it gets noticed.
- The same drawing ships as `assets/heart.svg` and is used in the README and the docs.
- Lives at the bottom of the sidebar next to the label "Give a heart" (icon only when
  the sidebar is collapsed, and in the bottom bar's More sheet on mobile).
- Hover or focus: the heart beats twice (scale 1 → 1.16 → 1 → 1.1 → 1 over 900 ms).
  No motion under `prefers-reduced-motion`.
- Click opens a popover, never a modal:

  > **Give Homewarp a heart**
  > Homewarp is free and stays free. If it saved you a hosting bill, a heart keeps it going.
  > `[ GitHub Sponsors ]` `[ Ko-fi ]`
  > You can hide the heart in Settings.

- Once per install, when the first server reaches Running, one toast:
  "*Survival* is live. Homewarp is free, kept going by hearts." with a "Give a heart" link.
- Settings → About repeats the links and has the switch that hides the heart.

Never: a modal, a banner, a countdown, a prompt at startup, anything on the console or
file pages, or any tracking of who clicked.

### Tunnel diagram

Used on the Network page and in the connect wizard:

```
  Players ───────► Gate ═══════════════► Home ───────► 3 servers
  internet      203.0.113.10   23 ms      this machine
                Frankfurt      0 % loss
```

Lines are `accent` when healthy, amber when degraded, red and dashed when down.
Each node is a small card with its own status pill.

## Information architecture

### Global navigation

| Item | Contains |
|---|---|
| **Servers** | All servers as cards (or a table for 10+). Landing page. |
| **Templates** | Installed templates; browse catalogue; import an egg file or URL. |
| **Network** | Gate status, tunnel health, forwarded ports, domains, client-IP mode. |
| **Activity** | Audit log and system events, filterable by server and user. |
| **Settings** | Account, Users, Security, Backups, System, Updates. |

### Inside a server

Sticky header (name · status pill · address chip · power controls), then tabs:

| Tab | Purpose |
|---|---|
| **Console** (default) | Live log + command input; stats rail on the right. |
| **Files** | Browser, editor, upload, archive/extract. |
| **Backups** | Create, restore, download, retention. |
| **Schedules** | Timed commands, restarts, backups. |
| **Network** | This server's ports, public address, SRV hint. |
| **Startup** | Template variables, image/version, startup command. |
| **Users** | Sub-users and their permissions. |
| **Settings** | Rename, resources, reinstall, delete. |

### Key screens

**Server → Console**

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ Servers / Survival                                                           │
│ Survival  ● Running   [203.0.113.10:25565 ⧉]     [▶ Start][↻ Restart][■ Stop]│
│ Console  Files  Backups  Schedules  Network  Startup  Users  Settings        │
│ ───────                                                                      │
│ ┌──────────────────────────────────────────────────┐ ┌─────────────────────┐ │
│ │ [12:01:07 INFO] Done (8.2s)! For help, type "help"│ │ CPU          34 %   │ │
│ │ [12:03:41 INFO] Alex joined the game             │ │ ▁▂▃▅▃▂▃▅            │ │
│ │ [12:04:02 INFO] <Alex> hello                     │ ├─────────────────────┤ │
│ │                                                  │ │ Memory  2.1 / 4 GB  │ │
│ │                                                  │ │ ▓▓▓▓▓▓░░░░░         │ │
│ │                                                  │ ├─────────────────────┤ │
│ │                                                  │ │ Disk    1.3 / 10 GB │ │
│ │                                                  │ ├─────────────────────┤ │
│ │                                                  │ │ Players      3 / 20 │ │
│ │                                                  │ │ Alex · Sam · Jo     │ │
│ ├──────────────────────────────────────────────────┤ ├─────────────────────┤ │
│ │ > say hello_                                     │ │ Uptime      2 h 14 m│ │
│ └──────────────────────────────────────────────────┘ └─────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────────┘
```

**New server (three steps, one decision each)**

```
  ① Choose a game ──── ② Configure ──── ③ Install

  ①  [ search templates…            ]
      Popular
      ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐
      │ Paper   │ │ Fabric  │ │ Valheim │ │ Terraria│
      └─────────┘ └─────────┘ └─────────┘ └─────────┘

  ②  Name        [ Survival            ]
      Version     [ Latest           ▾ ]
      Memory      ──────●────────  4 GB     (8.5 GB free on this machine)
      Address     203.0.113.10:25565        (assigned automatically)
      ▸ Advanced

  ③  Live install log → lands on the Console tab, already starting.
```

**Connect a VPS (first-run wizard)**

```
  ① VPS address ──── ② Run one command ──── ③ Verify

  ①  VPS IP or hostname   [ 203.0.113.10        ]

  ②  Run this on the VPS as root:
      ┌──────────────────────────────────────────────────────────┐
      │ curl -fsSL https://…/gate.sh | sudo sh -s -- eyJ2Ij…   ⧉ │
      └──────────────────────────────────────────────────────────┘
      ◌ Waiting for the gate to come online…   (token valid 14:32)

  ③  ✓ Tunnel up            23 ms
      ✓ Keys rotated
      ✓ Player IPs preserved
                                              [ Create first server ]
```

**Network**

```
  [ tunnel diagram ]

  Forwarded ports
  Public                 Protocol   Server        Traffic (24 h)
  203.0.113.10:25565     TCP        Survival      1.2 GB
  203.0.113.10:19132     UDP        Bedrock       310 MB
  203.0.113.10:24454     UDP        Survival      88 MB     (voice chat)

  Player IP addresses    ● Preserved
  Panel access           LAN only                    [ Make public… ]
```

## Interaction & motion

- 120 ms ease-out for hover and press; 180 ms for dialogs and popovers. Nothing longer.
- Only transitional states pulse (starting, stopping). Running servers are still.
- Power actions are optimistic; a failure rolls the pill back and raises an error toast.
- Live values update in place — no layout shift, no skeleton flashing on refresh.
- `prefers-reduced-motion` removes pulses and transitions.

### Keyboard

| Keys | Action |
|---|---|
| `⌘K` / `Ctrl K` | Command palette |
| `/` | Focus search or the console input |
| `g` then `s` `t` `n` `a` | Go to Servers, Templates, Network, Activity |
| `[` | Collapse / expand sidebar |
| `1`–`8` | Switch server tab |
| `Esc` | Close the top-most layer |

## Responsive behavior

| Width | Changes |
|---|---|
| ≥ 1200 | Full shell; console with stats rail; 3-column cards |
| 768–1199 | Sidebar collapsed to icons; 2-column cards; stats rail moves above the console as a 4-tile strip |
| < 768 | Sidebar replaced by a floating bottom bar (Servers · Templates · Network · More); 1-column cards; tabs scroll; power controls become a sticky bar above the bottom nav |

- Touch targets 44px minimum; buttons grow from 32 to 40px.
- Starting or stopping a server and reading the console must be comfortable
  one-handed on a phone — that is the main mobile job.
- The console keeps 13px mono and wraps long lines on mobile instead of scrolling sideways.

## Accessibility

- State is always dot **and** word (and an icon for errors); never colour alone.
- Every control is reachable by keyboard with a visible 2px accent focus ring.
- Console output is an ARIA live region set to `polite`, and can be paused.
- Dialogs trap focus and restore it on close.
- Text tokens target WCAG AA; `ink-faint` is exempt and used only decoratively.
- All icons with meaning have text labels or `aria-label`s.

## Do's and Don'ts

### Do

- Use colour only for state; keep buttons and chrome neutral.
- Keep one primary button per view.
- Put the address chip wherever a server is shown — it is the thing users came for.
- Use monospace for anything copyable.
- Step up the surface ladder for depth; add a hairline before reaching for a shadow.
- Keep the default path short and tuck the rest into "Advanced".
- Write status and errors in plain words: "Gate unreachable — retrying in 5 s".

### Don't

- Don't use violet as a fill for cards, sections or primary buttons.
- Don't introduce a second accent colour or gradients.
- Don't use green, amber or red decoratively.
- Don't add a second decorative colour; the pink donation heart is the single exception.
- Don't turn the donation heart into a nag: no modals, banners or repeat prompts.
- Don't show a spinner in place of a page; render structure first and stream values.
- Don't hide Start/Stop inside a menu, on any screen size.
- Don't nest navigation more than two levels (global → server tab).
- Don't use modals for information; modals are for confirmation only.
- Don't load fonts, icons or scripts from third-party hosts.

## Agent prompt guide

Quick reference when generating UI for Homewarp:

- Background `#0B0C0E`, cards `#111316` with `1px solid #23272E`, radius 12px, padding 16px.
- Text `#F4F5F7` / body `#B6BCC6` / labels `#8A919E`. Font Inter 14px; technical values JetBrains Mono 13px.
- Primary button: `#F4F5F7` background, `#0B0C0E` text, 32px high, 8px radius. One per view.
- Accent `#8F7CFF` for links, focus rings, selected nav and active tab underline only.
- Server state: running `#3DD68C`, starting `#F5B73D`, crashed `#FF6369`,
  installing `#5EB0EF`, offline `#8A919E` — always a dot plus a word.
- Layout: 232px sidebar with five items, 48px top bar, content max-width 1200px.
- No shadows except on menus, dialogs and the command palette.

Example prompts:

- "Build the Servers page: a 3-column grid of server cards per DESIGN.md, each with
  name, template subtitle, status pill, address chip, CPU/RAM/players row and power
  buttons. Include the empty state."
- "Build the server header: breadcrumb, name, status pill, address chip, and the
  segmented power controls with disabled states for each lifecycle state."
- "Build the Connect a VPS wizard: three steps with a copyable command block, a
  waiting state with countdown, and a three-row verification checklist."
