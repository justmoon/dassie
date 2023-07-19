import {
  BadRequestError,
  assertAcceptHeader,
  assertContentTypeHeader,
  asyncHandler,
  parseBody,
  respondBinary,
} from "@dassie/lib-http-server"
import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { httpsRouterService } from "../http-server/serve-https"
import {
  handlePeerMessage,
  incomingPeerMessageTopic,
} from "./actors/handle-peer-message"
import { ALLOW_ANONYMOUS_USAGE } from "./constants/anonymous-messages"
import { peerMessage as peerMessageSchema } from "./peer-schema"
import { nodeTableStore } from "./stores/node-table"
import { authenticatePeerMessage } from "./utils/authenticate-peer-message"

const logger = createLogger("das:node:handle-peer-http-request")

export const registerPeerHttpHandler = () =>
  createActor((sig) => {
    const router = sig.get(httpsRouterService)

    if (!router) return

    router.post(
      "/peer",
      asyncHandler(async (request, response) => {
        assertAcceptHeader(request, "application/dassie-peer-message")
        assertContentTypeHeader(request, "application/dassie-peer-message")

        const body = await parseBody(request)

        const parseResult = peerMessageSchema.parse(body)

        if (!parseResult.success) {
          logger.debug("error while parsing incoming dassie message", {
            error: parseResult.error,
            body,
          })
          throw new BadRequestError(`Bad Request, failed to parse message`)
        }

        if (parseResult.value.version !== 0) {
          logger.debug("incoming dassie message has unknown version", {
            version: parseResult.value.version,
            body,
          })
          throw new BadRequestError(`Bad Request, unsupported version`)
        }

        const senderId = parseResult.value.sender
        const senderNode = sig.use(nodeTableStore).read().get(senderId)

        const isAuthenticated =
          !!senderNode &&
          (senderNode.peerState.id === "peered" ||
            senderNode.peerState.id === "request-peering") &&
          authenticatePeerMessage(senderNode.nodePublicKey, parseResult.value)

        // Only certain messages are allowed to be sent anonymously
        if (
          !isAuthenticated &&
          !ALLOW_ANONYMOUS_USAGE.includes(parseResult.value.content.value.type)
        ) {
          logger.debug(
            "incoming dassie message is not authenticated, ignoring",
            {
              message: parseResult.value,
              body,
            }
          )
          return
        }

        sig.use(incomingPeerMessageTopic).emit({
          message: parseResult.value,
          authenticated: isAuthenticated,
          peerState: senderNode?.peerState,
          asUint8Array: body,
        })

        const responseMessage = await sig.use(handlePeerMessage).ask("handle", {
          message: parseResult.value,
          authenticated: isAuthenticated,
          peerState: senderNode?.peerState,
          asUint8Array: body,
        })

        respondBinary(
          response,
          200,
          responseMessage,
          "application/dassie-peer-response"
        )
      })
    )
  })
