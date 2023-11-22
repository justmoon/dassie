import { Reactor } from "@dassie/lib-reactive"

import { ManagePluginsActor } from "../../spsp-server/manage-plugins"
import { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface PluginEndpointInfo extends CommonEndpointInfo {
  readonly type: "plugin"
  readonly pluginId: number
  readonly localIlpAddressPart: string
}

export const SendPluginPackets = (reactor: Reactor): PacketSender<"plugin"> => {
  const pluginManager = reactor.use(ManagePluginsActor)

  return {
    sendPrepare: ({
      destinationEndpointInfo: { pluginId, localIlpAddressPart },
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
  }
}
