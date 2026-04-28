import { ref, computed, onMounted, onUnmounted } from 'vue'
import type { Ref } from 'vue'
import type { LogEntry, StatusEntry, TimestampedEntry, AvailabilityEntry, GraphData } from '../types'

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
  const url = computed(() => {
    const base = `${BASE}/${getDeviceId()}/status`
    if (hoursRange.value != null) {
      return `${base}?hours=${hoursRange.value}`
    }
    return `${base}?limit=${limit}`
  })

  const data = ref<StatusEntry[]>([]) as Ref<StatusEntry[]>
  const loading = ref(true)
  const error = ref<string | null>(null)
  let timer: ReturnType<typeof setInterval> | null = null
  let fetchSeq = 0

  async function refresh(): Promise<void> {
    const seq = ++fetchSeq
    try {
      const res = await fetch(url.value)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const json = await res.json()
      if (seq !== fetchSeq) return
      data.value = json
      error.value = null
    } catch (e) {
      if (seq !== fetchSeq) return
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      if (seq === fetchSeq) loading.value = false
    }
  }

  const hoursRange = ref<number | null>(null)

  function setHoursRange(hours: number | null) {
    hoursRange.value = hours
    loading.value = true
    refresh()
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

  return { data, loading, error, refresh, hoursRange, setHoursRange }
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

async function deleteResource(path: string): Promise<void> {
  const res = await fetch(`${BASE}/${getDeviceId()}${path}`, { method: 'DELETE' })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
}

export function clearLogs() {
  return deleteResource('/logs')
}

export function clearAlerts() {
  return deleteResource('/alerts')
}

export function clearDiagnostics() {
  return deleteResource('/diagnostics')
}

export function clearSniffFrames() {
  return deleteResource('/sniff')
}

export function useAvailabilityHistory(limit = 500, intervalMs = 30000) {
  const url = computed(() => {
    const base = `${BASE}/${getDeviceId()}/availability/history`
    if (hoursRange.value != null) {
      return `${base}?hours=${hoursRange.value}`
    }
    return `${base}?limit=${limit}`
  })

  const data = ref<AvailabilityEntry[]>([]) as Ref<AvailabilityEntry[]>
  const loading = ref(true)
  const error = ref<string | null>(null)
  let timer: ReturnType<typeof setInterval> | null = null
  let fetchSeq = 0

  async function refresh(): Promise<void> {
    const seq = ++fetchSeq
    try {
      const res = await fetch(url.value)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const json = await res.json()
      if (seq !== fetchSeq) return
      data.value = json
      error.value = null
    } catch (e) {
      if (seq !== fetchSeq) return
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      if (seq === fetchSeq) loading.value = false
    }
  }

  const hoursRange = ref<number | null>(null)

  function setHoursRange(hours: number | null) {
    hoursRange.value = hours
    loading.value = true
    refresh()
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

  return { data, loading, error, refresh, hoursRange, setHoursRange }
}

export function useGraphHistory(intervalMs = 30000) {
  const url = computed(() => {
    const hours = hoursRange.value ?? 24
    return `${BASE}/${getDeviceId()}/status/graph?hours=${hours}`
  })

  const data = ref<GraphData>({ temperatures: [], components: [] }) as Ref<GraphData>
  const loading = ref(true)
  const error = ref<string | null>(null)
  let timer: ReturnType<typeof setInterval> | null = null
  let fetchSeq = 0

  async function refresh(): Promise<void> {
    const seq = ++fetchSeq
    try {
      const res = await fetch(url.value)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const json = await res.json()
      if (seq !== fetchSeq) return
      data.value = json
      error.value = null
    } catch (e) {
      if (seq !== fetchSeq) return
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      if (seq === fetchSeq) loading.value = false
    }
  }

  const hoursRange = ref<number | null>(null)

  function setHoursRange(hours: number | null) {
    hoursRange.value = hours
    loading.value = true
    refresh()
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

  return { data, loading, error, refresh, hoursRange, setHoursRange }
}
