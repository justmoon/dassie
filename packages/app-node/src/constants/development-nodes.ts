import type { NodeDefinition } from "@xen-ilp/lib-dev-server"

import type { InputConfig } from "../config"

const LOCAL_PATH = new URL("../../../../local", import.meta.url).pathname

export const generateNodeConfig = (index: number) => {
  const id = `node${index + 1}`
  return {
    id,
    config: {
      host: `${id}.localhost`,
      port: 4000 + index,
      tlsXenCertFile: `${LOCAL_PATH}/ssl/${id}.localhost/xen-${id}.localhost.pem`,
      tlsXenKeyFile: `${LOCAL_PATH}/ssl/${id}.localhost/xen-${id}.localhost-key.pem`,
      tlsWebCertFile: `${LOCAL_PATH}/ssl/${id}.localhost/web-${id}.localhost.pem`,
      tlsWebKeyFile: `${LOCAL_PATH}/ssl/${id}.localhost/web-${id}.localhost-key.pem`,
    },
    url: `https://${id}.localhost:${4000 + index}/`,
  }
}

export const NODES: Array<NodeDefinition<InputConfig>> = [
  generateNodeConfig(0),
  generateNodeConfig(1),
  generateNodeConfig(2),
]
