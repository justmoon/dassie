import react from "@vitejs/plugin-react"
import unocss from "unocss/vite"
import { defineConfig } from "vite"

export default defineConfig({
  plugins: [
    react(),
    unocss({
      configFile: "./uno.config.ts",
    }),
  ],
  build: {
    target: "esnext",
  },
})
