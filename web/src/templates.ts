import { queryOptions } from '@tanstack/react-query'

import { getTemplate, listTemplates } from './api/client'

/**
 * Templates change only when someone imports or removes one, and both drop
 * what is cached here. Half a minute keeps a page that has just loaded from
 * asking again as it mounts.
 */
const FRESH_FOR = 30_000

/** Every template: the one request the Templates page waits for. */
export const templatesQuery = queryOptions({
  queryKey: ['templates'],
  queryFn: listTemplates,
  staleTime: FRESH_FOR,
})

/** One template in full, or null where there is none. */
export function templateQuery(id: number) {
  return queryOptions({
    queryKey: ['templates', id],
    queryFn: () => getTemplate(id),
    staleTime: FRESH_FOR,
  })
}
