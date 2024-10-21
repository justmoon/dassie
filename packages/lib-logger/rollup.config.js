import { entrypoint } from "@dassie/meta-rollup-config"

const config = [
  ...entrypoint("browser", {
    external: ["@dassie/lib-type-utils", "@dassie/lib-reactive"],
  }),
  ...entrypoint("node", {
    external: ["@dassie/lib-type-utils", "@dassie/lib-reactive"],
  }),
]

export default config
