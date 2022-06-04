import dts from "rollup-plugin-dts"
import esbuild from "rollup-plugin-esbuild"
import hashbang from "rollup-plugin-hashbang"

/** @type {(input?: Record<string, string> | string) => Array<import('rollup').RollupOptions>} */
export const createLibraryConfig = (input = "src/index.ts") => {
  const bundle = {
    input,
    external: (/** @type {string} */ id) => !/^[./]/.test(id),
  }

  return [
    {
      ...bundle,
      plugins: [hashbang(), esbuild()],
      output: [
        {
          dir: "dist",
          format: "esm",
          entryFileNames: "[name].js",
          sourcemap: true,
        },
        {
          dir: "dist",
          format: "cjs",
          entryFileNames: "[name].cjs",
          sourcemap: true,
        },
      ],
    },
    {
      ...bundle,
      plugins: [dts()],
      output: {
        dir: "dist",
        format: "es",
        entryFileNames: "[name].d.ts",
      },
    },
  ]
}
