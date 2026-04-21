<script setup lang="ts">
withDefaults(defineProps<{
  class?: string
}>(), {
  class: 'h-8 w-8',
})
</script>

<template>
  <svg class="animate-spin text-blue-400" :class="$props.class" fill="none" viewBox="0 0 24 24">
    <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
    <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
  </svg>
</template>
