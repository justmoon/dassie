import react from "@vitejs/plugin-react"
import tailwind from "tailwindcss"
import { defineConfig } from "vite"

import tailwindConfig from "./tailwind.config.js"

export default defineConfig({
  plugins: [react()],
  css: {
    postcss: {
      plugins: [tailwind({ config: tailwindConfig })],
    },
  },
  build: {
    target: "esnext",
  },
})
