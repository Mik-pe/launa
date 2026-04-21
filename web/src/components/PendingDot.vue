<script setup lang="ts">
/**
 * Animated ping dot indicating a pending state.
 * Uses full Tailwind class strings to ensure JIT compatibility.
 */
withDefaults(defineProps<{
  sizeClass?: string
  colorClass?: string
}>(), {
  sizeClass: 'h-2.5 w-2.5',
  colorClass: 'bg-blue-400',
})
</script>

<template>
  <span :class="['relative flex', sizeClass]">
    <span :class="['animate-ping absolute inline-flex h-full w-full rounded-full opacity-75', colorClass]" />
    <span :class="['relative inline-flex rounded-full', sizeClass, colorClass]" />
  </span>
</template>
