import react from "@vitejs/plugin-react"
import { presetUno } from "unocss"
import unocssPlugin from "unocss/vite"
import { defineConfig } from "vite"

const radixVariantRegex = /^radix-state-(active|inactive):/

export default defineConfig({
  plugins: [
    react(),
    unocssPlugin({
      presets: [presetUno()],
      variants: [
        // Radix state variants
        (matcher) => {
          const match = matcher.match(radixVariantRegex)

          if (!match) return matcher

          const [fullMatch, state] = match

          if (!fullMatch || !state) return matcher

          return {
            matcher: matcher.slice(fullMatch.length),
            selector: (s) => `${s}[data-state="${state}"]`,
          }
        },
      ],
    }),
  ],
  build: {
    target: "esnext",
  },
})
