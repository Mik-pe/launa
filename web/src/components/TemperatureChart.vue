<script setup>
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useStatusHistory } from '../composables/useApi.js'

const { data: history, loading, error } = useStatusHistory(200, 10000)

const canvasRef = ref(null)
const tooltipData = ref(null)

const points = computed(() => {
  if (!history.value?.length) return []
  // history is newest-first, reverse for chart (oldest-first)
  const reversed = [...history.value].reverse()
  return reversed.map(entry => {
    try {
      const payload = typeof entry.payload === 'string' ? JSON.parse(entry.payload) : entry.payload
      return {
        time: new Date(entry.received_at),
        current_temp: payload?.current_temp,
        set_temp: payload?.set_temp,
      }
    } catch {
      return null
    }
  }).filter(p => p && p.current_temp != null)
})

// Chart drawing
function drawChart() {
  const canvas = canvasRef.value
  if (!canvas) return
  const ctx = canvas.getContext('2d')
  const dpr = window.devicePixelRatio || 1

  const rect = canvas.getBoundingClientRect()
  canvas.width = rect.width * dpr
  canvas.height = rect.height * dpr
  ctx.scale(dpr, dpr)

  const w = rect.width
  const h = rect.height
  const pad = { top: 20, right: 20, bottom: 40, left: 50 }
  const chartW = w - pad.left - pad.right
  const chartH = h - pad.top - pad.bottom

  // Clear
  ctx.clearRect(0, 0, w, h)

  const pts = points.value
  if (pts.length < 2) {
    ctx.fillStyle = '#737373'
    ctx.font = '14px system-ui'
    ctx.textAlign = 'center'
    ctx.fillText('Waiting for temperature data...', w / 2, h / 2)
    return
  }

  // Compute ranges
  let minT = Infinity, maxT = -Infinity
  for (const p of pts) {
    if (p.current_temp != null) {
      minT = Math.min(minT, p.current_temp)
      maxT = Math.max(maxT, p.current_temp)
    }
    if (p.set_temp != null) {
      minT = Math.min(minT, p.set_temp)
      maxT = Math.max(maxT, p.set_temp)
    }
  }
  const padT = Math.max((maxT - minT) * 0.1, 1)
  minT -= padT
  maxT += padT

  const tMin = pts[0].time.getTime()
  const tMax = pts[pts.length - 1].time.getTime()
  const tRange = tMax - tMin || 1

  function xOf(t) { return pad.left + ((t - tMin) / tRange) * chartW }
  function yOf(v) { return pad.top + chartH - ((v - minT) / (maxT - minT)) * chartH }

  // Grid lines
  ctx.strokeStyle = '#262626'
  ctx.lineWidth = 1
  const nLines = 5
  for (let i = 0; i <= nLines; i++) {
    const v = minT + (maxT - minT) * (i / nLines)
    const y = yOf(v)
    ctx.beginPath()
    ctx.moveTo(pad.left, y)
    ctx.lineTo(w - pad.right, y)
    ctx.stroke()
    ctx.fillStyle = '#737373'
    ctx.font = '11px system-ui'
    ctx.textAlign = 'right'
    ctx.fillText(v.toFixed(1), pad.left - 6, y + 4)
  }

  // Time labels
  const nTimeLabels = Math.min(6, pts.length)
  ctx.fillStyle = '#737373'
  ctx.font = '11px system-ui'
  ctx.textAlign = 'center'
  for (let i = 0; i < nTimeLabels; i++) {
    const idx = Math.floor(i * (pts.length - 1) / (nTimeLabels - 1))
    const p = pts[idx]
    const x = xOf(p.time.getTime())
    const label = p.time.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
    ctx.fillText(label, x, h - pad.bottom + 20)
  }

  // Draw line helper
  function drawLine(key, color, dashed = false) {
    ctx.strokeStyle = color
    ctx.lineWidth = 2
    ctx.setLineDash(dashed ? [6, 3] : [])
    ctx.beginPath()
    let started = false
    for (const p of pts) {
      const v = p[key]
      if (v == null) continue
      const x = xOf(p.time.getTime())
      const y = yOf(v)
      if (!started) { ctx.moveTo(x, y); started = true }
      else ctx.lineTo(x, y)
    }
    ctx.stroke()
    ctx.setLineDash([])
  }

  drawLine('set_temp', '#3b82f6', true)   // Blue dashed for set temp
  drawLine('current_temp', '#f97316')       // Orange for current temp

  // Legend
  ctx.font = '12px system-ui'
  const legendY = pad.top - 6
  // Current temp
  ctx.strokeStyle = '#f97316'
  ctx.lineWidth = 2
  ctx.setLineDash([])
  ctx.beginPath(); ctx.moveTo(pad.left, legendY); ctx.lineTo(pad.left + 20, legendY); ctx.stroke()
  ctx.fillStyle = '#a3a3a3'
  ctx.textAlign = 'left'
  ctx.fillText('Current', pad.left + 24, legendY + 4)
  // Set temp
  ctx.strokeStyle = '#3b82f6'
  ctx.lineWidth = 2
  ctx.setLineDash([6, 3])
  ctx.beginPath(); ctx.moveTo(pad.left + 90, legendY); ctx.lineTo(pad.left + 110, legendY); ctx.stroke()
  ctx.setLineDash([])
  ctx.fillText('Target', pad.left + 114, legendY + 4)
}

