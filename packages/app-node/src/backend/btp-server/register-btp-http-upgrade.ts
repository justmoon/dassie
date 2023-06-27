import { nanoid } from "nanoid"
import type { WebSocket } from "ws"
import { WebSocketServer } from "ws"

import { createLogger } from "@dassie/lib-logger"
import {
  BtpType,
  btpEnvelopeSchema,
  btpMessageSchema,
  btpTransferSchema,
} from "@dassie/lib-protocol-utils"
import { createActor } from "@dassie/lib-reactive"

import { websocketRoutesSignal } from "../http-server/serve-http"
import { nodeIlpAddressSignal } from "../ilp-connector/computed/node-ilp-address"
import { processPacket } from "../ilp-connector/process-packet"
import { BtpEndpointInfo } from "../ilp-connector/senders/send-btp-packets"
import { routingTableSignal } from "../routing/signals/routing-table"

const logger = createLogger("das:node:websocket-server")

let unique = 0

export const registerBtpHttpUpgrade = () =>
  createActor((sig) => {
    const nodeIlpAddress = sig.get(nodeIlpAddressSignal)

    const websocketRoutes = sig.get(websocketRoutesSignal)

    const processIncomingPacketActor = sig.use(processPacket)

    const socketMap = new Map<number, WebSocket>()

    const handleConnection = (socket: WebSocket) => {
      try {
        const connectionId = unique++
        socketMap.set(connectionId, socket)

        const ilpRoutingTable = sig.use(routingTableSignal)
        let localIlpAddressPart: string
        do {
          localIlpAddressPart = nanoid(6)
        } while (
          ilpRoutingTable.read().get(`${nodeIlpAddress}.${localIlpAddressPart}`)
        )

        const endpointInfo: BtpEndpointInfo = {
          type: "btp",
          connectionId,
          ilpAddress: `${nodeIlpAddress}.${localIlpAddressPart}`,
          accountPath: "builtin/owner/btp",
        }

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
                const messageParseResult = btpMessageSchema.parse(
                  message.message
                )

                if (!messageParseResult.success) {
                  logger.debug("failed to parse BTP message payload", {
                    error: messageParseResult.error,
                  })
                  return
                }

                if (
                  messageParseResult.value.protocolData.some(
                    ({ protocolName }) => protocolName === "auth"
                  )
                ) {
                  logger.debug("received BTP auth packet")
                  const responseSerializationResult =
                    btpMessageSchema.serialize({
                      protocolData: [],
                    })
                  if (!responseSerializationResult.success) {
                    logger.error("error serializing BTP response message", {
                      error: responseSerializationResult.error,
                    })
                    return
                  }
                  const envelopeSerializationResult =
                    btpEnvelopeSchema.serialize({
                      messageType: 1,
                      requestId: message.requestId,
                      message: responseSerializationResult.value,
                    })
                  if (!envelopeSerializationResult.success) {
                    logger.error("error serializing BTP response", {
                      error: envelopeSerializationResult.error,
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
                    processIncomingPacketActor.tell("handle", {
                      sourceEndpointInfo: endpointInfo,
                      serializedPacket: protocolData.data,
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
                    error: transferParseResult.error,
                  })
                  return
                }

                for (const protocolData of transferParseResult.value
                  .protocolData) {
                  if (protocolData.protocolName === "ilp") {
                    logger.debug("received ILP packet via BTP transfer")

                    processIncomingPacketActor.tell("handle", {
                      sourceEndpointInfo: endpointInfo,
                      serializedPacket: protocolData.data,
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
                    error: responseParseResult.error,
                  })
                  return
                }

                for (const protocolData of responseParseResult.value
                  .protocolData) {
                  if (protocolData.protocolName === "ilp") {
                    logger.debug("received ILP packet via BTP response")
                    processIncomingPacketActor.tell("handle", {
                      sourceEndpointInfo: endpointInfo,
                      serializedPacket: protocolData.data,
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
              error: messageResult.error,
            })
          }
        })

        ilpRoutingTable.read().set(`${nodeIlpAddress}.${localIlpAddressPart}`, {
          type: "fixed",
          destination: endpointInfo,
        })

        socket.on("close", () => {
          ilpRoutingTable
            .read()
            .delete(`${nodeIlpAddress}.${localIlpAddressPart}`)
        })
      } catch (error) {
        logger.error("error handling websocket connection", { error })
      }
    }

    const websocketServer = new WebSocketServer({ noServer: true })

    websocketServer.on("connection", handleConnection)

    websocketRoutes.set("/btp", (request, socket, head) => {
      websocketServer.handleUpgrade(request, socket, head, (ws) => {
        websocketServer.emit("connection", ws, request)
      })
    })

    sig.onCleanup(() => {
      websocketRoutes.delete("/btp")
      websocketServer.off("connection", handleConnection)
      websocketServer.close()
    })

    return {
      send: ({
        connectionId,
        message,
      }: {
        connectionId: number
        message: Uint8Array
      }) => {
        const socket = socketMap.get(connectionId)
        if (!socket) {
          logger.error("failed to send BTP message: no socket found", {
            connectionId,
          })
          return
        }

        socket.send(message)
      },
    }
  })
