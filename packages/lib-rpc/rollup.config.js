import { entrypoint } from "@dassie/meta-rollup-config"

const config = [
  ...entrypoint("server/index", {
    external: ["zod", "@dassie/lib-type-utils"],
  }),
  ...entrypoint("client/index", {
    external: ["zod", "@dassie/lib-type-utils"],
  }),
]

export default config
