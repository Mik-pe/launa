import { ref, onMounted, onUnmounted } from 'vue'

const DEVICE_ID = 'spa_001'
const BASE = '/api/devices'

/**
 * Generic auto-refreshing fetch composable.
 * @param {string} url
 * @param {number} intervalMs - refresh interval in ms (0 = no auto-refresh)
 * @param {*} initialValue
 */
export function useFetch(url, intervalMs = 0, initialValue = null) {
  const data = ref(initialValue)
  const loading = ref(true)
  const error = ref(null)

  let timer = null

  async function refresh() {
    try {
      const res = await fetch(url)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      data.value = await res.json()
      error.value = null
    } catch (e) {
      error.value = e.message
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
  const url = `${BASE}/${DEVICE_ID}/status/latest`
  return useFetch(url, intervalMs, null)
}

export function useStatusHistory(limit = 100, intervalMs = 10000) {
  const url = `${BASE}/${DEVICE_ID}/status?limit=${limit}`
  return useFetch(url, intervalMs, [])
}

export function useLogs(limit = 100, intervalMs = 5000) {
  const url = `${BASE}/${DEVICE_ID}/logs?limit=${limit}`
  return useFetch(url, intervalMs, [])
}

export function useAlerts(limit = 100, intervalMs = 10000) {
  const url = `${BASE}/${DEVICE_ID}/alerts?limit=${limit}`
  return useFetch(url, intervalMs, [])
}

export function useDiagnostics(limit = 100, intervalMs = 10000) {
  const url = `${BASE}/${DEVICE_ID}/diagnostics?limit=${limit}`
  return useFetch(url, intervalMs, [])
}

export function useSniffFrames(limit = 100, intervalMs = 10000) {
  const url = `${BASE}/${DEVICE_ID}/sniff?limit=${limit}`
  return useFetch(url, intervalMs, [])
}
