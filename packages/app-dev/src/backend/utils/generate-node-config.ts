import pureRand from "pure-rand"

import assert from "node:assert"
import { resolve } from "node:path"

import { SessionToken } from "@dassie/app-node/src/backend/authentication/types/session-token"
import { BootstrapNodesConfig } from "@dassie/app-node/src/backend/config/environment-config"
import { getPublicKey } from "@dassie/app-node/src/backend/crypto/ed25519"
import { calculatePathHmac } from "@dassie/app-node/src/backend/crypto/utils/seed-paths"
import { calculateNodeId } from "@dassie/app-node/src/backend/ilp-connector/utils/calculate-node-id"
import { derPreamble } from "@dassie/app-node/src/backend/utils/pem"
import {
  SEED_PATH_DEV_SESSION,
  SEED_PATH_NODE,
} from "@dassie/app-node/src/common/constants/seed-paths"

import { NODES_DEBUG_START_PORT, NODES_START_PORT } from "../constants/ports"
import { TEST_NODE_VANITY_SEEDS } from "../constants/vanity-nodes"
import type { EnvironmentSettings } from "../stores/environment-settings"
import { calculateHaltonLocation } from "./calculate-halton-location"

const ENTRYPOINT = new URL("../../runner/launchers/node", import.meta.url)
  .pathname
const LOCAL_PATH = new URL("../../../../../local", import.meta.url).pathname

const BOOTSTRAP_NODES = [0, 1]

export const nodeIndexToFriendlyId = (index: number) => `n${index + 1}`
export const nodeIndexToPort = (index: number) => NODES_START_PORT + index
export const nodeIndexToSeed = (index: number): Buffer => {
  const seedHex = TEST_NODE_VANITY_SEEDS[index]

  if (!seedHex) {
    throw new Error(`No vanity private key for node ${index} available`)
  }

  return Buffer.from(seedHex, "hex")
}
export const nodeIndexToPrivateKey = (index: number) => {
  const seed = nodeIndexToSeed(index)
  return calculatePathHmac(seed, SEED_PATH_NODE)
}

const PRIVATE_KEY_HEADER = "-----BEGIN PRIVATE KEY-----\n"
const PRIVATE_KEY_FOOTER = "\n-----END PRIVATE KEY-----\n"
const OPENSSL_ASCII_CHUNK_SIZE = 64

function splitStringIntoChunks(input: string, chunkSize: number): string[] {
  const chunks: string[] = []
  for (let index = 0; index < input.length; index += chunkSize) {
    chunks.push(input.slice(index, index + chunkSize))
  }
  return chunks
}
export const nodeIndexToPrivateKeyPem = (index: number) => {
  const key = nodeIndexToPrivateKey(index)
  return (
    PRIVATE_KEY_HEADER +
    splitStringIntoChunks(
      Buffer.concat([derPreamble, key]).toString("base64"),
      OPENSSL_ASCII_CHUNK_SIZE,
    ).join("\n") +
    PRIVATE_KEY_FOOTER
  )
}
export const nodeIndexToPublicKey = (index: number) => {
  const nodePrivateKey = nodeIndexToPrivateKey(index)
  return getPublicKey(nodePrivateKey)
}
export const nodeIndexToId = (index: number) => {
  const nodePublicKey = nodeIndexToPublicKey(index)
  return calculateNodeId(nodePublicKey)
}
export const nodeIndexToCoordinates = (index: number) =>
  calculateHaltonLocation(index + 1)

export const nodeFriendlyIdToIndex = (id: string) => {
  const index = Number.parseInt(id.slice(1), 10) - 1

  assert(`n${index + 1}` === id, `Invalid node id: ${id}`)

  return index
}
export const nodeIndexToSessionToken = (index: number) => {
  const seed = nodeIndexToSeed(index)
  return calculatePathHmac(seed, SEED_PATH_DEV_SESSION).toString(
    "hex",
  ) as SessionToken
}

export const nodeIndexToDataPath = (index: number) => {
  const friendlyId = nodeIndexToFriendlyId(index)

  return `${LOCAL_PATH}/data/${friendlyId}.localhost`
}

export interface BaseNodeConfig {
  id: string
  hostname: string
  httpsPort: number
  debugPort: number
  peers: readonly number[]
  latitude: number
  longitude: number
  dataPath: string
  ipcSocketPath: string
  dassieNodeKey: string
  tlsWebCertFile: string
  tlsWebKeyFile: string
  sessionToken: SessionToken
  bootstrapNodes: BootstrapNodesConfig
  url: string
  entry: string
}

export type NodeConfig = ReturnType<typeof generateNodeConfig>

const MAX_PEERS = 3
const PEER_SELECTION_BASE_SEED = 0xa3_e5_ef_27

const selectPeers = (nodeIndex: number) => {
  const generator = pureRand.xoroshiro128plus(
    PEER_SELECTION_BASE_SEED + nodeIndex,
  )

  const peers = new Set<number>()

  if (nodeIndex === 0) {
    return []
  }

  const peerCount = pureRand.unsafeUniformIntDistribution(
    1,
    MAX_PEERS,
    generator,
  )

  for (let index = 0; index < peerCount; index++) {
    const peerIndex = pureRand.unsafeUniformIntDistribution(
      0,
      nodeIndex - 1,
      generator,
    )
    peers.add(peerIndex)
  }

  return [...peers]
}

export const generatePeerInfo = (peerIndex: number) => ({
  nodeId: nodeIndexToId(peerIndex),
  url: `https://${nodeIndexToFriendlyId(peerIndex)}.localhost:${nodeIndexToPort(
    peerIndex,
  )}`,
  alias: nodeIndexToFriendlyId(peerIndex),
  nodePublicKey: nodeIndexToPublicKey(peerIndex),
})

export const generateNodeConfig = ((id, environmentSettings) => {
  const index = nodeFriendlyIdToIndex(id)
  const port = nodeIndexToPort(index)
  const { latitude, longitude } = nodeIndexToCoordinates(index)
  const peers = selectPeers(index)
  const dataPath = nodeIndexToDataPath(index)

  return {
    id,
    hostname: `${id}.localhost`,
    httpsPort: port,
    debugPort: NODES_DEBUG_START_PORT + index,
    peers: environmentSettings.peeringMode === "fixed" ? peers : [],
    latitude,
    longitude,
    dataPath,
    ipcSocketPath: resolve(dataPath, "dassie.sock"),
    dassieNodeKey: nodeIndexToPrivateKeyPem(index),
    tlsWebCertFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost.pem`,
    tlsWebKeyFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost-key.pem`,
    sessionToken: nodeIndexToSessionToken(index),
    bootstrapNodes: BOOTSTRAP_NODES.map((peerIndex) =>
      generatePeerInfo(peerIndex),
    ).map((peerInfo) => ({
      ...peerInfo,
      nodePublicKey: Buffer.from(peerInfo.nodePublicKey).toString("base64url"),
    })),
    url: `https://${id}.localhost:${port}/`,
    entry: ENTRYPOINT,
  } as const
}) satisfies (
  id: string,
  environmentSettings: EnvironmentSettings,
) => BaseNodeConfig
