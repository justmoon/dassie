import { castImmutable, enableMapSet, produce } from "immer"

import type { IlpEndpoint } from "@dassie/lib-protocol-ilp"
import { createStore } from "@dassie/lib-reactive"

import type { LocalEndpointInfo } from "../../ilp-connector/senders/send-local-packets"

enableMapSet()

interface LocalEndpointInfoWithCallback extends LocalEndpointInfo {
  readonly prepareCallback?: IlpEndpoint["sendPacket"] | undefined
}

export const LocalEndpointsStore = () =>
  createStore(
    castImmutable(new Map<string, LocalEndpointInfoWithCallback>()),
  ).actions({
    registerLocalEndpoint: (entry: LocalEndpointInfo) =>
      produce((draft) => {
        draft.set(entry.localIlpAddressPart, entry)
      }),
    unregisterLocalEndpoint: (localAddressPart: string) =>
      produce((draft) => {
        draft.delete(localAddressPart)
      }),
    setHandler: (
      localAddressPart: string,
      handler: IlpEndpoint["sendPacket"] | undefined,
    ) =>
      produce((draft) => {
        const endpoint = draft.get(localAddressPart)
        if (!endpoint) return

        endpoint.prepareCallback = handler
      }),
  })
