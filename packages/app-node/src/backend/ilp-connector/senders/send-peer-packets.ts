import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { nodeTableStore } from "../.."
import {
  processPacketPrepare,
  processPacketResult,
} from "../../accounting/functions/process-interledger-packet"
import { ledgerStore } from "../../accounting/stores/ledger"
import { sendPeerMessage } from "../../peer-protocol/actors/send-peer-message"
import { NodeId } from "../../peer-protocol/types/node-id"
import { IlpType } from "../ilp-packet-codec"
import { PacketSender } from "../send-outgoing-packets"

export interface PeerDestinationInfo {
  type: "peer"
  firstHopOptions: NodeId[]
}

const logger = createLogger("das:node:send-interledger-packets")

export const sendPeerPackets = () =>
  createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)
    const ledger = sig.use(ledgerStore)

    return {
      sendPrepare: ({
        firstHopOptions,
        parsedPacket,
        serializedPacket,
        outgoingRequestId: requestId,
      }) => {
        const nextHop = firstHopOptions[0]!

        const peerState = nodeTable.read().get(nextHop)?.peerState

        if (peerState?.id !== "peered") {
          throw new Error(`Next hop node is not actually peered ${nextHop}`)
        }

        if (parsedPacket.amount > 0n) {
          processPacketPrepare(
            ledger,
            `${peerState.subnetId}/peer/${nextHop}/interledger`,
            parsedPacket,
            "outgoing"
          )
        }

        logger.debug("sending ilp packet", { nextHop })

        sig.use(sendPeerMessage).tell("send", {
          destination: nextHop,
          message: {
            type: "interledgerPacket",
            value: {
              signed: {
                requestId,
                packet: serializedPacket,
              },
            },
          },
        })
      },
      sendResult: ({
        firstHopOptions,
        parsedPacket,
        serializedPacket,
        prepare: { parsedPacket: preparePacket, incomingRequestId: requestId },
      }) => {
        const nextHop = firstHopOptions[0]!

        const peerState = sig.use(nodeTableStore).read().get(nextHop)?.peerState

        if (peerState?.id !== "peered") {
          throw new Error(`Next hop node is not actually peered ${nextHop}`)
        }

        const ledger = sig.use(ledgerStore)
        if (preparePacket.amount > 0n) {
          processPacketResult(
            ledger,
            `${peerState.subnetId}/peer/${nextHop}/interledger`,
            preparePacket,
            parsedPacket.type === IlpType.Fulfill ? "fulfill" : "reject"
          )
        }

        logger.debug("sending ilp packet", { nextHop })

        sig.use(sendPeerMessage).tell("send", {
          destination: nextHop,
          message: {
            type: "interledgerPacket",
            value: {
              signed: {
                requestId: requestId,
                packet: serializedPacket,
              },
            },
          },
        })
      },
    }
  }) satisfies PacketSender<"peer">
