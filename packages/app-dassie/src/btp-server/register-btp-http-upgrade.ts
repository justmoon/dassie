import { nanoid } from "nanoid"
import { uint8ArrayToHex } from "uint8array-extras"

import type { ServerWebSocket } from "@dassie/lib-http-server"
import { parseIlpPacket } from "@dassie/lib-protocol-ilp"
import {
  BtpContentType,
  BtpType,
  btpEnvelopeSchema,
  btpMessageSchema,
  btpTransferSchema,
} from "@dassie/lib-protocol-utils"
import { createActor, createSignal } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { BtpTokensStore } from "../api-keys/database-stores/btp-tokens"
import type { BtpToken } from "../api-keys/types/btp-token"
import type { DassieReactor } from "../base/types/dassie-base"
import { HttpsWebSocketRouter } from "../http-server/values/https-websocket-router"
import { NodeIlpAddressSignal } from "../ilp-connector/computed/node-ilp-address"
import { ProcessPacket } from "../ilp-connector/functions/process-packet"
import type { BtpEndpointInfo } from "../ilp-connector/senders/send-btp-packets"
import { btp as logger } from "../logger/instances"
import { RoutingTableSignal } from "../routing/signals/routing-table"

const NextConnectionIdSignal = () => createSignal<number>(0)

