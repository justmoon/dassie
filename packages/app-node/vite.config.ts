import { defineConfig } from "vite"

export default defineConfig({
  build: {
    target: "node16",
    polyfillDynamicImport: false,
    lib: {
      entry: new URL("src/index.ts", import.meta.url).pathname,
      formats: ["es"],
    },
  },
})
