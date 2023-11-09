import {
  BadRequestError,
  assertAcceptHeader,
  assertContentTypeHeader,
  asyncHandler,
  parseBody,
} from "@dassie/lib-http-server"
import { Reactor, createActor } from "@dassie/lib-reactive"

import { HttpsRouterServiceActor } from "../http-server/serve-https"
import { ProcessPacketActor } from "../ilp-connector/process-packet"
import { IlpHttpEndpointInfo } from "../ilp-connector/senders/send-ilp-http-packets"
import { ILP_OVER_HTTP_CONTENT_TYPE } from "./constants/content-type"

export const RegisterIlpHttpCallbackHandlerActor = (reactor: Reactor) => {
  const processPacketActor = reactor.use(ProcessPacketActor)

  return createActor((sig) => {
    const router = sig.get(HttpsRouterServiceActor)

    if (!router) return

    router.post(
      "/ilp/callback",
      asyncHandler(async (request, response) => {
        assertAcceptHeader(request, ILP_OVER_HTTP_CONTENT_TYPE)
        assertContentTypeHeader(request, ILP_OVER_HTTP_CONTENT_TYPE)

        const body = await parseBody(request)

        const textualRequestIdHeader = request.headers["request-id"]
        const textualRequestId = Array.isArray(textualRequestIdHeader)
          ? textualRequestIdHeader[0]
          : textualRequestIdHeader

        if (!textualRequestId) {
          throw new BadRequestError("Missing required header Request-Id")
        }

        // TODO: Authentication

        const endpointInfo: IlpHttpEndpointInfo = {
          type: "http",
          accountPath: "builtin/owner/http",
          ilpAddress: "test.not-implemented",
        }

        processPacketActor.api.handle.tell({
          sourceEndpointInfo: endpointInfo,
          serializedPacket: body,
          requestId: Number(textualRequestId),
        })

        response.writeHead(202, {
          "Content-Type": ILP_OVER_HTTP_CONTENT_TYPE,
        })
      }),
    )
  })
}
