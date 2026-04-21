import type { TimestampedEntry } from '../types'

/**
 * Format an ISO timestamp as a human-readable relative time string.
 */
export function timeAgo(iso: string): string {
  if (!iso) return ''
  const diff = (Date.now() - new Date(iso).getTime()) / 1000
  if (diff < 5) return 'just now'
  if (diff < 60) return `${Math.floor(diff)}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return new Date(iso).toLocaleDateString()
}

/**
 * Parse the payload field of a TimestampedEntry, handling both
 * string-encoded JSON and already-parsed objects.
 */
export function parsePayload(entry: TimestampedEntry): any {
  try {
    return typeof entry.payload === 'string' ? JSON.parse(entry.payload) : entry.payload
  } catch {
    return { raw: entry.payload }
  }
}
