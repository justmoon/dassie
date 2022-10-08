import {
  BadRequestError,
  assertAcceptHeader,
  assertContentTypeHeader,
  asyncHandler,
  parseBody,
  respondPlainly,
} from "@dassie/lib-http-server"
import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { routerService } from "../http-server/serve-http"
import { incomingPeerMessageTopic } from "./handle-peer-messages"
import { peerMessage as peerMessageSchema } from "./peer-schema"
import { nodeTableStore } from "./stores/node-table"
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
          error: parseResult.failure,
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
      const senderPublicKey = sig
        .use(nodeTableStore)
        .read()
        .get(senderId)?.nodeKey
      const isAuthenticated =
        !!senderPublicKey &&
        authenticatePeerMessage(senderPublicKey, parseResult.value)

      sig.use(incomingPeerMessageTopic).emit({
        message: parseResult.value,
        authenticatedAs: isAuthenticated ? senderId : undefined,
        asUint8Array: body,
      })

      respondPlainly(response, 200, "OK")
    })
  )
}
