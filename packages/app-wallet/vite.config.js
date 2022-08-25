import react from "@vitejs/plugin-react"
import { presetUno } from "unocss"
import unocssPlugin from "unocss/vite"
import { defineConfig } from "vite"

export default defineConfig({
  plugins: [
    react(),
    unocssPlugin({
      presets: [presetUno()],
    }),
  ],
  build: {
    target: "esnext",
  },
})
