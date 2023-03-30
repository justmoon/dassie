import { hexToBytes } from "@noble/hashes/utils"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { sendPeerMessage } from "./actions/send-peer-message"
import { peersComputation } from "./computed/peers"
import { signedPeerNodeInfo } from "./peer-schema"
import type { PerSubnetParameters } from "./run-per-subnet-effects"
import { nodeTableStore } from "./stores/node-table"

const PEERING_CHECK_INTERVAL = 1000

const MINIMUM_PEERS = 2

const logger = createLogger(
  "das:app-node:peer-protocol:maintain-peering-relationships"
)

export const maintainPeeringRelationships = () =>
  createActor((sig, { subnetId }: PerSubnetParameters) => {
    const subnetConfig = sig.get(configSignal, (state) =>
      state.initialSubnets.find(({ id }) => id === subnetId)
    )
    const { nodeId: ownNodeId } = sig.getKeys(configSignal, ["nodeId"])

    if (!subnetConfig) {
      throw new Error(`Subnet '${subnetId}' is not configured`)
    }

    const candidates = subnetConfig.bootstrapNodes.filter(
      ({ nodeId }) => nodeId !== ownNodeId
    )

    const nodes = sig.use(nodeTableStore).read()

    for (const candidate of candidates) {
      const node = nodes.get(`${subnetId}.${candidate.nodeId}`)

      if (!node) {
        sig.use(nodeTableStore).addNode({
          nodeId: candidate.nodeId,
          subnetId,
          url: candidate.url,
          nodePublicKey: hexToBytes(candidate.nodePublicKey),
          linkState: {
            sequence: 0n,
            lastUpdate: undefined,
            updateReceivedCounter: 0,
            scheduledRetransmitTime: 0,
            neighbors: [],
          },
          peerState: { id: "none" },
        })
      }
    }

    const { reactor } = sig
    const checkPeers = async () => {
      try {
        const peersSet = sig.use(peersComputation).read()
        if (peersSet.size >= MINIMUM_PEERS) {
          return
        }

        await discoverPeer()
      } catch (error) {
        logger.error("peer check failed", { error })
      }

      setTimeout(() => void checkPeers(), PEERING_CHECK_INTERVAL)
    }

    const discoverPeer = async () => {
      const randomCandidate =
        candidates[Math.floor(Math.random() * candidates.length)]

      if (!randomCandidate) {
        logger.debug("no valid candidates")
        return
      }

      // Send a peer discovery request
      const linkState = await queryLinkState(
        randomCandidate.nodeId,
        randomCandidate.nodeId
      )

      const peersSet = sig.use(peersComputation).read()

      const candidateNodeIds = new Set(
        [
          ...candidates.map(({ nodeId }) => nodeId),
          ...linkState.entries
            // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
            .filter(({ type }) => type === "neighbor")
            .map(({ value }) => value.nodeId)
            .filter((nodeId) => nodeId !== ownNodeId),
        ].filter((nodeId) => !peersSet.has(`${subnetId}.${nodeId}`))
      )

      if (candidateNodeIds.size === 0) {
        return
      }

      const randomCandidateNodeId = [...candidateNodeIds][
        Math.floor(Math.random() * candidateNodeIds.size)
      ]

      if (!randomCandidateNodeId) {
        return
      }

      const candidateLinkState = await queryLinkState(
        randomCandidate.nodeId,
        randomCandidateNodeId
      )

      reactor
        .use(nodeTableStore)
        .updateNode(`${subnetId}.${candidateLinkState.nodeId}`, {
          peerState: { id: "request-peering", lastSeen: Date.now() },
        })
    }

    const queryLinkState = async (
      oracleNodeId: string,
      subjectNodeId: string
    ) => {
      const response = await reactor.use(sendPeerMessage).ask({
        subnet: subnetId,
        destination: oracleNodeId,
        message: {
          type: "linkStateRequest",
          value: {
            subnetId,
            nodeId: subjectNodeId,
          },
        },
      })

      if (!response?.length) {
        throw new Error("no/invalid response")
      }

      const discoveryResponseParseResult = signedPeerNodeInfo.parse(response)

      if (!discoveryResponseParseResult.success) {
        throw new Error("invalid response")
      }

      const { signed } = discoveryResponseParseResult.value

      return signed
    }

    void checkPeers()
  })
