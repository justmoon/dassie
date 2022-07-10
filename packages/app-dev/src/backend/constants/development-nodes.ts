import type { InputConfig } from "@xen-ilp/app-node"

import type { NodeDefinition } from "../effects/run-nodes"

const ENTRYPOINT = new URL("../../runner/launchers/node", import.meta.url)
  .pathname
const LOCAL_PATH = new URL("../../../../../local", import.meta.url).pathname

export const PEERS: [source: number, target: number][] = [
  [1, 0],
  [2, 0],
  [2, 1],
]

export const nodeIndexToId = (index: number) => `n${index + 1}`
export const nodeIndexToPort = (index: number) => 4000 + index

export const generateNodeConfig = (index: number) => {
  const id = nodeIndexToId(index)
  const peers = PEERS.filter(([source]) => source === index)

  return {
    id,
    port: nodeIndexToPort(index),
    debugPort: nodeIndexToPort(index) + 1000,
    peers: peers.map(([, index]) => nodeIndexToId(index)),
    config: {
      nodeId: id,
      host: `${id}.localhost`,
      port: nodeIndexToPort(index),
      tlsXenCertFile: `${LOCAL_PATH}/ssl/${id}.localhost/xen-${id}.localhost.pem`,
      tlsXenKeyFile: `${LOCAL_PATH}/ssl/${id}.localhost/xen-${id}.localhost-key.pem`,
      tlsWebCertFile: `${LOCAL_PATH}/ssl/${id}.localhost/web-${id}.localhost.pem`,
      tlsWebKeyFile: `${LOCAL_PATH}/ssl/${id}.localhost/web-${id}.localhost-key.pem`,
      initialPeers: peers
        .map(
          ([, target]) =>
            `${nodeIndexToId(target)}=https://${nodeIndexToId(
              target
            )}.localhost:${nodeIndexToPort(target)}`
        )
        .join(";"),
    },
    url: `https://${id}.localhost:${4000 + index}/`,
    entry: ENTRYPOINT,
  }
}

export const NODES: NodeDefinition<InputConfig>[] = [
  generateNodeConfig(0),
  generateNodeConfig(1),
  generateNodeConfig(2),
]
