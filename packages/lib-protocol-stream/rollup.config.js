import { entrypoint } from "@dassie/meta-rollup-config"

const config = [
  ...entrypoint("index", {
    external: [
      "@dassie/lib-logger",
      "@dassie/lib-protocol-ildcp",
      "@dassie/lib-protocol-ilp",
      "@dassie/lib-type-utils",
      "@dassie/lib-oer",
      "@dassie/lib-reactive",
    ],
  }),
]

export default config
