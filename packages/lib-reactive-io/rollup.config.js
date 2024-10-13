import { entrypoint } from "@dassie/meta-rollup-config"

const config = [
  ...entrypoint("browser/index", {
    external: ["@dassie/lib-type-utils", "@dassie/lib-reactive"],
  }),
  ...entrypoint("node/index", {
    external: ["@dassie/lib-type-utils", "@dassie/lib-reactive"],
  }),
]

export default config
