import {
  BadRequestFailure,
  UnauthorizedFailure,
  createAcceptHeaderAssertion,
  createBinaryResponse,
  createContentTypeHeaderAssertion,
} from "@dassie/lib-http-server"
import { Reactor, createActor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { HttpsRouter } from "../http-server/serve-https"
import { peerProtocol as logger } from "../logger/instances"
import {
  HandlePeerMessageActor,
  IncomingPeerMessageTopic,
} from "./actors/handle-peer-message"
import { ALLOW_ANONYMOUS_USAGE } from "./constants/anonymous-messages"
import { DASSIE_MESSAGE_CONTENT_TYPE } from "./constants/content-type"
import { AuthenticatePeerMessage } from "./functions/authenticate-peer-message"
import { peerMessage as peerMessageSchema } from "./peer-schema"
import { NodeTableStore } from "./stores/node-table"

export const RegisterPeerHttpHandlerActor = (reactor: Reactor) => {
  const authenticatePeerMessage = reactor.use(AuthenticatePeerMessage)
  const nodeTableStore = reactor.use(NodeTableStore)
  const http = reactor.use(HttpsRouter)
  const handlePeerMessageActor = reactor.use(HandlePeerMessageActor)

  return createActor((sig) => {
    http
      .post()
      .path("/peer")
      .bodyParser("uint8Array")
      .assert(createAcceptHeaderAssertion(DASSIE_MESSAGE_CONTENT_TYPE))
      .assert(createContentTypeHeaderAssertion(DASSIE_MESSAGE_CONTENT_TYPE))
      .handler(sig, async (request) => {
        const parseResult = peerMessageSchema.parse(request.body)
        if (isFailure(parseResult)) {
          logger.debug("error while parsing incoming dassie message", {
            error: parseResult,
            body: request.body,
          })
          return new BadRequestFailure(`Bad Request, failed to parse message`)
        }

        if (parseResult.value.version !== 0) {
          logger.debug("incoming dassie message has unknown version", {
            version: parseResult.value.version,
          })
          return new BadRequestFailure(`Bad Request, unsupported version`)
        }

        const senderNode = nodeTableStore.read().get(parseResult.value.sender)

        const isAuthenticated = authenticatePeerMessage(
          parseResult.value,
          senderNode,
        )

        // Only certain messages are allowed to be sent anonymously
        if (
          !isAuthenticated &&
          !ALLOW_ANONYMOUS_USAGE.includes(parseResult.value.content.value.type)
        ) {
          logger.debug(
            "incoming dassie message is not authenticated, ignoring",
            {
              message: parseResult.value,
            },
          )
          return new UnauthorizedFailure(
            "Incoming Dassie message is not authenticated",
          )
        }

        sig.reactor.use(IncomingPeerMessageTopic).emit({
          message: parseResult.value,
          authenticated: isAuthenticated,
          peerState: senderNode?.peerState,
          asUint8Array: request.body,
        })

        const responseMessage = await handlePeerMessageActor.api.handle.ask({
          message: parseResult.value,
          authenticated: isAuthenticated,
          peerState: senderNode?.peerState,
          asUint8Array: request.body,
        })

        return createBinaryResponse(responseMessage, {
          contentType: "application/dassie-peer-response",
        })
      })
  })
}
