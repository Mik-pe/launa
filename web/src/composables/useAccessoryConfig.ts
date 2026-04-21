import { ref, computed } from 'vue'
import type { AccessoryConfig } from '../types'

export function useAccessoryConfig() {
  const serverConfig = ref<AccessoryConfig | null>(null)

  async function fetchServerConfig() {
    try {
      const res = await fetch('/api/config')
      if (res.ok) {
        serverConfig.value = await res.json() as AccessoryConfig
      }
    } catch { /* ignore */ }
  }

  async function saveAccessoryConfig(newCfg: AccessoryConfig) {
    try {
      const res = await fetch('/api/config', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(newCfg),
      })
      if (res.ok) {
        serverConfig.value = await res.json() as AccessoryConfig
      }
    } catch { /* ignore */ }
  }

  const visibleControls = computed<Record<string, boolean>>(() => {
    const cfg = serverConfig.value
    if (!cfg) return {}
    const vc: Record<string, boolean> = {}
    for (let i = 1; i <= 6; i++) vc['pump' + i] = i <= cfg.pumps
    for (let i = 1; i <= 4; i++) vc['light' + i] = i <= cfg.lights
    vc.blower = cfg.blower
    vc.mister = cfg.mister
    return vc
  })

  return {
    serverConfig,
    visibleControls,
    fetchServerConfig,
    saveAccessoryConfig,
  }
}
