import { createActor } from "@dassie/lib-reactive"

import { ManagePluginsActor } from "../../spsp-server/manage-plugins"
import { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface PluginEndpointInfo extends CommonEndpointInfo {
  readonly type: "plugin"
  readonly pluginId: number
  readonly localIlpAddressPart: string
}

export const SendPluginPacketsActor = () =>
  createActor((sig) => {
    const pluginManager = sig.use(ManagePluginsActor)

    return sig.handlers({
      sendPrepare: ({
        pluginId,
        localIlpAddressPart,
        serializedPacket,
        outgoingRequestId,
      }) => {
        pluginManager.api.submitPrepare.tell({
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
        pluginManager.api.submitResult.tell({
          requestId,
          serializedPacket,
        })
      },
    })
  }) satisfies PacketSender<"plugin">
