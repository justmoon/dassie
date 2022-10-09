import {
  BadRequestError,
  assertContentTypeHeader,
  parseBody,
  respondPlainly,
} from "@dassie/lib-http-server"
import { createLogger } from "@dassie/lib-logger"
import { ia5String, sequence, sequenceOf } from "@dassie/lib-oer"
import type { EffectContext } from "@dassie/lib-reactive"

import { routerService } from "../http-server/serve-http"
import { incomingPingTopic } from "./incoming-ping-topic"

const logger = createLogger("das:beacon:register-ping-http-handler")

const pingSchema = sequence({
  nodeId: ia5String(),
  url: ia5String(),
  subnets: sequenceOf(
    sequence({
      subnetId: ia5String(),
    })
  ),
})

export const registerPingHttpHandler = (sig: EffectContext) => {
  const router = sig.get(routerService)

  if (!router) return

  router.post("/ping", async (request, response) => {
    assertContentTypeHeader(request, "application/dassie-beacon-ping")

    const body = await parseBody(request)

    const parseResult = pingSchema.parse(body)

    if (!parseResult.success) {
      logger.debug("error while parsing incoming ping message", {
        error: parseResult.error,
        body,
      })
      throw new BadRequestError(`Bad Request, failed to parse message`)
    }

    sig.use(incomingPingTopic).emit(parseResult.value)

    respondPlainly(response, 200, "OK")
  })
}
