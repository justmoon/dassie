import esbuild from "rollup-plugin-esbuild"

// eslint-disable-next-line tsdoc/syntax
/** @type {(input?: Record<string, string> | string) => Array<import('rollup').RollupOptions>} */
export const createLibraryConfig = (input = "src/index.ts") => {
  const bundle = {
    input,
    // eslint-disable-next-line tsdoc/syntax
    external: (/** @type {string} */ id) => !/^[./]/.test(id),
  }

  return [
    {
      ...bundle,
      plugins: [esbuild()],
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
  ]
}
