<script setup>
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useStatusHistory } from '../composables/useApi'

const { data: history, loading, error } = useStatusHistory(200, 10000)

const canvasRef = ref(null)
const tooltipData = ref(null)
const mouseX = ref(-1)

const points = computed(() => {
  if (!history.value?.length) return []
  const reversed = [...history.value].reverse()
  return reversed.map(entry => {
    try {
      const payload = typeof entry.payload === 'string' ? JSON.parse(entry.payload) : entry.payload
      return {
        time: new Date(entry.received_at),
        current_temp: payload?.current_temp ?? null,
        set_temp: payload?.set_temp ?? null,
        is_heating: !!payload?.is_heating,
        pump1_on: !!payload?.pump1_on,
        pump2_on: !!payload?.pump2_on,
        pump3_on: !!payload?.pump3_on,
        pump4_on: !!payload?.pump4_on,
        pump5_on: !!payload?.pump5_on,
        pump6_on: !!payload?.pump6_on,
        circ_pump: !!payload?.circ_pump,
        blower: !!payload?.blower,
        light1: !!payload?.light1,
        light2: !!payload?.light2,
        mister: !!payload?.mister,
      }
    } catch {
      return null
    }
  }).filter(p => p != null)
})

const componentDefs = [
  { key: 'is_heating', label: 'Heater', color: '#f97316' },
  { key: 'circ_pump', label: 'Circ Pump', color: '#22d3ee' },
  { key: 'pump1_on', label: 'Pump 1', color: '#3b82f6' },
  { key: 'pump2_on', label: 'Pump 2', color: '#8b5cf6' },
  { key: 'pump3_on', label: 'Pump 3', color: '#ec4899' },
  { key: 'pump4_on', label: 'Pump 4', color: '#14b8a6' },
  { key: 'pump5_on', label: 'Pump 5', color: '#f43f5e' },
  { key: 'pump6_on', label: 'Pump 6', color: '#a855f7' },
  { key: 'blower', label: 'Blower', color: '#eab308' },
  { key: 'light1', label: 'Light 1', color: '#fbbf24' },
  { key: 'light2', label: 'Light 2', color: '#fcd34d' },
  { key: 'mister', label: 'Mister', color: '#06b6d4' },
]

const activeComponents = computed(() => {
  return componentDefs.filter(comp =>
    points.value.some(p => p[comp.key])
  )
})

const compCanvasRef = ref(null)
const compTooltipData = ref(null)
const compMouseX = ref(-1)

