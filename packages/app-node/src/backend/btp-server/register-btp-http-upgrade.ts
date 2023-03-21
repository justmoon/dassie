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
import { IlpType } from "../ilp-connector/ilp-packet-codec"
import { ilpRoutingTableSignal } from "../ilp-connector/signals/ilp-routing-table"
import { primaryIlpAddressSignal } from "../ilp-connector/signals/primary-ilp-address"
import { incomingIlpPacketTopic } from "../ilp-connector/topics/incoming-ilp-packet"

const logger = createLogger("das:node:websocket-server")

let unique = 0

export const registerBtpHttpUpgrade = () =>
  createActor((sig) => {
    const nodeIlpAddress = sig.get(primaryIlpAddressSignal)
    if (!nodeIlpAddress) return

    const websocketRoutes = sig.get(websocketRoutesSignal)

    const websocketServer = new WebSocketServer({ noServer: true })

    const handleConnection = (socket: WebSocket) => {
      try {
        const connectionId = unique++
        const ilpClientMap = sig.use(ilpRoutingTableSignal)
        let ilpAddress: string
        do {
          ilpAddress = `${nodeIlpAddress}.${nanoid(6)}`
        } while (ilpClientMap.read().get(ilpAddress))

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
                    sig.use(incomingIlpPacketTopic).emitPacket({
                      source: ilpAddress,
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
                    error: transferParseResult.error,
                  })
                  return
                }

                for (const protocolData of transferParseResult.value
                  .protocolData) {
                  if (protocolData.protocolName === "ilp") {
                    logger.debug("received ILP packet via BTP transfer")
                    sig.use(incomingIlpPacketTopic).emitPacket({
                      source: ilpAddress,
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
                    error: responseParseResult.error,
                  })
                  return
                }

                for (const protocolData of responseParseResult.value
                  .protocolData) {
                  if (protocolData.protocolName === "ilp") {
                    logger.debug("received ILP packet via BTP response")
                    sig.use(incomingIlpPacketTopic).emitPacket({
                      source: ilpAddress,
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
              error: messageResult.error,
            })
          }
        })

        ilpClientMap.read().set(ilpAddress, {
          prefix: ilpAddress,
          type: "btp",
          sendPacket: ({ packet, asUint8Array, requestId }) => {
            if (packet.type !== IlpType.Prepare || packet.amount === 0n) {
              const btpMessageSerializeResult = btpMessageSchema.serialize({
                protocolData: [
                  {
                    protocolName: "ilp",
                    contentType: 0,
                    data: asUint8Array,
                  },
                ],
              })

              if (!btpMessageSerializeResult.success) {
                logger.error("could not serialize BTP message", {
                  error: btpMessageSerializeResult.error,
                })
                return
              }

              const btpEnvelopeSerializeResult = btpEnvelopeSchema.serialize({
                messageType:
                  packet.type === IlpType.Prepare
                    ? BtpType.Message
                    : BtpType.Response,
                requestId,
                message: btpMessageSerializeResult.value,
              })

              if (!btpEnvelopeSerializeResult.success) {
                logger.error("could not serialize BTP envelope", {
                  error: btpEnvelopeSerializeResult.error,
                })
                return
              }

              socket.send(btpEnvelopeSerializeResult.value)

              return
            } else {
              const btpMessageSerializeResult = btpMessageSchema.serialize({
                protocolData: [
                  {
                    protocolName: "ilp",
                    contentType: 0,
                    data: asUint8Array,
                  },
                ],
              })

              if (!btpMessageSerializeResult.success) {
                logger.error("could not serialize BTP message", {
                  error: btpMessageSerializeResult.error,
                })
                return
              }

              // const btpTransferSerializeResult = btpTransferSchema.serialize({
              //   amount,
              //   protocolData: [
              //     {
              //       protocolName: "ilp",
              //       contentType: 0,
              //       data: packet,
              //     },
              //   ],
              // })

              // if (!btpTransferSerializeResult.success) {
              //   logger.error("could not serialize BTP transfer", {
              //     error: btpTransferSerializeResult.failure,
              //   })
              //   return
              // }

              const btpEnvelopeSerializeResult = btpEnvelopeSchema.serialize({
                messageType: 6, // Message
                requestId,
                message: btpMessageSerializeResult.value,
              })

              if (!btpEnvelopeSerializeResult.success) {
                logger.error("could not serialize BTP envelope", {
                  error: btpEnvelopeSerializeResult.error,
                })
                return
              }

              socket.send(btpEnvelopeSerializeResult.value)

              return
            }
          },
        })

        socket.on("close", () => {
          sig.use(ilpRoutingTableSignal).read().delete(ilpAddress)
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
  })
