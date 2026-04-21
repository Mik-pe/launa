import { ref, computed } from 'vue'
import type { SpaState } from '../types'

const RECONNECTING_TIMEOUT_MS = 30_000
const PENDING_TIMEOUT_MS = 5_000

const _stateKeyMap: Record<string, string> = {
  pump1: 'pump1_on', pump2: 'pump2_on', pump3: 'pump3_on',
  pump4: 'pump4_on', pump5: 'pump5_on', pump6: 'pump6_on',
  light1: 'light1', light2: 'light2', light3: 'light3', light4: 'light4',
  blower: 'blower', circulation_pump: 'circ_pump', mister: 'mister',
  hold_mode: 'hold_mode',
  heat_mode: 'heating_mode',
  temp_range: 'temp_range',
}

export function useSpaState() {
  const spaState = ref<SpaState | null>(null)
  const availability = ref('offline')
  const diagnostics = ref<Record<string, unknown> | string | null>(null)
  const alert_ = ref<string | null>(null)
  const pendingKeys = ref(new Set<string>())
  const selfTestEnabled = ref(false)
  const sniffEnabled = ref(false)
  let reconnectingTimer: ReturnType<typeof setTimeout> | null = null
  const pendingTimers = new Map<string, ReturnType<typeof setTimeout>>()

  function addPending(key: string): void {
    const next = new Set(pendingKeys.value)
    next.add(key)
    pendingKeys.value = next
    // Clear any existing timer for this key
    const existing = pendingTimers.get(key)
    if (existing !== undefined) clearTimeout(existing)
    // Auto-clear after timeout
    pendingTimers.set(key, setTimeout(() => {
      const n = new Set(pendingKeys.value)
      n.delete(key)
      pendingKeys.value = n
      pendingTimers.delete(key)
    }, PENDING_TIMEOUT_MS))
  }

  function onConnect() {
    availability.value = 'reconnecting'
    clearTimeout(reconnectingTimer!)
    reconnectingTimer = setTimeout(() => {
      if (availability.value === 'reconnecting') {
        availability.value = 'offline'
      }
    }, RECONNECTING_TIMEOUT_MS)
  }

  function handleMessage(topic: string, payload: string, deviceId: string) {
    const base = `launa/${deviceId}`

    if (topic === `${base}/state`) {
      try {
        const state = JSON.parse(payload) as SpaState
        spaState.value = state
        if (typeof state.self_test === 'boolean') {
          selfTestEnabled.value = state.self_test
        }
        if (typeof state.sniff_mode === 'boolean') {
          sniffEnabled.value = state.sniff_mode
        }
        if (pendingKeys.value.size > 0) {
          pendingKeys.value = new Set()
          for (const t of pendingTimers.values()) clearTimeout(t)
          pendingTimers.clear()
        }
      } catch { /* ignore */ }
    } else if (topic === `${base}/availability`) {
      clearTimeout(reconnectingTimer!)
      availability.value = payload
    } else if (topic === `${base}/diagnostics`) {
      try {
        diagnostics.value = JSON.parse(payload)
      } catch {
        diagnostics.value = payload
      }
    } else if (topic === `${base}/alert`) {
      alert_.value = payload
    }
  }

  function toggle(
    subtopic: string,
    publish: (subtopic: string, payload: string | number | boolean) => void,
  ) {
    const key = _stateKeyMap[subtopic]
    if (key) addPending(key)
    publish(subtopic, true)
  }

  function setTemperature(
    temp: number,
    publish: (subtopic: string, payload: string | number | boolean) => void,
  ) {
    addPending('set_temp')
    publish('set_temperature', String(temp))
  }

  function isPending(key: string): boolean {
    return pendingKeys.value.has(key)
  }

  function setSelfTest(
    enabled: boolean,
    publish: (subtopic: string, payload: string | number | boolean) => void,
  ) {
    selfTestEnabled.value = enabled
    publish('self_test', enabled ? 'ON' : 'OFF')
  }

  function setSniff(
    enabled: boolean,
    publish: (subtopic: string, payload: string | number | boolean) => void,
  ) {
    sniffEnabled.value = enabled
    publish('sniff', enabled ? 'ON' : 'OFF')
  }

  function cleanup() {
    clearTimeout(reconnectingTimer!)
    for (const t of pendingTimers.values()) clearTimeout(t)
    pendingTimers.clear()
  }

  return {
    spaState,
    availability,
    diagnostics,
    alert: alert_,
    pendingKeys,
    selfTestEnabled,
    sniffEnabled,
    onConnect,
    handleMessage,
    toggle,
    setTemperature,
    isPending,
    setSelfTest,
    setSniff,
    cleanup,
  }
}
