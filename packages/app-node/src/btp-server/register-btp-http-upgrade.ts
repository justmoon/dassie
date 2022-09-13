import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { createLogger } from "@dassie/lib-logger"
import {
  BtpType,
  btpEnvelopeSchema,
  btpMessageSchema,
  btpTransferSchema,
} from "@dassie/lib-protocol-utils"
import type { EffectContext } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { httpService } from "../http-server/serve-http"
import { incomingIlpPacketBuffer } from "../ilp-connector/topics/incoming-ilp-packet"

const logger = createLogger("das:node:websocket-server")

let unique = 0

export const connectionMap = new Map<string, WebSocket>()

export const registerBtpHttpUpgrade = (sig: EffectContext) => {
  const httpServer = sig.get(httpService)

  if (!httpServer) return

  const ilpAddress = sig.get(configSignal, (config) => config.ilpAddress)

  const websocketServer = new WebSocketServer({ noServer: true })

  const handleConnection = (socket: WebSocket) => {
    try {
      const connectionId = unique++

      const clientIlpAddress = `${ilpAddress}.c${connectionId}`

      connectionMap.set(clientIlpAddress, socket)

      logger.debug("handle BTP websocket connection", { connectionId })

      socket.on("message", (messageBuffer) => {
        const messageResult = btpEnvelopeSchema.parse(
          messageBuffer as Uint8Array
        )
        if (messageResult.success) {
          const message = messageResult.value
          logger.debug("received BTP message", {
            type: message.messageType,
          })

          switch (message.messageType) {
            case BtpType.Message: {
              const messageParseResult = btpMessageSchema.parse(message.message)

              if (!messageParseResult.success) {
                logger.debug("failed to parse BTP message payload", {
                  error: messageParseResult.failure,
                })
                return
              }

              if (
                messageParseResult.value.protocolData.some(
                  ({ protocolName }) => protocolName === "auth"
                )
              ) {
                logger.debug("received BTP auth packet")
                const responseSerializationResult = btpMessageSchema.serialize({
                  protocolData: [],
                })
                if (!responseSerializationResult.success) {
                  logger.error("error serializing BTP response message", {
                    error: responseSerializationResult.failure,
                  })
                  return
                }
                const envelopeSerializationResult = btpEnvelopeSchema.serialize(
                  {
                    messageType: 1,
                    requestId: message.requestId,
                    message: responseSerializationResult.value,
                  }
                )
                if (!envelopeSerializationResult.success) {
                  logger.error("error serializing BTP response", {
                    error: envelopeSerializationResult.failure,
                  })
                  return
                }
                socket.send(envelopeSerializationResult.value)
                return
              }

              for (const protocolData of messageParseResult.value
                .protocolData) {
                if (protocolData.protocolName === "ilp") {
                  logger.debug("received ILP packet via BTP message")
                  sig.use(incomingIlpPacketBuffer).emit({
                    source: clientIlpAddress,
                    packet: protocolData.data,
                    requestId: message.requestId,
                  })
                  return
                }
              }

              return
            }

            case BtpType.Transfer: {
              const transferParseResult = btpTransferSchema.parse(
                message.message
              )

              if (!transferParseResult.success) {
                logger.debug("failed to parse BTP transfer payload", {
                  error: transferParseResult.failure,
                })
                return
              }

              for (const protocolData of transferParseResult.value
                .protocolData) {
                if (protocolData.protocolName === "ilp") {
                  logger.debug("received ILP packet via BTP transfer")
                  sig.use(incomingIlpPacketBuffer).emit({
                    source: clientIlpAddress,
                    packet: protocolData.data,
                    requestId: message.requestId,
                  })
                  return
                }
              }
              return
            }

            case BtpType.Response: {
              const responseParseResult = btpMessageSchema.parse(
                message.message
              )

              if (!responseParseResult.success) {
                logger.debug("failed to parse BTP response payload", {
                  error: responseParseResult.failure,
                })
                return
              }

              for (const protocolData of responseParseResult.value
                .protocolData) {
                if (protocolData.protocolName === "ilp") {
                  logger.debug("received ILP packet via BTP response")
                  sig.use(incomingIlpPacketBuffer).emit({
                    source: clientIlpAddress,
                    packet: protocolData.data,
                    requestId: message.requestId,
                  })
                  return
                }
              }
            }
          }
        } else {
          logger.debug("failed to parse BTP message envelope", {
            // eslint-disable-next-line @typescript-eslint/no-base-to-string
            message: messageBuffer.toString("hex"),
            error: messageResult.failure,
          })
        }
      })
    } catch (error) {
      logger.error(
        "error handling websocket connection",
        { error },
        {
          skipAfter: "WebSocketService.handleConnection",
        }
      )
    }
  }

  websocketServer.on("connection", handleConnection)

  sig.onCleanup(() => {
    websocketServer.off("connection", handleConnection)
    websocketServer.close()
  })

  httpServer.on("upgrade", (request, socket, head) => {
    websocketServer.handleUpgrade(request, socket, head, (ws) => {
      websocketServer.emit("connection", ws, request)
    })
  })
}
