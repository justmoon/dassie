import { createConnection, createServer } from "ilp-protocol-stream"

import {
  IlpType,
  parseIlpPacket,
  serializeIlpPacket,
} from "@dassie/lib-protocol-ilp"
import { bufferToUint8Array } from "@dassie/lib-type-utils"

import type { StreamProtocolContext } from "../../context/context"

export function createCompatibilityPlugin(context: StreamProtocolContext) {
  let connected = false
  let deregisterHandler: (() => void) | undefined

  return {
    connect: () => {
      connected = true
      return Promise.resolve()
    },
    disconnect: () => {
      connected = false
      return Promise.resolve()
    },
    isConnected: () => connected,
    async sendData(data: Buffer) {
      const packet = parseIlpPacket(bufferToUint8Array(data))

      if (packet.type !== IlpType.Prepare) {
        throw new Error("unexpected packet type")
      }

      const result = await context.endpoint.sendPacket(packet.data)

      const serializedResult = serializeIlpPacket(result)

      return Buffer.from(serializedResult)
    },
    registerDataHandler(newHandler: (data: Buffer) => Promise<Buffer>) {
      deregisterHandler = context.endpoint.handlePackets(async (packet) => {
        const data = serializeIlpPacket({
          type: IlpType.Prepare,
          data: packet,
        })
        const dataBuffer = Buffer.from(data)

        const resultData = bufferToUint8Array(await newHandler(dataBuffer))

        const resultPacket = parseIlpPacket(resultData)

        if (resultPacket.type === IlpType.Prepare) {
          throw new Error("Unexpected packet type")
        }

        return resultPacket
      })
    },
    deregisterDataHandler() {
      deregisterHandler?.()
    },
  }
}

export async function createCompatibilityServer(
  context: StreamProtocolContext,
) {
  const plugin = createCompatibilityPlugin(context)

  const server = await createServer({
    plugin,
    serverSecret: Buffer.alloc(32),
  })

  context.scope.onCleanup(async () => {
    await server.close()
  })

  return server
}

export async function createCompatibilityClient(
  context: StreamProtocolContext,
  connectionOptions: Omit<Parameters<typeof createConnection>[0], "plugin">,
) {
  const plugin = createCompatibilityPlugin(context)

  const connection = await createConnection({
    ...connectionOptions,
    plugin,
  })

  context.scope.onCleanup(async () => {
    await connection.end()
  })

  return connection
}
