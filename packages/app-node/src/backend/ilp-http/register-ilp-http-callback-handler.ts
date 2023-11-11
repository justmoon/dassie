import {
  BadRequestFailure,
  assertAcceptHeader,
  assertContentTypeHeader,
  createHandler,
  createPlainResponse,
  parseBody,
} from "@dassie/lib-http-server"
import { Reactor, createActor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

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
      createHandler(async (request) => {
        {
          const result = assertAcceptHeader(request, ILP_OVER_HTTP_CONTENT_TYPE)
          if (isFailure(result)) return result
        }

        {
          const result = assertContentTypeHeader(
            request,
            ILP_OVER_HTTP_CONTENT_TYPE,
          )
          if (isFailure(result)) return result
        }

        const body = await parseBody(request)

        if (isFailure(body)) return body

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
          serializedPacket: body,
          requestId: Number(textualRequestId),
        })

        return createPlainResponse("", {
          contentType: ILP_OVER_HTTP_CONTENT_TYPE,
        })
      }),
    )
  })
}
