import path from "node:path"

import { SessionToken } from "@dassie/app-node/src/backend/authentication/types/session-token"
import { BootstrapNodesConfig } from "@dassie/app-node/src/backend/config/environment-config"
import { getPublicKey } from "@dassie/app-node/src/backend/crypto/ed25519"
import { calculatePathHmac } from "@dassie/app-node/src/backend/crypto/utils/seed-paths"
import { calculateNodeId } from "@dassie/app-node/src/backend/ilp-connector/utils/calculate-node-id"
import type { SettlementSchemeId } from "@dassie/app-node/src/backend/peer-protocol/types/settlement-scheme-id"
import {
  SEED_PATH_DEV_SESSION,
  SEED_PATH_NODE,
} from "@dassie/app-node/src/common/constants/seed-paths"
import { assert } from "@dassie/lib-logger"

import { NODE_ENTRYPOINT } from "../constants/entrypoints"
import { NODES_DEBUG_START_PORT, NODES_START_PORT } from "../constants/ports"
import { TEST_NODE_VANITY_SEEDS } from "../constants/vanity-nodes"
import { setup as logger } from "../logger/instances"
import type { EnvironmentSettings } from "../stores/environment"
import { calculateHaltonLocation } from "./calculate-halton-location"

const LOCAL_PATH = new URL("../../../../../local", import.meta.url).pathname

const BOOTSTRAP_NODES = [0, 1]

export const nodeIndexToFriendlyId = (index: number) => `d${index + 1}`
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

  assert(logger, `d${index + 1}` === id, `Invalid node id: ${id}`)

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
export const nodeIndexToUrl = (index: number) =>
  `https://${nodeIndexToFriendlyId(index)}.localhost:${nodeIndexToPort(index)}`

export interface BaseNodeConfig {
  index: number
  id: string
  hostname: string
  httpsPort: number
  debugPort: number
  peers: readonly PeerSettings[]
  latitude: number
  longitude: number
  dataPath: string
  ipcSocketPath: string
  dassieNodeKey: Uint8Array
  tlsWebCertFile: string
  tlsWebKeyFile: string
  sessionToken: SessionToken
  bootstrapNodes: BootstrapNodesConfig
  settlementMethods: readonly string[]
  url: string
  entry: string
}

export interface PeerSettings {
  readonly index: number
  readonly settlement: {
    readonly settlementSchemeId: SettlementSchemeId
    readonly settlementSchemeState: object
  }
}

export interface NodeSettings {
  readonly peers?: readonly PeerSettings[]
  readonly settlementMethods?: readonly string[]
}

export type NodeConfig = ReturnType<typeof generateNodeConfig>

const DEFAULT_NODE_SETTINGS: Required<NodeSettings> = {
  peers: [],
  settlementMethods: ["stub"],
}

export const generatePeerInfo = ({ index, settlement }: PeerSettings) => ({
  index,
  nodeId: nodeIndexToId(index),
  url: nodeIndexToUrl(index),
  alias: nodeIndexToFriendlyId(index),
  nodePublicKey: nodeIndexToPublicKey(index),
  settlement,
})

export const generateNodeConfig = ((
  index,
  nodeSettings,
  environmentSettings,
) => {
  const completeNodeSettings: Required<NodeSettings> = {
    ...DEFAULT_NODE_SETTINGS,
    ...environmentSettings.defaultNodeSettings,
    ...nodeSettings,
  }
  const id = nodeIndexToFriendlyId(index)
  const port = nodeIndexToPort(index)
  const { latitude, longitude } = nodeIndexToCoordinates(index)
  const dataPath = nodeIndexToDataPath(index)

  return {
    index,
    id,
    hostname: `${id}.localhost`,
    httpsPort: port,
    debugPort: NODES_DEBUG_START_PORT + index,
    peers: completeNodeSettings.peers,
    latitude,
    longitude,
    dataPath,
    ipcSocketPath: path.resolve(dataPath, "dassie.sock"),
    dassieNodeKey: nodeIndexToPrivateKey(index),
    tlsWebCertFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost.pem`,
    tlsWebKeyFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost-key.pem`,
    sessionToken: nodeIndexToSessionToken(index),
    bootstrapNodes: BOOTSTRAP_NODES.map((index) => {
      return {
        id: nodeIndexToId(index),
        url: nodeIndexToUrl(index),
        publicKey: Buffer.from(nodeIndexToPublicKey(index)).toString(
          "base64url",
        ),
      }
    }),
    settlementMethods: completeNodeSettings.settlementMethods,
    url: nodeIndexToUrl(index),
    entry: NODE_ENTRYPOINT,
  } as const
}) satisfies (
  index: number,
  nodeConfig: NodeSettings,
  environmentSettings: EnvironmentSettings,
) => BaseNodeConfig
