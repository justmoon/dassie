import react from "@vitejs/plugin-react"
import unocssPlugin from "unocss/vite"
import { defineConfig } from "vite"

export default defineConfig({
  plugins: [
    react(),
    unocssPlugin({
      configFile: "./uno.config.ts",
    }),
  ],
  build: {
    target: "esnext",
  },
})
