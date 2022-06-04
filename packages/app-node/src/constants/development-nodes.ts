import type { NodeDefinition } from "@xen-ilp/lib-dev-server"

import type { InputConfig } from "../config"

export const generateNodeConfig = (index: number) => {
  const id = `node${index + 1}`
  return {
    id,
    config: {
      host: `${id}.localhost`,
      port: 4000 + index,
      tlsCertFile: `../../local/ssl/${id}.localhost.pem`,
      tlsKeyFile: `../../local/ssl/${id}.localhost-key.pem`,
    },
    url: `https://${id}.localhost:${4000 + index}/`,
  }
}

export const NODES: Array<NodeDefinition<InputConfig>> = [
  generateNodeConfig(0),
  generateNodeConfig(1),
  generateNodeConfig(2),
]
