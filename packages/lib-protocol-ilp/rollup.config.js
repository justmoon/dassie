import { entrypoint } from "@dassie/meta-rollup-config"

const config = [
  ...entrypoint("index", {
    external: ["@dassie/lib-type-utils", "@dassie/lib-oer"],
  }),
]

export default config
