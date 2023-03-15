import {
  BadRequestError,
  assertAcceptHeader,
  assertContentTypeHeader,
  asyncHandler,
  parseBody,
  respondBinary,
} from "@dassie/lib-http-server"
import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { routerService } from "../http-server/serve-http"
import { ALLOW_ANONYMOUS_USAGE } from "./constants/anonymous-messages"
import {
  handlePeerMessage,
  incomingPeerMessageTopic,
} from "./handle-peer-message"
import { peerMessage as peerMessageSchema } from "./peer-schema"
import { peerTableStore } from "./stores/peer-table"
import { authenticatePeerMessage } from "./utils/authenticate-peer-message"

const logger = createLogger("das:node:handle-peer-http-request")

export const registerPeerHttpHandler = (sig: EffectContext) => {
  const router = sig.get(routerService)

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

      const subnetId = parseResult.value.subnetId
      const senderId = parseResult.value.sender
      const senderPublicKey = sig
        .use(peerTableStore)
        .read()
        .get(`${subnetId}.${senderId}`)?.nodePublicKey
      const isAuthenticated =
        !!senderPublicKey &&
        authenticatePeerMessage(senderPublicKey, parseResult.value)

      // Only certain messages are allowed to be sent anonymously
      if (
        !isAuthenticated &&
        !ALLOW_ANONYMOUS_USAGE.includes(
          String(Object.keys(parseResult.value.content.value)[0])
        )
      ) {
        logger.debug("incoming dassie message is not authenticated, ignoring", {
          message: parseResult.value,
          body,
        })
        return
      }

      sig.use(incomingPeerMessageTopic).emit({
        message: parseResult.value,
        authenticated: isAuthenticated,
        asUint8Array: body,
      })

      const responseMessage = handlePeerMessage({
        reactor: sig.reactor,
        message: parseResult.value,
        authenticated: isAuthenticated,
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
}
