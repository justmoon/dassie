import { presetIcons, presetUno } from "unocss"
import unocssPlugin from "unocss/vite"
import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"

export default defineConfig({
  plugins: [
    react(),
    unocssPlugin({
      presets: [
        presetUno(),
        presetIcons({
          collections: {
            mdi: () =>
              import("@iconify-json/mdi/icons.json", { assert: { type: 'json' } }).then(
                (module) => module.default
              ),
          },
        }),
      ],
    }),
  ],
  build: {
    target: "esnext",
  },
})