export const RegisterBtpHttpUpgradeActor = (reactor: DassieReactor) => {
  const processPacket = reactor.use(ProcessPacket)
  const btpTokensStore = reactor.use(BtpTokensStore)
  const routingTableSignal = reactor.use(RoutingTableSignal)
  const nextConnectionIdSignal = reactor.use(NextConnectionIdSignal)
  const websocketRouter = reactor.use(HttpsWebSocketRouter)

  return createActor((sig) => {
    const nodeIlpAddress = sig.readAndTrack(NodeIlpAddressSignal)

    const socketMap = new Map<number, ServerWebSocket>()

    websocketRouter
      .get()
      .path("/btp")
      .handler(sig, ({ upgrade }) => {
        return upgrade((websocket) => {
          const connectionId = nextConnectionIdSignal.read()
          let isAuthenticated = false
          nextConnectionIdSignal.update((id) => id + 1)

          socketMap.set(connectionId, websocket)

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
          }

          logger.debug?.("handle BTP websocket connection", { connectionId })

          const handleDataOnUnauthenticatedConnection = (
            message: Uint8Array,
          ) => {
            try {
              const envelopeParseResult = btpEnvelopeSchema.parse(message)

              if (isFailure(envelopeParseResult)) {
                logger.warn(
                  "received invalid BTP packet on unauthenticated connection, closing connection",
                  {
                    error: envelopeParseResult,
                  },
                )
                websocket.close()
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
                websocket.close()
                return
              }

              const messageParseResult = btpMessageSchema.parse(
                envelope.message,
              )

              if (isFailure(messageParseResult)) {
                logger.warn(
                  "received invalid BTP message on unauthenticated connection, closing connection",
                  {
                    error: messageParseResult,
                  },
                )
                websocket.close()
                return
              }

              const btpMessage = messageParseResult.value

              const primaryProtocolData = btpMessage.protocolData[0]

              if (primaryProtocolData?.protocolName !== "auth") {
                logger.warn(
                  "received BTP message without auth protocol data on unauthenticated connection, closing connection",
                )
                websocket.close()
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
                websocket.close()
                return
              }

              if (primaryProtocolData.data.length > 0) {
                logger.warn(
                  "received BTP auth packet with auth data entry having data, should be empty, closing connection",
                  {
                    length: primaryProtocolData.data.length,
                  },
                )
                websocket.close()
                return
              }

              const tokenData = btpMessage.protocolData.find(
                ({ protocolName }) => protocolName === "auth_token",
              )

              if (!tokenData) {
                logger.warn(
                  "received BTP auth packet without auth_token data entry, closing connection",
                )
                websocket.close()
                return
              }

              if (tokenData.contentType !== BtpContentType.TextPlainUtf8) {
                logger.warn(
                  "received BTP auth packet with auth_token data having the wrong content type, should be text/plain",
                  { contentType: tokenData.contentType },
                )
                websocket.close()
                return
              }

              const token = Buffer.from(tokenData.data).toString("utf8")

              if (!btpTokensStore.read().has(token as BtpToken)) {
                logger.warn(
                  "received BTP auth packet with invalid token, closing connection",
                  { token },
                )
                websocket.close()
                return
              }

              logger.debug?.("received valid BTP auth packet")

              const serializedResponse = btpMessageSchema.serializeOrThrow({
                protocolData: [],
              })
              const serializedEnvelope = btpEnvelopeSchema.serializeOrThrow({
                messageType: 1,
                requestId: envelope.requestId,
                message: serializedResponse,
              })
              websocket.send(serializedEnvelope)

              isAuthenticated = true

              return
            } catch (error) {
              logger.warn(
                "error while processing pre-auth BTP message, closing connection",
                { error },
              )
              websocket.close()
            }
          }

          const handleDataOnAuthenticatedConnection = (message: Uint8Array) => {
            const messageResult = btpEnvelopeSchema.parse(message)
            if (isFailure(messageResult)) {
              logger.debug?.("failed to parse BTP message envelope", {
                message: uint8ArrayToHex(message),
                error: messageResult,
              })
              return
            }

            const btpMessage = messageResult.value
            logger.debug?.("received BTP message", {
              type: btpMessage.messageType,
            })

            switch (btpMessage.messageType) {
              case BtpType.Message: {
                const messageParseResult = btpMessageSchema.parse(
                  btpMessage.message,
                )

                if (isFailure(messageParseResult)) {
                  logger.debug?.("failed to parse BTP message payload", {
                    error: messageParseResult,
                  })
                  return
                }

                if (
                  messageParseResult.value.protocolData.some(
                    ({ protocolName }) => protocolName === "auth",
                  )
                ) {
                  logger.debug?.(
                    "received BTP auth packet on already authenticated connection, closing connection",
                  )
                  websocket.close()
                  return
                }

                for (const protocolData of messageParseResult.value
                  .protocolData) {
                  if (protocolData.protocolName === "ilp") {
                    logger.debug?.("received ILP packet via BTP message")
                    processPacket({
                      sourceEndpointInfo: endpointInfo,
                      serializedPacket: protocolData.data,
                      parsedPacket: parseIlpPacket(protocolData.data),
                      requestId: btpMessage.requestId,
                    })
                    return
                  }
                }

                return
              }

              case BtpType.Transfer: {
                const transferParseResult = btpTransferSchema.parse(
                  btpMessage.message,
                )

                if (isFailure(transferParseResult)) {
                  logger.debug?.("failed to parse BTP transfer payload", {
                    error: transferParseResult,
                  })
                  return
                }

                for (const protocolData of transferParseResult.value
                  .protocolData) {
                  if (protocolData.protocolName === "ilp") {
                    logger.debug?.("received ILP packet via BTP transfer")

                    processPacket({
                      sourceEndpointInfo: endpointInfo,
                      serializedPacket: protocolData.data,
                      parsedPacket: parseIlpPacket(protocolData.data),
                      requestId: btpMessage.requestId,
                    })
                    return
                  }
                }
                return
              }

              case BtpType.Response: {
                const responseParseResult = btpMessageSchema.parse(
                  btpMessage.message,
                )

                if (isFailure(responseParseResult)) {
                  logger.debug?.("failed to parse BTP response payload", {
                    error: responseParseResult,
                  })
                  return
                }

                for (const protocolData of responseParseResult.value
                  .protocolData) {
                  if (protocolData.protocolName === "ilp") {
                    logger.debug?.("received ILP packet via BTP response")
                    processPacket({
                      sourceEndpointInfo: endpointInfo,
                      serializedPacket: protocolData.data,
                      parsedPacket: parseIlpPacket(protocolData.data),
                      requestId: btpMessage.requestId,
                    })
                    return
                  }
                }
              }
            }
          }

          function handleOpen() {
            routingTableSignal
              .read()
              .set(`${nodeIlpAddress}.${localIlpAddressPart}`, {
                type: "fixed",
                destination: endpointInfo,
              })
          }

          function handleClose() {
            routingTableSignal
              .read()
              .delete(`${nodeIlpAddress}.${localIlpAddressPart}`)
            socketMap.delete(connectionId)
          }

          function handleMessage(message: Uint8Array) {
            if (isAuthenticated) {
              handleDataOnAuthenticatedConnection(message)
            } else {
              handleDataOnUnauthenticatedConnection(message)
            }
          }

          websocket.addEventListener("open", handleOpen)
          websocket.addEventListener("message", (event) => {
            if (typeof event.data === "string") {
              logger.warn("received non-binary BTP message, closing connection")
              websocket.close()
              return
            }

            handleMessage(new Uint8Array(event.data))
          })
          websocket.addEventListener("close", handleClose)
        })
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
