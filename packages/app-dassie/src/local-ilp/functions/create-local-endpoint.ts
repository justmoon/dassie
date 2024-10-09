import { nanoid } from "nanoid"

import {
  type IlpEndpoint,
  type IlpResponsePacket,
  IlpType,
  serializeIlpPacket,
} from "@dassie/lib-protocol-ilp"
import { type Scope, createDeferred } from "@dassie/lib-reactive"

import type { DassieReactor } from "../../base/types/dassie-base"
import { ProcessPacket } from "../../ilp-connector/functions/process-packet"
import type { LocalEndpointInfo } from "../../ilp-connector/senders/send-local-packets"
import { LocalEndpointsStore } from "../stores/local-endpoints"
import { OutstandingRequestsStore } from "../stores/outstanding-requests"

interface CreateLocalEndpointParameters {
  readonly scope: Scope
  readonly hint: string
}

export interface LocalIlpEndpoint extends IlpEndpoint {
  id: string
}

export const CreateLocalEndpoint = (reactor: DassieReactor) => {
  const localEndpointsStore = reactor.use(LocalEndpointsStore)
  const processPacket = reactor.use(ProcessPacket)
  const outstandingRequestsStore = reactor.use(OutstandingRequestsStore)

  return function createLocalEndpoint({
    scope,
    hint,
  }: CreateLocalEndpointParameters): LocalIlpEndpoint {
    let id: string
    do {
      id = nanoid()
    } while (localEndpointsStore.read().has(id))

    const localEndpointInfo: LocalEndpointInfo = {
      type: "local",
      hint,
      localIlpAddressPart: id,
    }

    localEndpointsStore.act.registerLocalEndpoint(localEndpointInfo)

    scope.onCleanup(() => {
      localEndpointsStore.act.unregisterLocalEndpoint(id)
    })

    return {
      id,
      sendPacket: (packet) => {
        let requestId: string
        do {
          requestId = nanoid()
        } while (outstandingRequestsStore.read().has(requestId))

        const deferred = createDeferred<IlpResponsePacket>()

        outstandingRequestsStore.act.addRequest(requestId, deferred.resolve)

        const parsedPacket = {
          type: IlpType.Prepare,
          data: packet,
        }
        processPacket({
          parsedPacket,
          serializedPacket: serializeIlpPacket(parsedPacket),
          requestId,
          sourceEndpointInfo: localEndpointInfo,
        })

        return deferred
      },
      handlePackets: (handler) => {
        if (localEndpointsStore.read().get(id)?.prepareCallback) {
          throw new Error("Local endpoint already has a handler")
        }
        localEndpointsStore.act.setHandler(id, handler)

        return () => {
          localEndpointsStore.act.setHandler(id, undefined)
        }
      },
    }
  }
}
