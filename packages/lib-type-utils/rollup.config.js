import { nodeResolve } from "@rollup/plugin-node-resolve"
import { dts } from "rollup-plugin-dts"
import esbuild from "rollup-plugin-esbuild"

const config = [
  {
    input: "./src/index.ts",
    output: [
      {
        file: `dist/index.js`,
        format: "es",
        sourcemap: true,
      },
    ],
    external: ["@dassie/lib-type-utils"],
    plugins: [esbuild(), nodeResolve()],
  },
  {
    input: "./dist/src/index.d.ts",
    output: [{ file: "dist/index.d.ts", format: "es" }],
    plugins: [dts()],
  },
]

export default config
