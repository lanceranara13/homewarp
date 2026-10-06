import { queryOptions } from '@tanstack/react-query'

import { getDiskUse, listFiles, readFile } from './api/client'

/**
 * One folder of a server's files, or null where there is no such folder. The
 * key sits under the server's own, so that what is forgotten with a server
 * takes its files along.
 */
export function filesQuery(id: number, path: string) {
  return queryOptions({
    queryKey: ['servers', id, 'files', path],
    queryFn: () => listFiles(id, path),
    staleTime: 2_000,
  })
}

/**
 * A file's text for the editor, or null where there is no such file yet. Read
 * afresh each time the editor opens, and never again under what is being typed:
 * so its key is not under the server's, which is asked after again at every turn.
 */
export function fileTextQuery(id: number, path: string) {
  return queryOptions({
    queryKey: ['file-text', id, path],
    queryFn: () => readFile(id, path),
    staleTime: Infinity,
    gcTime: 0,
    retry: false,
  })
}

/** What a server's files take of the disk, and what is left of it. Every file is asked, so nothing waits for this. */
export function diskUseQuery(id: number) {
  return queryOptions({
    queryKey: ['servers', id, 'disk'],
    queryFn: () => getDiskUse(id),
    staleTime: 30_000,
  })
}

/** A path made of a folder and the name of something in it. The top folder is the empty path. */
export function joined(folder: string, name: string): string {
  return folder ? `${folder}/${name}` : name
}

/** The folder a path sits in. */
export function parentOf(path: string): string {
  return path.split('/').slice(0, -1).join('/')
}

/** The last part of a path: the name of what it leads to. */
export function nameOf(path: string): string {
  return path.split('/').pop() ?? path
}
