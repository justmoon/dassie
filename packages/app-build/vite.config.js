import { defineConfig } from "vite"

export default defineConfig({
  plugins: [],
  server: {
    hmr: true,
  },
  build: {
    target: "esnext",
  },
})
