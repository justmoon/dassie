import type { DassieReactor } from "../../base/types/dassie-base"
import { ManagePluginsActor } from "../../open-payments/manage-plugins"
import type { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface PluginEndpointInfo extends CommonEndpointInfo {
  readonly type: "plugin"
  readonly pluginId: number
  readonly localIlpAddressPart: string
}

export const SendPluginPackets = (
  reactor: DassieReactor,
): PacketSender<"plugin"> => {
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
