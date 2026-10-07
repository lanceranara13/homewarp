/** A number of bytes as a person reads it: 512 B, 3.4 MB, 1.2 GB. */
export function bytes(count: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = count
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${unit === 0 || value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`
}

/** A number of bytes a second, as a person reads it: 3.4 MB/s. */
export function rate(perSecond: number): string {
  return `${bytes(perSecond)}/s`
}

const MOMENT =new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' })

/** A moment given in Unix seconds, written the way the reader's own country writes dates. */
export function when(unix: number): string {
  return MOMENT.format(unix * 1000)
}