const chartPad = { top: 24, right: 24, bottom: 44, left: 52 }

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
  const pad = chartPad
  const cw = w - pad.left - pad.right
  const ch = h - pad.top - pad.bottom

  ctx.clearRect(0, 0, w, h)

  const pts = points.value
  if (pts.length < 2) {
    ctx.fillStyle = '#525252'
    ctx.font = '13px system-ui, sans-serif'
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    ctx.fillText('Waiting for temperature data...', w / 2, h / 2)
    return
  }

  let minT = Infinity, maxT = -Infinity
  for (const p of pts) {
    for (const v of [p.current_temp, p.set_temp]) {
      if (v != null) { minT = Math.min(minT, v); maxT = Math.max(maxT, v) }
    }
  }
  if (!isFinite(minT)) { minT = 0; maxT = 100 }
  const padT = Math.max((maxT - minT) * 0.15, 1)
  minT -= padT; maxT += padT

  const tMin = pts[0].time.getTime()
  const tMax = pts[pts.length - 1].time.getTime()
  const tRange = tMax - tMin || 1

  const xOf = t => pad.left + ((t - tMin) / tRange) * cw
  const yOf = v => pad.top + ch - ((v - minT) / (maxT - minT)) * ch

  // Grid
  ctx.strokeStyle = '#1c1c1c'
  ctx.lineWidth = 1
  const nLines = 5
  for (let i = 0; i <= nLines; i++) {
    const v = minT + (maxT - minT) * (i / nLines)
    const y = yOf(v)
    ctx.beginPath(); ctx.moveTo(pad.left, y); ctx.lineTo(w - pad.right, y); ctx.stroke()
    ctx.fillStyle = '#525252'
    ctx.font = '11px system-ui, sans-serif'
    ctx.textAlign = 'right'
    ctx.textBaseline = 'middle'
    ctx.fillText(v.toFixed(1), pad.left - 8, y)
  }

  // Time labels
  const nLabels = Math.min(6, pts.length)
  ctx.textAlign = 'center'
  ctx.textBaseline = 'top'
  for (let i = 0; i < nLabels; i++) {
    const idx = Math.floor(i * (pts.length - 1) / (nLabels - 1))
    const x = xOf(pts[idx].time.getTime())
    ctx.fillStyle = '#525252'
    ctx.font = '11px system-ui, sans-serif'
    ctx.fillText(pts[idx].time.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }), x, h - pad.bottom + 12)
  }

  // Split points into contiguous segments (break at null values)
  function buildSegments(key) {
    const segments = []
    let current = []
    for (const p of pts) {
      if (p[key] != null) {
        current.push({ x: xOf(p.time.getTime()), y: yOf(p[key]) })
      } else {
        if (current.length > 0) {
          segments.push(current)
          current = []
        }
      }
    }
    if (current.length > 0) segments.push(current)
    return segments
  }

  // Draw gap indicators (dashed lines between segments)
  function drawGaps(segments) {
    if (segments.length < 2) return
    ctx.strokeStyle = 'rgba(255,255,255,0.06)'
    ctx.lineWidth = 1
    ctx.setLineDash([4, 6])
    for (let i = 0; i < segments.length - 1; i++) {
      const last = segments[i][segments[i].length - 1]
      const next = segments[i + 1][0]
      ctx.beginPath()
      ctx.moveTo(last.x, last.y)
      ctx.lineTo(next.x, next.y)
      ctx.stroke()
    }
    ctx.setLineDash([])
  }

  // Bezier line helper with gradient fill
  function drawSmoothLine(coords, strokeColor, fillColorTop, fillColorBottom) {
    if (coords.length < 2) return
    ctx.beginPath()
    ctx.moveTo(coords[0].x, coords[0].y)

    // Catmull-Rom to Bezier approximation
    for (let i = 0; i < coords.length - 1; i++) {
      const p0 = coords[Math.max(0, i - 1)]
      const p1 = coords[i]
      const p2 = coords[i + 1]
      const p3 = coords[Math.min(coords.length - 1, i + 2)]

      const tension = 0.3
      const cp1x = p1.x + (p2.x - p0.x) * tension
      const cp1y = p1.y + (p2.y - p0.y) * tension
      const cp2x = p2.x - (p3.x - p1.x) * tension
      const cp2y = p2.y - (p3.y - p1.y) * tension

      ctx.bezierCurveTo(cp1x, cp1y, cp2x, cp2y, p2.x, p2.y)
    }

    // Stroke
    ctx.strokeStyle = strokeColor
    ctx.lineWidth = 2.5
    ctx.setLineDash([])
    ctx.stroke()

    // Gradient fill
    if (fillColorTop && fillColorBottom) {
      const last = coords[coords.length - 1]
      const first = coords[0]
      ctx.lineTo(last.x, pad.top + ch)
      ctx.lineTo(first.x, pad.top + ch)
      ctx.closePath()
      const grad = ctx.createLinearGradient(0, pad.top, 0, pad.top + ch)
      grad.addColorStop(0, fillColorTop)
      grad.addColorStop(1, fillColorBottom)
      ctx.fillStyle = grad
      ctx.fill()
    }
  }

  // Draw set temp segments (blue dashed)
  const setSegments = buildSegments('set_temp')
  for (const seg of setSegments) {
    if (seg.length >= 2) {
      ctx.setLineDash([6, 4])
      ctx.strokeStyle = '#3b82f6'
      ctx.lineWidth = 1.5
      ctx.beginPath()
      ctx.moveTo(seg[0].x, seg[0].y)
      for (let i = 1; i < seg.length; i++) ctx.lineTo(seg[i].x, seg[i].y)
      ctx.stroke()
      ctx.setLineDash([])
    }
  }
  drawGaps(setSegments)

  // Draw current temp segments (orange with gradient fill)
  const curSegments = buildSegments('current_temp')
  for (const seg of curSegments) {
    drawSmoothLine(seg, '#f97316', 'rgba(249,115,22,0.15)', 'rgba(249,115,22,0)')
  }
  drawGaps(curSegments)

  // Draw dots on current temp (all segments)
  for (const seg of curSegments) {
    for (const c of seg) {
      ctx.beginPath()
      ctx.arc(c.x, c.y, 2.5, 0, Math.PI * 2)
      ctx.fillStyle = '#f97316'
      ctx.fill()
    }
  }

  // Crosshair
  if (mouseX.value >= 0 && pts.length >= 2) {
    const mx = mouseX.value
    let nearest = null
    let minDist = Infinity
    for (const p of pts) {
      if (p.current_temp == null) continue
      const cx = xOf(p.time.getTime())
      const d = Math.abs(cx - mx)
      if (d < minDist) { minDist = d; nearest = p }
    }
    if (nearest) {
      const nx = xOf(nearest.time.getTime())
      const ny = yOf(nearest.current_temp)
      // Vertical line
      ctx.strokeStyle = 'rgba(255,255,255,0.1)'
      ctx.lineWidth = 1
      ctx.setLineDash([])
      ctx.beginPath()
      ctx.moveTo(nx, pad.top)
      ctx.lineTo(nx, pad.top + ch)
      ctx.stroke()
      // Dot highlight
      ctx.beginPath()
      ctx.arc(nx, ny, 5, 0, Math.PI * 2)
      ctx.fillStyle = '#f97316'
      ctx.fill()
      ctx.strokeStyle = 'rgba(249,115,22,0.4)'
      ctx.lineWidth = 3
      ctx.stroke()
    }
  }

  // Legend
  ctx.font = '12px system-ui, sans-serif'
  const ly = pad.top - 8
  ctx.fillStyle = '#f97316'
  ctx.fillRect(pad.left, ly - 4, 16, 3)
  ctx.fillStyle = '#737373'
  ctx.textAlign = 'left'
  ctx.textBaseline = 'middle'
  ctx.fillText('Current', pad.left + 22, ly - 2)

  ctx.setLineDash([4, 3])
  ctx.strokeStyle = '#3b82f6'
  ctx.lineWidth = 1.5
  ctx.beginPath(); ctx.moveTo(pad.left + 80, ly - 2); ctx.lineTo(pad.left + 96, ly - 2); ctx.stroke()
  ctx.setLineDash([])
  ctx.fillText('Target', pad.left + 102, ly - 2)
}

