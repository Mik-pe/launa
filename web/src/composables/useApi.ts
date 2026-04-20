import { ref, onMounted, onUnmounted } from 'vue'
import type { Ref } from 'vue'
import type { LogEntry, StatusEntry, TimestampedEntry } from '../types'

function getDeviceId(): string {
  try {
    const saved = localStorage.getItem('launa-settings')
    if (saved) {
      const s = JSON.parse(saved)
      if (s.deviceId) return s.deviceId
    }
  } catch { /* ignore */ }
  return 'launa_spa'
}

const BASE = '/api/devices'

interface UseFetchReturn<T> {
  data: Ref<T>
  loading: Ref<boolean>
  error: Ref<string | null>
  refresh: () => Promise<void>
}

function useFetch<T>(url: string, intervalMs = 0, initialValue: T): UseFetchReturn<T> {
  const data = ref(initialValue) as Ref<T>
  const loading = ref(true)
  const error = ref<string | null>(null)

  let timer: ReturnType<typeof setInterval> | null = null

  async function refresh(): Promise<void> {
    try {
      const res = await fetch(url)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      data.value = await res.json()
      error.value = null
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      loading.value = false
    }
  }

  onMounted(() => {
    refresh()
    if (intervalMs > 0) {
      timer = setInterval(refresh, intervalMs)
    }
  })

  onUnmounted(() => {
    if (timer) clearInterval(timer)
  })

  return { data, loading, error, refresh }
}

export function useLatestStatus(intervalMs = 5000) {
  const url = `${BASE}/${getDeviceId()}/status/latest`
  return useFetch<StatusEntry | null>(url, intervalMs, null)
}

export function useStatusHistory(limit = 100, intervalMs = 10000) {
  const url = `${BASE}/${getDeviceId()}/status?limit=${limit}`
  return useFetch<StatusEntry[]>(url, intervalMs, [])
}

export function useLogs(limit = 100, intervalMs = 5000) {
  const url = `${BASE}/${getDeviceId()}/logs?limit=${limit}`
  return useFetch<LogEntry[]>(url, intervalMs, [])
}

export function useAlerts(limit = 100, intervalMs = 10000) {
  const url = `${BASE}/${getDeviceId()}/alerts?limit=${limit}`
  return useFetch<TimestampedEntry[]>(url, intervalMs, [])
}

export function useDiagnostics(limit = 100, intervalMs = 10000) {
  const url = `${BASE}/${getDeviceId()}/diagnostics?limit=${limit}`
  return useFetch<TimestampedEntry[]>(url, intervalMs, [])
}

export function useSniffFrames(limit = 100, intervalMs = 10000) {
  const url = `${BASE}/${getDeviceId()}/sniff?limit=${limit}`
  return useFetch<TimestampedEntry[]>(url, intervalMs, [])
}
