<script setup lang="ts">
/**
 * Konami-code easter egg: pixel-art Clawd scuttles across the tab strip
 * and exits screen right. The parent owns the lifecycle entirely -- it
 * mounts this component (v-if) when the sequence fires, swaps the :key
 * to force a fresh mount on re-trigger, and listens for `done` to
 * unmount. The cameo itself just emits when its scuttle animation ends.
 *
 * The sprite lives in ClawdSprite.vue -- this component owns the
 * scuttle/bob/gait animations and the leg-alternation timing that turns
 * a static sprite into a believable skitter.
 */
import ClawdSprite from './ClawdSprite.vue'

const emit = defineEmits<{ (e: 'done'): void }>()

function onAnimationEnd(ev: AnimationEvent) {
  // Only the outer scuttle animation ends -- bob and leg loops are
  // infinite and never fire animationend. Filter by name so a stray
  // bubbled event from a child can't end the cameo early.
  if (ev.animationName !== 'clawd-scuttle') return
  emit('done')
}
</script>

<template>
  <div
    class="clawd-cameo"
    aria-hidden="true"
    @animationend="onAnimationEnd"
  >
    <ClawdSprite :pixel="4" />
  </div>
</template>

<style scoped>
.clawd-cameo {
  position: fixed;
  /* Sit on top of the tab strip (which sits directly under the ~2rem
     header). z-index keeps Clawd above tabs, banners and the viewport
     but below modals (which use higher stacking contexts). */
  top: 2rem;
  left: 0;
  width: 40px;
  height: 32px;
  pointer-events: none;
  z-index: 1000;
  animation: clawd-scuttle 2.6s linear forwards;
  /* Soft shadow keeps Clawd readable against any tab colour. */
  filter: drop-shadow(0 1px 0 rgba(0, 0, 0, 0.45));
}

.clawd-cameo :deep(.clawd-sprite) {
  /* Subtle vertical bob to sell the scuttle. Stepped so it feels
     pixel-art, not silky-smooth. */
  animation: clawd-bob 0.18s steps(2, end) infinite;
}

.clawd-cameo :deep(.legs-a) {
  animation: clawd-legs-a 0.18s steps(1, end) infinite;
}
.clawd-cameo :deep(.legs-b) {
  animation: clawd-legs-b 0.18s steps(1, end) infinite;
}

@keyframes clawd-scuttle {
  from { transform: translateX(-60px); }
  to   { transform: translateX(calc(100vw + 60px)); }
}

@keyframes clawd-bob {
  0%, 49.9%   { transform: translateY(0); }
  50%, 100%   { transform: translateY(-2px); }
}

@keyframes clawd-legs-a {
  0%, 49.9%   { opacity: 1; }
  50%, 100%   { opacity: 0; }
}

@keyframes clawd-legs-b {
  0%, 49.9%   { opacity: 0; }
  50%, 100%   { opacity: 1; }
}
</style>
