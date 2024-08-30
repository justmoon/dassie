import { createServer } from "ilp-protocol-stream"

import {
  type IlpPreparePacket,
  IlpType,
  parseIlpPacket,
  serializeIlpPacket,
} from "@dassie/lib-protocol-ilp"
import { bufferToUint8Array } from "@dassie/lib-type-utils"

import { ildcpResponseSchema } from "./ildcp"

export async function createCompatibilityServer() {
  let connected = false
  let handler: ((data: Buffer) => Promise<Buffer>) | undefined

  const server = await createServer({
    plugin: {
      connect: () => {
        connected = true
        return Promise.resolve()
      },
      disconnect: () => {
        connected = false
        return Promise.resolve()
      },
      isConnected: () => connected,
      sendData(data) {
        const packet = parseIlpPacket(bufferToUint8Array(data))
        if (
          packet.type === IlpType.Prepare &&
          packet.data.destination === "peer.config"
        ) {
          const ildcpResponse = ildcpResponseSchema.serializeOrThrow({
            address: "test.dummy",
            assetScale: 9,
            assetCode: "XRP",
          })

          return Promise.resolve(
            Buffer.from(
              serializeIlpPacket({
                type: IlpType.Fulfill,
                data: {
                  fulfillment: Buffer.alloc(32),
                  data: ildcpResponse,
                },
              }),
            ),
          )
        }

        throw new Error("not implemented")
      },
      registerDataHandler(newHandler) {
        handler = newHandler
      },
      deregisterDataHandler() {
        handler = undefined
      },
    },
    serverSecret: Buffer.alloc(32),
  })

  return {
    server,

    sendPacket: async (packet: IlpPreparePacket) => {
      const data = serializeIlpPacket({
        type: IlpType.Prepare,
        data: packet,
      })
      const dataBuffer = Buffer.from(data)

      if (!handler) {
        throw new Error("no handler registered")
      }

      const resultData = bufferToUint8Array(await handler(dataBuffer))

      const resultPacket = parseIlpPacket(resultData)

      if (resultPacket.type === IlpType.Prepare) {
        throw new Error("Unexpected packet type")
      }

      return resultPacket
    },
  }
}
