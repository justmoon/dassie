import type { InputConfig } from "@dassie/app-node"

import { DEBUG_UI_PORT, NODES_DEBUG_START_PORT, NODES_START_PORT } from "../constants/ports"

const ENTRYPOINT = new URL("../../runner/launchers/node", import.meta.url)
  .pathname
const LOCAL_PATH = new URL("../../../../../local", import.meta.url).pathname

export const nodeIndexToId = (index: number) => `n${index + 1}`
export const nodeIndexToPort = (index: number) => NODES_START_PORT + index

interface NodeConfig {
  id: string
  port: number
  debugPort: number
  peers: string[]
  config: InputConfig
  url: string
  entry: string
}

// TODO: this is temporary hack while the prettier plugin doesn't support
// the satisfies operation introduced in TS 4.9
// See: https://github.com/trivago/prettier-plugin-sort-imports/issues/204
const satisfiesNodeConfig = <T extends NodeConfig>(result: T): T => {
  // satisfies (index: number, peers: readonly number[]) => NodeConfig
  return result
}

export const generateNodeConfig = (
  index: number,
  peers: readonly number[]
) => {
  const id = nodeIndexToId(index)
  const port = nodeIndexToPort(index)

  return satisfiesNodeConfig({
    id,
    port,
    debugPort: NODES_DEBUG_START_PORT + index,
    peers: peers.map((index) => nodeIndexToId(index)),
    config: {
      nodeId: id,
      realm: "test" as const,
      host: `${id}.localhost`,
      port: nodeIndexToPort(index),
      dataPath: `${LOCAL_PATH}/data/${id}.localhost`,
      tlsDassieCertFile: `${LOCAL_PATH}/tls/${id}.localhost/dassie-${id}.localhost.pem`,
      tlsDassieKeyFile: `${LOCAL_PATH}/tls/${id}.localhost/dassie-${id}.localhost-key.pem`,
      tlsWebCertFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost.pem`,
      tlsWebKeyFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost-key.pem`,
      initialSubnets: [
        {
          id: "stub",
          config: {},
          initialPeers: peers.map((target) => ({
            nodeId: nodeIndexToId(target),
            url: `https://${nodeIndexToId(target)}.localhost:${nodeIndexToPort(
              target
            )}`,
            nodePublicKey: "BEEF",
          })),
        },
      ],
      // TODO: Make this dynamic
      beacons: "https://b1.localhost:13001;https://b2.localhost:13002",
      exchangeRateUrl: `https://localhost:${DEBUG_UI_PORT}/rates.json`,
    },
    url: `https://${id}.localhost:${port}/`,
    entry: ENTRYPOINT,
  })
}