function handleMouseMove(e) {
  const canvas = canvasRef.value
  if (!canvas || points.value.length < 2) return
  const rect = canvas.getBoundingClientRect()
  mouseX.value = e.clientX - rect.left

  const mx = mouseX.value
  const pts = points.value
  const tMin = pts[0].time.getTime()
  const tMax = pts[pts.length - 1].time.getTime()
  const cw = rect.width - chartPad.left - chartPad.right
  const clickTime = tMin + ((mx - chartPad.left) / cw) * (tMax - tMin)

  let nearest = pts[0]
  let minDist = Infinity
  for (const p of pts) {
    const d = Math.abs(p.time.getTime() - clickTime)
    if (d < minDist) { minDist = d; nearest = p }
  }

  tooltipData.value = {
    time: nearest.time.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }),
    current_temp: nearest.current_temp,
    set_temp: nearest.set_temp,
  }
}

function handleMouseLeave() {
  mouseX.value = -1
  tooltipData.value = null
}

// Component activity chart
const compPad = { top: 8, right: 24, bottom: 28, left: 80 }

function drawCompChart() {
  const canvas = compCanvasRef.value
  if (!canvas) return
  const ctx = canvas.getContext('2d')
  const dpr = window.devicePixelRatio || 1
  const rect = canvas.getBoundingClientRect()
  canvas.width = rect.width * dpr
  canvas.height = rect.height * dpr
  ctx.scale(dpr, dpr)

  const w = rect.width
  const h = rect.height
  const pad = compPad
  const cw = w - pad.left - pad.right
  const ch = h - pad.top - pad.bottom

  ctx.clearRect(0, 0, w, h)

  const comps = activeComponents.value
  const pts = points.value
  if (!comps.length || pts.length < 2) {
    ctx.fillStyle = '#525252'
    ctx.font = '13px system-ui, sans-serif'
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    ctx.fillText('No component activity recorded', w / 2, h / 2)
    return
  }

  const rowH = ch / comps.length
  const barH = Math.max(rowH * 0.6, 4)

  const tMin = pts[0].time.getTime()
  const tMax = pts[pts.length - 1].time.getTime()
  const tRange = tMax - tMin || 1
  const xOf = t => pad.left + ((t - tMin) / tRange) * cw

  // Time labels
  const nLabels = Math.min(6, pts.length)
  ctx.textAlign = 'center'
  ctx.textBaseline = 'top'
  ctx.fillStyle = '#525252'
  ctx.font = '11px system-ui, sans-serif'
  for (let i = 0; i < nLabels; i++) {
    const idx = Math.floor(i * (pts.length - 1) / (nLabels - 1))
    const x = xOf(pts[idx].time.getTime())
    ctx.fillText(pts[idx].time.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }), x, h - pad.bottom + 8)
  }

  // Draw each component row
  for (let ci = 0; ci < comps.length; ci++) {
    const comp = comps[ci]
    const yCenter = pad.top + rowH * ci + rowH / 2
    const yTop = yCenter - barH / 2

    // Label
    ctx.fillStyle = '#737373'
    ctx.font = '11px system-ui, sans-serif'
    ctx.textAlign = 'right'
    ctx.textBaseline = 'middle'
    ctx.fillText(comp.label, pad.left - 8, yCenter)

    // Off background track
    ctx.fillStyle = 'rgba(255,255,255,0.03)'
    ctx.beginPath()
    ctx.roundRect(pad.left, yTop, cw, barH, 2)
    ctx.fill()

    // Find contiguous ON segments and draw them
    let segStart = null
    for (let pi = 0; pi < pts.length; pi++) {
      const on = pts[pi][comp.key]
      if (on && segStart === null) {
        segStart = pi
      } else if (!on && segStart !== null) {
        drawCompBar(ctx, pts, segStart, pi - 1, xOf, yTop, barH, comp.color)
        segStart = null
      }
    }
    if (segStart !== null) {
      drawCompBar(ctx, pts, segStart, pts.length - 1, xOf, yTop, barH, comp.color)
    }
  }

  // Crosshair
  if (compMouseX.value >= 0 && pts.length >= 2) {
    const mx = compMouseX.value
    ctx.strokeStyle = 'rgba(255,255,255,0.1)'
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.moveTo(mx, pad.top)
    ctx.lineTo(mx, pad.top + ch)
    ctx.stroke()
  }
}

