import { entrypoint } from "@dassie/meta-rollup-config"

const config = [
  ...entrypoint("server/index", {
    external: [
      "@dassie/lib-reactive",
      "@dassie/lib-rpc/server",
      "@dassie/lib-type-utils",
    ],
  }),
  ...entrypoint("client/index", {
    external: [
      "react",
      "react/jsx-runtime",
      "@dassie/lib-reactive",
      "@dassie/lib-rpc/client",
      "@dassie/lib-rpc/server",
      "@dassie/lib-rpc-react",
      "@dassie/lib-type-utils",
      "@tanstack/react-query",
    ],
  }),
]

export default config
