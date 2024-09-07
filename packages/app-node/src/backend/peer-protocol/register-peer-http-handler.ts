import {
  BadRequestFailure,
  UnauthorizedFailure,
  createAcceptHeaderAssertion,
  createBinaryResponse,
  createContentTypeHeaderAssertion,
  parseBodyOer,
} from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import type { DassieReactor } from "../base/types/dassie-base"
import { HttpsRouter } from "../http-server/values/https-router"
import { peerProtocol as logger } from "../logger/instances"
import { ALLOW_ANONYMOUS_USAGE } from "./constants/anonymous-messages"
import { DASSIE_MESSAGE_CONTENT_TYPE } from "./constants/content-type"
import { AuthenticatePeerMessage } from "./functions/authenticate-peer-message"
import {
  HandlePeerMessage,
  type IncomingPeerMessageEvent,
  IncomingPeerMessageTopic,
} from "./functions/handle-peer-message"
import { peerMessage as peerMessageSchema } from "./peer-schema"
import { NodeTableStore } from "./stores/node-table"

export const RegisterPeerHttpHandlerActor = (reactor: DassieReactor) => {
  const authenticatePeerMessage = reactor.use(AuthenticatePeerMessage)
  const nodeTableStore = reactor.use(NodeTableStore)
  const http = reactor.use(HttpsRouter)
  const handlePeerMessage = reactor.use(HandlePeerMessage)

  return createActor((sig) => {
    http
      .post()
      .path("/peer")
      .assert(createAcceptHeaderAssertion(DASSIE_MESSAGE_CONTENT_TYPE))
      .assert(createContentTypeHeaderAssertion(DASSIE_MESSAGE_CONTENT_TYPE))
      .use(parseBodyOer(peerMessageSchema))
      .handler(sig, async ({ body }) => {
        if (body.version !== 0) {
          logger.debug?.("incoming dassie message has unknown version", {
            version: body.version,
          })
          return new BadRequestFailure(`Bad Request, unsupported version`)
        }

        const senderNode = nodeTableStore.read().get(body.sender)

        const isAuthenticated = authenticatePeerMessage(body, senderNode)

        // Only certain messages are allowed to be sent anonymously
        if (
          !isAuthenticated &&
          !ALLOW_ANONYMOUS_USAGE.includes(body.content.value.type)
        ) {
          logger.debug?.(
            "incoming dassie message is not authenticated, ignoring",
            {
              message: body,
            },
          )
          return new UnauthorizedFailure(
            "Incoming Dassie message is not authenticated",
          )
        }

        const event: IncomingPeerMessageEvent = {
          message: body,
          authenticated: isAuthenticated,
          peerState: senderNode?.peerState,
        }

        sig.reactor.use(IncomingPeerMessageTopic).emit(event)

        const responseMessage = await handlePeerMessage(event)

        return createBinaryResponse(responseMessage, {
          contentType: "application/dassie-peer-response",
        })
      })
  })
}
