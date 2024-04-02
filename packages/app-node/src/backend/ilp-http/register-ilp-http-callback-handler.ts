import {
  BadRequestFailure,
  createAcceptHeaderAssertion,
  createContentTypeHeaderAssertion,
  createPlainResponse,
} from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { OwnerLedgerIdSignal } from "../accounting/signals/owner-ledger-id"
import { DassieReactor } from "../base/types/dassie-base"
import { HttpsRouter } from "../http-server/serve-https"
import { ProcessPacket } from "../ilp-connector/functions/process-packet"
import { parseIlpPacket } from "../ilp-connector/schemas/ilp-packet-codec"
import { IlpHttpEndpointInfo } from "../ilp-connector/senders/send-ilp-http-packets"
import { IlpAddress } from "../ilp-connector/types/ilp-address"
import { ILP_OVER_HTTP_CONTENT_TYPE } from "./constants/content-type"

export const RegisterIlpHttpCallbackHandlerActor = (reactor: DassieReactor) => {
  const processPacket = reactor.use(ProcessPacket)
  const ownerLedgerIdSignal = reactor.use(OwnerLedgerIdSignal)

  return createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)

    http
      .post()
      .path("/ilp/callback")
      .assert(createAcceptHeaderAssertion(ILP_OVER_HTTP_CONTENT_TYPE))
      .assert(createContentTypeHeaderAssertion(ILP_OVER_HTTP_CONTENT_TYPE))
      .bodyParser("uint8Array")
      .handler(sig, ({ request, body }) => {
        const textualRequestId = request.headers.get("request-id")

        if (!textualRequestId) {
          return new BadRequestFailure("Missing required header Request-Id")
        }

        // TODO: Authentication

        const endpointInfo: IlpHttpEndpointInfo = {
          type: "http",
          accountPath: `${ownerLedgerIdSignal.read()}:equity/owner`,
          ilpAddress: "test.not-implemented" as IlpAddress,
        }

        processPacket({
          sourceEndpointInfo: endpointInfo,
          serializedPacket: body,
          parsedPacket: parseIlpPacket(body),
          requestId: Number(textualRequestId),
        })

        return createPlainResponse("", {
          contentType: ILP_OVER_HTTP_CONTENT_TYPE,
        })
      })
  })
}
