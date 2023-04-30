import type { InputConfig } from "@dassie/app-node"
import { getPublicKey } from "@dassie/app-node/src/backend/crypto/ed25519"
import { calculateNodeId } from "@dassie/app-node/src/backend/ilp-connector/utils/calculate-node-id"

import { TEST_NODE_VANITY_KEYS } from "../constants/node-keys"
import {
  DEBUG_UI_PORT,
  NODES_DEBUG_START_PORT,
  NODES_START_PORT,
} from "../constants/ports"
import type { EnvironmentSettings } from "../stores/environment-settings"
import { calculateHaltonLocation } from "./calculate-halton-location"

const ENTRYPOINT = new URL("../../runner/launchers/node", import.meta.url)
  .pathname
const LOCAL_PATH = new URL("../../../../../local", import.meta.url).pathname

const BOOTSTRAP_NODES = [0, 1]

export const nodeIndexToFriendlyId = (index: number) => `n${index + 1}`
export const nodeIndexToPort = (index: number) => NODES_START_PORT + index
export const nodeIndexToPublicKey = (index: number) => {
  const nodePrivateKey = TEST_NODE_VANITY_KEYS[index]

  if (!nodePrivateKey) {
    throw new Error(`No private key for node ${index}`)
  }

  return getPublicKey(nodePrivateKey)
}
export const nodeIndexToId = (index: number) => {
  const nodePublicKey = nodeIndexToPublicKey(index)
  const nodeId = calculateNodeId(nodePublicKey)

  return nodeId
}
export const nodeIndexToCoordinates = (index: number) =>
  calculateHaltonLocation(index + 1)

export interface NodeConfig {
  id: string
  port: number
  debugPort: number
  peers: string[]
  peerIndices: readonly number[]
  latitude: number
  longitude: number
  config: InputConfig
  url: string
  entry: string
}

const generatePeerInfo = (peerIndex: number) => ({
  nodeId: nodeIndexToId(peerIndex),
  url: `https://${nodeIndexToFriendlyId(peerIndex)}.localhost:${nodeIndexToPort(
    peerIndex
  )}`,
  alias: nodeIndexToFriendlyId(peerIndex),
  nodePublicKey: Buffer.from(nodeIndexToPublicKey(peerIndex)).toString("hex"),
})

export const generateNodeConfig = ((index, peers, environmentSettings) => {
  const id = nodeIndexToFriendlyId(index)
  const port = nodeIndexToPort(index)
  const { latitude, longitude } = nodeIndexToCoordinates(index)

  const peersInfo = peers.map((peerIndex) => generatePeerInfo(peerIndex))

  return {
    id,
    port,
    debugPort: NODES_DEBUG_START_PORT + index,
    peers: peers.map((index) => nodeIndexToFriendlyId(index)),
    peerIndices: peers,
    latitude,
    longitude,
    config: {
      realm: "test" as const,
      host: `${id}.localhost`,
      port: nodeIndexToPort(index),
      alias: id,
      dataPath: `${LOCAL_PATH}/data/${id}.localhost`,
      tlsDassieCertFile: `${LOCAL_PATH}/tls/${id}.localhost/dassie-${id}.localhost.pem`,
      tlsDassieKeyFile: `${LOCAL_PATH}/tls/${id}.localhost/dassie-${id}.localhost-key.pem`,
      tlsWebCertFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost.pem`,
      tlsWebKeyFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost-key.pem`,
      initialSubnets: [
        {
          id: "stub",
          config: {},
          bootstrapNodes: BOOTSTRAP_NODES.map((peerIndex) =>
            generatePeerInfo(peerIndex)
          ),
          initialPeers:
            environmentSettings.peeringMode === "fixed" ? peersInfo : undefined,
        },
      ],
      exchangeRateUrl: `https://localhost:${DEBUG_UI_PORT}/rates.json`,
    },
    url: `https://${id}.localhost:${port}/`,
    entry: ENTRYPOINT,
  }
}) satisfies (
  index: number,
  peers: readonly number[],
  environmentSettings: EnvironmentSettings
) => NodeConfig
