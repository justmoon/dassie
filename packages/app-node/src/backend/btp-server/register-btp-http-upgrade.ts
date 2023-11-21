import { nanoid } from "nanoid"
import type { RawData, WebSocket } from "ws"
import { WebSocketServer } from "ws"

import {
  BtpContentType,
  BtpType,
  btpEnvelopeSchema,
  btpMessageSchema,
  btpTransferSchema,
} from "@dassie/lib-protocol-utils"
import { Reactor, createActor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { OwnerLedgerIdSignal } from "../accounting/signals/owner-ledger-id"
import { BtpTokensStore } from "../api-keys/database-stores/btp-tokens"
import { BtpToken } from "../api-keys/types/btp-token"
import { WebsocketRoutesSignal } from "../http-server/serve-https"
import { NodeIlpAddressSignal } from "../ilp-connector/computed/node-ilp-address"
import { ProcessPacketActor } from "../ilp-connector/process-packet"
import { BtpEndpointInfo } from "../ilp-connector/senders/send-btp-packets"
import { btp as logger } from "../logger/instances"
import { RoutingTableSignal } from "../routing/signals/routing-table"

let unique = 0

export const RegisterBtpHttpUpgradeActor = (reactor: Reactor) => {
  const processIncomingPacketActor = reactor.use(ProcessPacketActor)
  const btpTokensStore = reactor.use(BtpTokensStore)
  const routingTableSignal = reactor.use(RoutingTableSignal)
  const ownerLedgerIdSignal = reactor.use(OwnerLedgerIdSignal)

  return createActor((sig) => {
    const nodeIlpAddress = sig.readAndTrack(NodeIlpAddressSignal)
    const websocketRoutes = sig.readAndTrack(WebsocketRoutesSignal)

    const socketMap = new Map<number, WebSocket>()

    const handleConnection = (socket: WebSocket) => {
      try {
        const connectionId = unique++
        socketMap.set(connectionId, socket)

        let localIlpAddressPart: string
        do {
          localIlpAddressPart = nanoid(6)
        } while (
          routingTableSignal
            .read()
            .get(`${nodeIlpAddress}.${localIlpAddressPart}`)
        )

        const endpointInfo: BtpEndpointInfo = {
          type: "btp",
          connectionId,
          ilpAddress: `${nodeIlpAddress}.${localIlpAddressPart}`,
          accountPath: `${ownerLedgerIdSignal.read()}:owner/btp`,
        }

        logger.debug("handle BTP websocket connection", { connectionId })

        const handleDataOnUnauthenticatedConnection = (
          messageBuffer: RawData,
        ) => {
          try {
            const envelopeParseResult = btpEnvelopeSchema.parse(
              messageBuffer as Uint8Array,
            )

            if (isFailure(envelopeParseResult)) {
              logger.warn(
                "received invalid BTP packet on unauthenticated connection, closing connection",
                {
                  error: envelopeParseResult,
                },
              )
              socket.close()
              return
            }

            const envelope = envelopeParseResult.value

            if (envelope.messageType !== BtpType.Message) {
              logger.warn(
                "received non-message BTP packet on unauthenticated connection, closing connection",
                {
                  type: envelope.messageType,
                },
              )
              socket.close()
              return
            }

            const messageParseResult = btpMessageSchema.parse(envelope.message)

            if (isFailure(messageParseResult)) {
              logger.warn(
                "received invalid BTP message on unauthenticated connection, closing connection",
                {
                  error: messageParseResult,
                },
              )
              socket.close()
              return
            }

            const message = messageParseResult.value

            const primaryProtocolData = message.protocolData[0]

            if (primaryProtocolData?.protocolName !== "auth") {
              logger.warn(
                "received BTP message without auth protocol data on unauthenticated connection, closing connection",
              )
              socket.close()
              return
            }

            if (
              primaryProtocolData.contentType !==
              BtpContentType.ApplicationOctetStream
            ) {
              logger.warn(
                "received BTP auth packet with auth data entry having the wrong content type, should be application/octet-stream, closing connection",
                { contentType: primaryProtocolData.contentType },
              )
              socket.close()
              return
            }

            if (primaryProtocolData.data.length > 0) {
              logger.warn(
                "received BTP auth packet with auth data entry having data, should be empty, closing connection",
                {
                  length: primaryProtocolData.data.length,
                },
              )
              socket.close()
              return
            }

            const tokenData = message.protocolData.find(
              ({ protocolName }) => protocolName === "auth_token",
            )

            if (!tokenData) {
              logger.warn(
                "received BTP auth packet without auth_token data entry, closing connection",
              )
              socket.close()
              return
            }

            if (tokenData.contentType !== BtpContentType.TextPlainUtf8) {
              logger.warn(
                "received BTP auth packet with auth_token data having the wrong content type, should be text/plain",
                { contentType: tokenData.contentType },
              )
              socket.close()
              return
            }

            const token = Buffer.from(tokenData.data).toString("utf8")

            if (!btpTokensStore.read().has(token as BtpToken)) {
              logger.warn(
                "received BTP auth packet with invalid token, closing connection",
                { token },
              )
              socket.close()
              return
            }

            logger.debug("received valid BTP auth packet")

            const serializedResponse = btpMessageSchema.serializeOrThrow({
              protocolData: [],
            })
            const serializedEnvelope = btpEnvelopeSchema.serializeOrThrow({
              messageType: 1,
              requestId: envelope.requestId,
              message: serializedResponse,
            })
            socket.send(serializedEnvelope)

            socket.off("message", handleDataOnUnauthenticatedConnection)
            socket.on("message", handleDataOnAuthenticatedConnection)
            return
          } catch (error) {
            logger.warn(
              "error while processing pre-auth BTP message, closing connection",
              { error },
            )
            socket.close()
          }
        }

        const handleDataOnAuthenticatedConnection = (
          messageBuffer: RawData,
        ) => {
          const messageResult = btpEnvelopeSchema.parse(
            messageBuffer as Uint8Array,
          )
          if (isFailure(messageResult)) {
            logger.debug("failed to parse BTP message envelope", {
              // eslint-disable-next-line @typescript-eslint/no-base-to-string
              message: messageBuffer.toString("hex"),
              error: messageResult,
            })
            return
          }

          const message = messageResult.value
          logger.debug("received BTP message", {
            type: message.messageType,
          })

          switch (message.messageType) {
            case BtpType.Message: {
              const messageParseResult = btpMessageSchema.parse(message.message)

              if (isFailure(messageParseResult)) {
                logger.debug("failed to parse BTP message payload", {
                  error: messageParseResult,
                })
                return
              }

              if (
                messageParseResult.value.protocolData.some(
                  ({ protocolName }) => protocolName === "auth",
                )
              ) {
                logger.debug(
                  "received BTP auth packet on already authenticated connection, closing connection",
                )
                socket.close()
                return
              }

              for (const protocolData of messageParseResult.value
                .protocolData) {
                if (protocolData.protocolName === "ilp") {
                  logger.debug("received ILP packet via BTP message")
                  processIncomingPacketActor.api.handle.tell({
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
                message.message,
              )

              if (isFailure(transferParseResult)) {
                logger.debug("failed to parse BTP transfer payload", {
                  error: transferParseResult,
                })
                return
              }

              for (const protocolData of transferParseResult.value
                .protocolData) {
                if (protocolData.protocolName === "ilp") {
                  logger.debug("received ILP packet via BTP transfer")

                  processIncomingPacketActor.api.handle.tell({
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
                message.message,
              )

              if (isFailure(responseParseResult)) {
                logger.debug("failed to parse BTP response payload", {
                  error: responseParseResult,
                })
                return
              }

              for (const protocolData of responseParseResult.value
                .protocolData) {
                if (protocolData.protocolName === "ilp") {
                  logger.debug("received ILP packet via BTP response")
                  processIncomingPacketActor.api.handle.tell({
                    sourceEndpointInfo: endpointInfo,
                    serializedPacket: protocolData.data,
                    requestId: message.requestId,
                  })
                  return
                }
              }
            }
          }
        }

        socket.on("message", handleDataOnUnauthenticatedConnection)

        routingTableSignal
          .read()
          .set(`${nodeIlpAddress}.${localIlpAddressPart}`, {
            type: "fixed",
            destination: endpointInfo,
          })

        socket.on("close", () => {
          routingTableSignal
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
}
