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
import { IncomingRequestIdMap, nextId } from "./values/incoming-request-id-map"

export const RegisterIlpHttpHandlerActor = (reactor: Reactor) => {
  const processPacketActor = reactor.use(ProcessPacketActor)
  const incomingRequestIdMap = reactor.use(IncomingRequestIdMap)

  return createActor((sig) => {
    const router = sig.get(HttpsRouterServiceActor)

    if (!router) return

    router.post(
      "/ilp",
      asyncHandler(async (request, response) => {
        assertAcceptHeader(request, ILP_OVER_HTTP_CONTENT_TYPE)
        assertContentTypeHeader(request, ILP_OVER_HTTP_CONTENT_TYPE)

        const body = await parseBody(request)

        if (request.headers["prefer"] !== "respond-async") {
          throw new BadRequestError(
            "This implementation only supports asynchronous mode, expected 'Prefer: respond-async'",
          )
        }

        const textualRequestIdHeader = request.headers["request-id"]
        const textualRequestId = Array.isArray(textualRequestIdHeader)
          ? textualRequestIdHeader[0]
          : textualRequestIdHeader

        if (!textualRequestId) {
          throw new BadRequestError("Missing required header Request-Id")
        }

        const callbackUrlHeader = request.headers["callback-url"]
        const callbackUrl = Array.isArray(callbackUrlHeader)
          ? callbackUrlHeader[0]
          : callbackUrlHeader

        if (!callbackUrl) {
          throw new BadRequestError("Missing required header Callback-Url")
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
          serializedPacket: body,
          requestId: numericRequestId,
        })

        response.writeHead(202, {
          "Content-Type": ILP_OVER_HTTP_CONTENT_TYPE,
        })
      }),
    )
  })
}
