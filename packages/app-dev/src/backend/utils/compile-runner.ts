import { rollup } from "rollup"
import type { RollupOptions } from "rollup"
import esbuild from "rollup-plugin-esbuild"

import { createLogger } from "@dassie/lib-logger"

const logger = createLogger("das:dev:compile-runner")

const rollupConfig: RollupOptions = {
  input: new URL("../../runner/runner.ts", import.meta.url).pathname,
  external: (id: string) => !/^[./]/.test(id),
  plugins: [
    esbuild({
      target: "esnext",
    }),
  ],
  output: [
    {
      dir: "packages/app-dev/dist",
      format: "esm",
      entryFileNames: "[name].js",
      sourcemap: true,
    },
  ],
}

export const compileRunner = async () => {
  let bundle
  try {
    bundle = await rollup(rollupConfig)

    if (!rollupConfig.output) {
      logger.error('rollup config has no "output" property')
      return
    }

    for (const outputOptions of [rollupConfig.output].flat()) {
      await bundle.write(outputOptions)
    }
  } catch (error) {
    logger.error("error while compiling the runner entrypoint", { error })
  } finally {
    if (bundle) {
      await bundle.close()
    }
  }
}
