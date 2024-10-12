import { nodeResolve } from "@rollup/plugin-node-resolve"
import { dts } from "rollup-plugin-dts"
import esbuild from "rollup-plugin-esbuild"

export function entrypoint(
  name = "index",
  { input = `./src/${name}.ts`, external = [], externalTypes = [] } = {},
) {
  return [
    {
      input,
      output: [
        {
          file: `dist/${name}.js`,
          format: "es",
          sourcemap: true,
        },
      ],
      external,
      plugins: [esbuild(), nodeResolve()],
    },
    {
      input: `./dist/src/${name}.d.ts`,
      output: [{ file: `dist/${name}.d.ts`, format: "es" }],
      external: [...external, ...externalTypes],
      plugins: [dts()],
    },
  ]
}
