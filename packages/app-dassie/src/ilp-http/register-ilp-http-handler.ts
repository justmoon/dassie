import {
  BadRequestFailure,
  createAcceptHeaderAssertion,
  createContentTypeHeaderAssertion,
  createPlainResponse,
  parseBodyUint8Array,
} from "@dassie/lib-http-server"
import { parseIlpPacket } from "@dassie/lib-protocol-ilp"
import { createActor } from "@dassie/lib-reactive"

import type { DassieReactor } from "../base/types/dassie-base"
import { HttpsRouter } from "../http-server/values/https-router"
import { ProcessPacket } from "../ilp-connector/functions/process-packet"
import type { IlpHttpEndpointInfo } from "../ilp-connector/senders/send-ilp-http-packets"
import { ILP_OVER_HTTP_CONTENT_TYPE } from "./constants/content-type"
import { IncomingRequestIdMap } from "./values/incoming-request-id-map"

export const RegisterIlpHttpHandlerActor = (reactor: DassieReactor) => {
  const processPacket = reactor.use(ProcessPacket)
  const incomingRequestIdMap = reactor.use(IncomingRequestIdMap)

  return createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)

    http
      .post()
      .path("/ilp")
      .assert(createAcceptHeaderAssertion(ILP_OVER_HTTP_CONTENT_TYPE))
      .assert(createContentTypeHeaderAssertion(ILP_OVER_HTTP_CONTENT_TYPE))
      .use(parseBodyUint8Array)
      .handler(sig, ({ request, body }) => {
        if (request.headers.get("prefer") !== "respond-async") {
          return new BadRequestFailure(
            "This implementation only supports asynchronous mode, expected 'Prefer: respond-async'",
          )
        }

        const requestId = request.headers.get("request-id")

        if (!requestId) {
          return new BadRequestFailure("Missing required header Request-Id")
        }

        const callbackUrl = request.headers.get("callback-url")

        if (!callbackUrl) {
          return new BadRequestFailure("Missing required header Callback-Url")
        }

        // TODO: Authentication, manage ILP-HTTP connections

        const endpointInfo: IlpHttpEndpointInfo = {
          type: "http",
          id: "TODO",
        }

        incomingRequestIdMap.set(requestId, {
          requestId,
          callbackUrl,
        })

        processPacket({
          sourceEndpointInfo: endpointInfo,
          serializedPacket: body,
          parsedPacket: parseIlpPacket(body),
          requestId,
        })

        return createPlainResponse("", {
          contentType: ILP_OVER_HTTP_CONTENT_TYPE,
          statusCode: 202,
        })
      })
  })
}