function drawCompBar(ctx, pts, startIdx, endIdx, xOf, yTop, barH, color) {
  const x1 = xOf(pts[startIdx].time.getTime())
  const x2 = xOf(pts[endIdx].time.getTime())
  const barW = Math.max(x2 - x1, 2)
  ctx.fillStyle = color + '40'
  ctx.beginPath()
  ctx.roundRect(x1, yTop, barW, barH, 2)
  ctx.fill()
  ctx.strokeStyle = color + '80'
  ctx.lineWidth = 1
  ctx.stroke()
}

function handleCompMouseMove(e) {
  const canvas = compCanvasRef.value
  if (!canvas || points.value.length < 2 || !activeComponents.value.length) return
  const rect = canvas.getBoundingClientRect()
  compMouseX.value = e.clientX - rect.left

  const pts = points.value
  const tMin = pts[0].time.getTime()
  const tMax = pts[pts.length - 1].time.getTime()
  const cw = rect.width - compPad.left - compPad.right
  const clickTime = tMin + ((compMouseX.value - compPad.left) / cw) * (tMax - tMin)

  let nearest = pts[0]
  let minDist = Infinity
  for (const p of pts) {
    const d = Math.abs(p.time.getTime() - clickTime)
    if (d < minDist) { minDist = d; nearest = p }
  }

  compTooltipData.value = {
    time: nearest.time.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }),
    states: activeComponents.value.map(c => ({
      label: c.label,
      color: c.color,
      on: !!nearest[c.key],
    })),
  }
}

function handleCompMouseLeave() {
  compMouseX.value = -1
  compTooltipData.value = null
}

