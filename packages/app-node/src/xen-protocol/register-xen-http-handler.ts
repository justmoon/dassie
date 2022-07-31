import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { BadRequestError } from "../constants/http"
import { routerValue } from "../http-server/serve-http"
import {
  assertAcceptHeader,
  assertContentTypeHeader,
  parseBody,
  respondPlainly,
} from "../utils/http"
import { incomingXenMessageTopic } from "./handle-xen-messages"
import { xenMessage } from "./xen-schema"

const logger = createLogger("xen:node:handle-xen-http-request")

export const registerXenHttpHandler = (sig: EffectContext) => {
  const router = sig.get(routerValue)

  router.on("post", "/xen", async (request, response) => {
    assertAcceptHeader(request, "application/xen-message")
    assertContentTypeHeader(request, "application/xen-message")

    const body = await parseBody(request)

    const parseResult = xenMessage.parse(body)

    if (!parseResult.success) {
      logger.debug("error while parsing incoming xen message", {
        error: parseResult.failure,
        body,
      })
      throw new BadRequestError(`Bad Request, failed to parse message`)
    }

    sig.emit(incomingXenMessageTopic, {
      message: parseResult.value,
      asUint8Array: body,
    })

    respondPlainly(response, 200, "OK")
  })
}