// Store points data for mouse interaction
const pointsData = computed(() => points.value.map(p => ({
  x: 0, y: 0, // Will be computed relative to canvas
  ...p
})))

function handleMouseMove(e) {
  const canvas = canvasRef.value
  if (!canvas || points.value.length < 2) return

  const rect = canvas.getBoundingClientRect()
  const mx = e.clientX - rect.left

  const pad = { left: 50, right: 20, top: 20, bottom: 40 }
  const chartW = rect.width - pad.left - pad.right

  const pts = points.value
  const tMin = pts[0].time.getTime()
  const tMax = pts[pts.length - 1].time.getTime()
  const tRange = tMax - tMin || 1

  // Find nearest point by x position
  const clickTime = tMin + ((mx - pad.left) / chartW) * tRange
  let nearest = pts[0]
  let minDist = Infinity
  for (const p of pts) {
    const d = Math.abs(p.time.getTime() - clickTime)
    if (d < minDist) { minDist = d; nearest = p }
  }

  tooltipData.value = {
    time: nearest.time.toLocaleTimeString(),
    current_temp: nearest.current_temp,
    set_temp: nearest.set_temp,
  }
}

function handleMouseLeave() {
  tooltipData.value = null
}

watch(points, () => nextTick(drawChart), { deep: true })
onMounted(() => {
  drawChart()
  window.addEventListener('resize', drawChart)
})
onUnmounted(() => {
  window.removeEventListener('resize', drawChart)
})
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-semibold text-white">Temperature History</h2>
      <span class="text-xs text-neutral-500">{{ points.length }} data points</span>
    </div>

    <div v-if="loading && !points.length" class="text-center py-12 text-neutral-500">
      Loading history...
    </div>
    <div v-else-if="error" class="bg-red-950/50 border border-red-900/50 rounded-xl px-4 py-3 text-sm text-red-400">
      Error: {{ error }}
    </div>

    <div v-else class="bg-neutral-900 rounded-2xl p-4 ring-1 ring-neutral-800 relative">
      <div class="relative" style="height: 300px;">
        <canvas
          ref="canvasRef"
          class="w-full h-full cursor-crosshair"
          @mousemove="handleMouseMove"
          @mouseleave="handleMouseLeave"
        />
      </div>
      <!-- Tooltip -->
      <div v-if="tooltipData" class="absolute top-6 right-6 bg-neutral-800 rounded-lg p-3 text-xs shadow-lg ring-1 ring-neutral-700 pointer-events-none">
        <div class="text-neutral-400 mb-1">{{ tooltipData.time }}</div>
        <div class="flex gap-4">
          <span class="text-orange-400">{{ tooltipData.current_temp ?? '--' }}°</span>
          <span class="text-blue-400">{{ tooltipData.set_temp ?? '--' }}° target</span>
        </div>
      </div>
    </div>
  </div>
</template>