watch(points, () => nextTick(() => { drawChart(); drawCompChart() }), { deep: true })
onMounted(() => {
  drawChart()
  drawCompChart()
  window.addEventListener('resize', drawChartAndComp)
})
onUnmounted(() => {
  window.removeEventListener('resize', drawChartAndComp)
})

function drawChartAndComp() {
  drawChart()
  drawCompChart()
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-semibold text-white">Temperature History</h2>
      <span class="text-xs text-neutral-500">{{ points.length }} data points</span>
    </div>

    <div v-if="loading && !points.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="animate-spin h-8 w-8 mb-4 text-blue-400" fill="none" viewBox="0 0 24 24">
        <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
        <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
      </svg>
      <p class="text-sm">Loading history...</p>
    </div>
    <div v-else-if="error" class="bg-red-500/10 border border-red-500/20 rounded-2xl px-5 py-4 text-sm text-red-400">
      Error: {{ error }}
    </div>

    <template v-else>
      <!-- Temperature chart -->
      <div class="bg-neutral-900 rounded-2xl p-4 ring-1 ring-neutral-800 relative">
        <div class="relative" style="height: 320px;">
          <canvas
            ref="canvasRef"
            class="w-full h-full cursor-crosshair"
            @mousemove="handleMouseMove"
            @mouseleave="handleMouseLeave"
          />
        </div>
        <!-- Tooltip -->
        <Transition name="tooltip">
          <div v-if="tooltipData" class="absolute top-8 right-8 bg-neutral-800/95 backdrop-blur rounded-xl p-3.5 text-xs shadow-xl ring-1 ring-neutral-700 pointer-events-none min-w-[140px]">
            <p class="text-neutral-400 mb-2 font-medium">{{ tooltipData.time }}</p>
            <div class="space-y-1.5">
              <div class="flex items-center justify-between gap-4">
                <span class="flex items-center gap-1.5"><span class="w-2 h-0.5 rounded bg-orange-400" /> Current</span>
                <span class="text-orange-400 font-semibold tabular-nums">{{ tooltipData.current_temp ?? '--' }}°</span>
              </div>
              <div class="flex items-center justify-between gap-4">
                <span class="flex items-center gap-1.5"><span class="w-2 h-0.5 rounded bg-blue-400" /> Target</span>
                <span class="text-blue-400 font-semibold tabular-nums">{{ tooltipData.set_temp ?? '--' }}°</span>
              </div>
            </div>
          </div>
        </Transition>
      </div>

      <!-- Component activity chart -->
      <div v-if="activeComponents.length && points.length >= 2" class="bg-neutral-900 rounded-2xl p-4 ring-1 ring-neutral-800 relative">
        <h3 class="text-[11px] font-semibold text-neutral-500 uppercase tracking-[0.15em] mb-3 px-1">Component Activity</h3>
        <div :style="{ height: Math.max(activeComponents.length * 28 + 36, 100) + 'px' }">
          <canvas
            ref="compCanvasRef"
            class="w-full h-full cursor-crosshair"
            @mousemove="handleCompMouseMove"
            @mouseleave="handleCompMouseLeave"
          />
        </div>
        <!-- Component tooltip -->
        <Transition name="tooltip">
          <div v-if="compTooltipData" class="absolute top-8 right-8 bg-neutral-800/95 backdrop-blur rounded-xl p-3.5 text-xs shadow-xl ring-1 ring-neutral-700 pointer-events-none min-w-[140px]">
            <p class="text-neutral-400 mb-2 font-medium">{{ compTooltipData.time }}</p>
            <div class="space-y-1">
              <div
                v-for="s in compTooltipData.states"
                :key="s.label"
                class="flex items-center justify-between gap-4"
              >
                <span class="flex items-center gap-1.5">
                  <span class="w-2 h-2 rounded-full" :style="{ backgroundColor: s.on ? s.color : '#404040' }" />
                  {{ s.label }}
                </span>
                <span :class="s.on ? 'text-white font-medium' : 'text-neutral-600'">{{ s.on ? 'ON' : 'off' }}</span>
              </div>
            </div>
          </div>
        </Transition>
      </div>
    </template>
  </div>
</template>

<style scoped>
.tooltip-enter-active,
.tooltip-leave-active {
  transition: opacity 0.15s ease;
}
.tooltip-enter-from,
.tooltip-leave-to {
  opacity: 0;
}
</style>
