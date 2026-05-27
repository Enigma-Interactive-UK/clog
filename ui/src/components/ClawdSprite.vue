<script setup lang="ts">
/**
 * Pixel-art Clawd sprite, shared between the About modal (static inline
 * mascot) and ClawdCameo.vue (Konami easter egg). The sprite is a 10x8
 * grid drawn from a fixed set of run-length-compressed rects in the
 * template -- no runtime per-pixel iteration. Fill colour defaults to
 * --accent so the mascot themes automatically.
 *
 * The body and the two alternating leg pairs are kept as separate <g>
 * groups so ClawdCameo can animate the legs independently for the
 * scuttle gait. Static consumers just render all three with no
 * animation and they read as a single sprite.
 *
 * Source grid (X = pixel, . = empty):
 *   .XXXXXXXX.
 *   .XXXXXXXX.
 *   XX.XXXX.XX
 *   XXXXXXXXXX
 *   .XXXXXXXX.
 *   .XXXXXXXX.
 *   .X.X..X.X.
 *   .X.X..X.X.
 */
import { computed } from 'vue'

const props = withDefaults(defineProps<{ pixel?: number }>(), { pixel: 4 })

const W = 10
const H = 8

const width = computed(() => W * props.pixel)
const height = computed(() => H * props.pixel)
</script>

<template>
  <svg
    class="clawd-sprite"
    :width="width"
    :height="height"
    :viewBox="`0 0 ${W} ${H}`"
    shape-rendering="crispEdges"
    aria-hidden="true"
  >
    <g class="body">
      <!-- Upper shell, rows 0-1 -->
      <rect x="1" y="0" width="8" height="2" />
      <!-- Row 2: eye slots at x=2 and x=7 split the row into three runs -->
      <rect x="0" y="2" width="2" height="1" />
      <rect x="3" y="2" width="4" height="1" />
      <rect x="8" y="2" width="2" height="1" />
      <!-- Row 3: full-width waist -->
      <rect x="0" y="3" width="10" height="1" />
      <!-- Lower shell, rows 4-5 -->
      <rect x="1" y="4" width="8" height="2" />
    </g>
    <g class="legs-a">
      <!-- Outer leg pair: cols 1 and 8, rows 6-7 -->
      <rect x="1" y="6" width="1" height="2" />
      <rect x="8" y="6" width="1" height="2" />
    </g>
    <g class="legs-b">
      <!-- Inner leg pair: cols 3 and 6, rows 6-7 -->
      <rect x="3" y="6" width="1" height="2" />
      <rect x="6" y="6" width="1" height="2" />
    </g>
  </svg>
</template>

<style scoped>
.clawd-sprite {
  display: inline-block;
  fill: var(--accent);
  vertical-align: middle;
}
</style>
