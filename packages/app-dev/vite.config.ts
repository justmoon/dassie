import { presetIcons, presetUno } from "unocss"
import unocssPlugin from "unocss/vite"
import { defineConfig } from "vite"
import solidPlugin from "vite-plugin-solid"

export default defineConfig({
  plugins: [
    solidPlugin(),
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
    polyfillDynamicImport: false,
  },
})
