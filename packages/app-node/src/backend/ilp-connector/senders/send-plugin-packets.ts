import { createActor } from "@dassie/lib-reactive"

import { managePlugins } from "../../spsp-server/manage-plugins"
import { PacketSender } from "../functions/send-packet"

export interface PluginDestinationInfo {
  type: "plugin"
  pluginId: number
  localIlpAddressPart: string
}

export const sendPluginPackets = () =>
  createActor((sig) => {
    const pluginManager = sig.use(managePlugins)

    return {
      sendPrepare: ({
        pluginId,
        localIlpAddressPart,
        serializedPacket,
        outgoingRequestId,
      }) => {
        pluginManager.tell("submitPrepare", {
          pluginId,
          localIlpAddressPart,
          serializedPacket,
          outgoingRequestId,
        })
      },
      sendResult: ({
        prepare: { incomingRequestId: requestId },
        serializedPacket,
      }) => {
        pluginManager.tell("submitResult", {
          requestId,
          serializedPacket,
        })
      },
    }
  }) satisfies PacketSender<"plugin">
