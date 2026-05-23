import { defineConfig } from 'vitest/config'

// Vitest runs the axis-2 highlight engine tests. No browser env needed --
// the engine is pure functions over strings.
export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
})
