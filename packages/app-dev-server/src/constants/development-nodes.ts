import type { InputConfig } from "../../../app-node/src/config"
import type { NodeDefinition } from "../server"

const ENTRYPOINT = "@xen-ilp/app-node"
const LOCAL_PATH = new URL("../../../../local", import.meta.url).pathname

export const PEERS: Array<[number, number]> = [
  [1, 0],
  [2, 0],
  [2, 1],
]

export const nodeIndexToId = (index: number) => `n${index + 1}`
export const nodeIndexToPort = (index: number) => 4000 + index

export const generateNodeConfig = (index: number) => {
  const id = nodeIndexToId(index)
  return {
    id,
    config: {
      nodeId: id,
      host: `${id}.localhost`,
      port: nodeIndexToPort(index),
      tlsXenCertFile: `${LOCAL_PATH}/ssl/${id}.localhost/xen-${id}.localhost.pem`,
      tlsXenKeyFile: `${LOCAL_PATH}/ssl/${id}.localhost/xen-${id}.localhost-key.pem`,
      tlsWebCertFile: `${LOCAL_PATH}/ssl/${id}.localhost/web-${id}.localhost.pem`,
      tlsWebKeyFile: `${LOCAL_PATH}/ssl/${id}.localhost/web-${id}.localhost-key.pem`,
      initialPeers: PEERS.filter(([peerIndex]) => peerIndex === index)
        .map(
          ([, b]) =>
            `${nodeIndexToId(b)}=https://${nodeIndexToId(
              b
            )}.localhost:${nodeIndexToPort(b)}`
        )
        .join(";"),
    },
    url: `https://${id}.localhost:${4000 + index}/`,
    entry: ENTRYPOINT,
  }
}

export const NODES: Array<NodeDefinition<InputConfig>> = [
  generateNodeConfig(0),
  generateNodeConfig(1),
  generateNodeConfig(2),
]
