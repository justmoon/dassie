import pureRand from "pure-rand"

import assert from "node:assert"

import type { InputConfig } from "@dassie/app-node"
import { getPublicKey } from "@dassie/app-node/src/backend/crypto/ed25519"
import { calculateNodeId } from "@dassie/app-node/src/backend/ilp-connector/utils/calculate-node-id"
import { SubnetId } from "@dassie/app-node/src/backend/peer-protocol/types/subnet-id"

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

export const nodeFriendlyIdToIndex = (id: string) => {
  const index = Number.parseInt(id.slice(1), 10) - 1

  assert(`n${index + 1}` === id, `Invalid node id: ${id}`)

  return index
}

export interface BaseNodeConfig {
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

export type NodeConfig = ReturnType<typeof generateNodeConfig>

const MAX_PEERS = 3
const PEER_SELECTION_BASE_SEED = 0xa3_e5_ef_27

const selectPeers = (nodeIndex: number) => {
  const generator = pureRand.xoroshiro128plus(
    PEER_SELECTION_BASE_SEED + nodeIndex
  )

  const peers = new Set<number>()

  if (nodeIndex === 0) {
    return []
  }

  const peerCount = pureRand.unsafeUniformIntDistribution(
    1,
    MAX_PEERS,
    generator
  )

  for (let index = 0; index < peerCount; index++) {
    const peerIndex = pureRand.unsafeUniformIntDistribution(
      0,
      nodeIndex - 1,
      generator
    )
    peers.add(peerIndex)
  }

  return [...peers]
}

const generatePeerInfo = (peerIndex: number) => ({
  nodeId: nodeIndexToId(peerIndex),
  url: `https://${nodeIndexToFriendlyId(peerIndex)}.localhost:${nodeIndexToPort(
    peerIndex
  )}`,
  alias: nodeIndexToFriendlyId(peerIndex),
  nodePublicKey: Buffer.from(nodeIndexToPublicKey(peerIndex)).toString("hex"),
})

export const generateNodeConfig = ((id, environmentSettings) => {
  const index = nodeFriendlyIdToIndex(id)
  const port = nodeIndexToPort(index)
  const { latitude, longitude } = nodeIndexToCoordinates(index)
  const peers = selectPeers(index)

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
          id: "stub" as SubnetId,
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
  } as const
}) satisfies (
  id: string,
  environmentSettings: EnvironmentSettings
) => BaseNodeConfig
