import type { InputConfig } from "../../../config"

export const generateNodeConfig = (index: number) => {
  const nodeId = `node${index + 1}`
  return {
    nodeId,
    config: {
      host: `${nodeId}.localhost`,
      port: 4000 + index,
      tlsCertFile: `local/ssl/${nodeId}.localhost.pem`,
      tlsKeyFile: `local/ssl/${nodeId}.localhost-key.pem`,
    },
  }
}

export interface NodeConfig {
  nodeId: string
  config: InputConfig
}

export const NODES: Array<NodeConfig> = [
  generateNodeConfig(0),
  generateNodeConfig(1),
  generateNodeConfig(2),
]
