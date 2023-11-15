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
import { IncomingRequestIdMap, nextId } from "./values/incoming-request-id-map"

export const RegisterIlpHttpHandlerActor = (reactor: Reactor) => {
  const processPacketActor = reactor.use(ProcessPacketActor)
  const incomingRequestIdMap = reactor.use(IncomingRequestIdMap)

  return createActor((sig) => {
    const http = sig.use(HttpsRouter)

    http
      .post()
      .path("/ilp")
      .bodyParser("uint8Array")
      .assert(createAcceptHeaderAssertion(ILP_OVER_HTTP_CONTENT_TYPE))
      .assert(createContentTypeHeaderAssertion(ILP_OVER_HTTP_CONTENT_TYPE))
      .handler((request) => {
        if (request.headers["prefer"] !== "respond-async") {
          return new BadRequestFailure(
            "This implementation only supports asynchronous mode, expected 'Prefer: respond-async'",
          )
        }

        const textualRequestIdHeader = request.headers["request-id"]
        const textualRequestId = Array.isArray(textualRequestIdHeader)
          ? textualRequestIdHeader[0]
          : textualRequestIdHeader

        if (!textualRequestId) {
          return new BadRequestFailure("Missing required header Request-Id")
        }

        const callbackUrlHeader = request.headers["callback-url"]
        const callbackUrl = Array.isArray(callbackUrlHeader)
          ? callbackUrlHeader[0]
          : callbackUrlHeader

        if (!callbackUrl) {
          return new BadRequestFailure("Missing required header Callback-Url")
        }

        // TODO: Authentication

        const endpointInfo: IlpHttpEndpointInfo = {
          type: "http",
          accountPath: "builtin/owner/http",
          ilpAddress: "test.not-implemented",
        }

        const numericRequestId = nextId()

        incomingRequestIdMap.set(numericRequestId, {
          requestId: textualRequestId,
          callbackUrl,
        })

        processPacketActor.api.handle.tell({
          sourceEndpointInfo: endpointInfo,
          serializedPacket: request.body,
          requestId: numericRequestId,
        })

        return createPlainResponse("", {
          contentType: ILP_OVER_HTTP_CONTENT_TYPE,
          statusCode: 202,
        })
      })
  })
}
