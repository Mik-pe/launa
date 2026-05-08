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

/**
 * Decode a raw chunks sniff capture into a structured display format.
 * Input: {"capture_us":N, "chunks":[["R",ts,"HEX"], ...]}
 */
export function decodeSniffChunks(parsed: any): {
  captureUs: number
  chunks: { dir: string; tsUs: number; hex: string; byteCount: number }[]
  rxBytes: number
  txBytes: number
  totalChunks: number
} | null {
  if (!parsed || !Array.isArray(parsed.chunks)) return null

  let rxBytes = 0
  let txBytes = 0

  const chunks = parsed.chunks
    .filter((c: any) => Array.isArray(c) && c.length >= 3)
    .map((c: any) => {
      const dir = c[0] === 'T' ? 'T' : 'R'
      const tsUs = typeof c[1] === 'number' ? c[1] : 0
      const hex = typeof c[2] === 'string' ? c[2] : ''
      const byteCount = hex.length / 2
      if (dir === 'R') rxBytes += byteCount
      else txBytes += byteCount
      return { dir, tsUs, hex, byteCount }
    })

  return {
    captureUs: typeof parsed.capture_us === 'number' ? parsed.capture_us : 0,
    chunks,
    rxBytes,
    txBytes,
    totalChunks: chunks.length,
  }
}
