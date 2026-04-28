const MARGIN = 8

function positionTooltip(el: Element): void {
  const text = el.getAttribute('data-tooltip')
  if (!text) {
    const s = (el as HTMLElement).style
    s.removeProperty('--tooltip-left')
    s.removeProperty('--tooltip-transform')
    return
  }

  const rect = el.getBoundingClientRect()
  const vw = window.innerWidth

  // Estimate tooltip width from text content
  const estimatedWidth = text.length * 7.2 + 20

  const halfWidth = estimatedWidth / 2
  const center = rect.left + rect.width / 2
  const leftEdge = center - halfWidth
  const rightEdge = center + halfWidth

  const style = (el as HTMLElement).style

  if (leftEdge < MARGIN) {
    // Clamp to left viewport edge
    style.setProperty('--tooltip-left', `${MARGIN - rect.left}px`)
    style.setProperty('--tooltip-transform', 'none')
  } else if (rightEdge > vw - MARGIN) {
    // Clamp to right viewport edge
    style.setProperty('--tooltip-left', `${vw - MARGIN - rect.left}px`)
    style.setProperty('--tooltip-transform', 'translateX(-100%)')
  } else {
    // Default centered
    style.setProperty('--tooltip-left', '50%')
    style.setProperty('--tooltip-transform', 'translateX(-50%)')
  }
}

export function initTooltipBounds(): void {
  document.addEventListener('mouseenter', (e) => {
    const target = (e.target as Element)?.closest?.('[data-tooltip]')
    if (target) positionTooltip(target)
  }, true)
}
