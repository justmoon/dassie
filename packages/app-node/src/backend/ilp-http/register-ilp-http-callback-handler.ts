import {
  BadRequestFailure,
  createAcceptHeaderAssertion,
  createContentTypeHeaderAssertion,
  createPlainResponse,
} from "@dassie/lib-http-server"
import { Reactor, createActor } from "@dassie/lib-reactive"

import { HttpsRouter } from "../http-server/serve-https"
import { ProcessPacketActor } from "../ilp-connector/process-packet"
import { IlpHttpEndpointInfo } from "../ilp-connector/senders/send-ilp-http-packets"
import { ILP_OVER_HTTP_CONTENT_TYPE } from "./constants/content-type"

export const RegisterIlpHttpCallbackHandlerActor = (reactor: Reactor) => {
  const processPacketActor = reactor.use(ProcessPacketActor)

  return createActor((sig) => {
    const http = sig.use(HttpsRouter)

    http
      .post()
      .path("/ilp/callback")
      .bodyParser("uint8Array")
      .assert(createAcceptHeaderAssertion(ILP_OVER_HTTP_CONTENT_TYPE))
      .assert(createContentTypeHeaderAssertion(ILP_OVER_HTTP_CONTENT_TYPE))
      .handler((request) => {
        const textualRequestIdHeader = request.headers["request-id"]
        const textualRequestId = Array.isArray(textualRequestIdHeader)
          ? textualRequestIdHeader[0]
          : textualRequestIdHeader

        if (!textualRequestId) {
          return new BadRequestFailure("Missing required header Request-Id")
        }

        // TODO: Authentication

        const endpointInfo: IlpHttpEndpointInfo = {
          type: "http",
          accountPath: "builtin/owner/http",
          ilpAddress: "test.not-implemented",
        }

        processPacketActor.api.handle.tell({
          sourceEndpointInfo: endpointInfo,
          serializedPacket: request.body,
          requestId: Number(textualRequestId),
        })

        return createPlainResponse("", {
          contentType: ILP_OVER_HTTP_CONTENT_TYPE,
        })
      })
  })
}
