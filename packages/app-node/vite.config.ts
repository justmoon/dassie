import { resolve } from "path"
import { defineConfig } from "vite"

export default defineConfig({
  build: {
    target: "node16",
    polyfillDynamicImport: false,
    lib: {
      entry: resolve(__dirname, "src/index.ts"),
      formats: ["es"],
    },
  },
})
