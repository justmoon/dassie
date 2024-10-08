import type { Plugin } from "ilp-protocol-stream/dist/src/util/plugin-interface"

import {
  IlpType,
  parseIlpPacket,
  serializeIlpPacket,
} from "@dassie/lib-protocol-ilp"
import { type Scope } from "@dassie/lib-reactive"
import { bufferToUint8Array } from "@dassie/lib-type-utils"

import type { DassieReactor } from "../../base/types/dassie-base"
import { CreateLocalEndpoint } from "../../local-ilp/functions/create-local-endpoint"

export const CreatePlugin = (reactor: DassieReactor) => {
  const createLocalEndpoint = reactor.use(CreateLocalEndpoint)

  return function createPlugin(scope: Scope): Plugin {
    let connected = false
    let handlerDisposer: (() => void) | undefined
    const localEndpoint = createLocalEndpoint({
      scope,
      hint: "plugin",
    })

    return {
      connect: () => {
        connected = true
        return Promise.resolve()
      },
      disconnect: () => {
        connected = false
        return Promise.resolve()
      },
      isConnected: () => {
        return connected
      },
      sendData: async (data: Buffer) => {
        if (!connected) {
          throw new Error("Plugin is not connected")
        }

        const parsedPacket = parseIlpPacket(bufferToUint8Array(data))
        if (parsedPacket.type !== IlpType.Prepare) {
          throw new Error("Unexpected packet type")
        }

        const resultPacket = await localEndpoint.sendPacket(parsedPacket.data)

        return Buffer.from(serializeIlpPacket(resultPacket))
      },
      registerDataHandler: (handler: (data: Buffer) => Promise<Buffer>) => {
        if (handlerDisposer) {
          throw new Error("Cannot register multiple handlers")
        }

        handlerDisposer = localEndpoint.handlePackets(async (packet) => {
          const result = await handler(
            Buffer.from(
              serializeIlpPacket({
                type: IlpType.Prepare,
                data: packet,
              }),
            ),
          )

          const parsedResult = parseIlpPacket(bufferToUint8Array(result))

          if (parsedResult.type === IlpType.Prepare) {
            throw new Error("Received prepare packet as a result")
          }

          return parsedResult
        })
      },
      deregisterDataHandler: () => {
        if (!handlerDisposer) {
          throw new Error("No handler registered")
        }

        handlerDisposer()
        handlerDisposer = undefined
      },
    }
  }
}
