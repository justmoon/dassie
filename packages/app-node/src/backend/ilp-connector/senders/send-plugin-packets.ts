import { createActor } from "@dassie/lib-reactive"

import {
  processPacketPrepare,
  processPacketResult,
} from "../../accounting/functions/process-interledger-packet"
import { ledgerStore } from "../../accounting/stores/ledger"
import { managePlugins } from "../../spsp-server/manage-plugins"
import { IlpType } from "../ilp-packet-codec"
import { PacketSender } from "../send-outgoing-packets"

export interface PluginDestinationInfo {
  type: "plugin"
  pluginId: number
  localIlpAddressPart: string
}

export const sendPluginPackets = () =>
  createActor((sig) => {
    const ledger = sig.use(ledgerStore)
    const pluginManager = sig.use(managePlugins)

    return {
      sendPrepare: ({
        pluginId,
        localIlpAddressPart,
        parsedPacket,
        serializedPacket,
        outgoingRequestId,
      }) => {
        if (parsedPacket.amount > 0n) {
          processPacketPrepare(
            ledger,
            "builtin/owner/spsp",
            parsedPacket,
            "outgoing"
          )
        }

        pluginManager.tell("submitPrepare", {
          pluginId,
          localIlpAddressPart,
          serializedPacket,
          outgoingRequestId,
        })
      },
      sendResult: ({
        prepare: { parsedPacket: preparePacket, incomingRequestId: requestId },
        parsedPacket,
        serializedPacket,
      }) => {
        if (preparePacket.amount > 0n) {
          processPacketResult(
            ledger,
            "builtin/owner/spsp",
            preparePacket,
            parsedPacket.type === IlpType.Fulfill ? "fulfill" : "reject"
          )
        }

        pluginManager.tell("submitResult", {
          requestId,
          serializedPacket,
        })
      },
    }
  }) satisfies PacketSender<"plugin">
