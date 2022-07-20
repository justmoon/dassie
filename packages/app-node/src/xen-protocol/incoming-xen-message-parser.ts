import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { BadRequestError } from "../constants/http"
import { parseMessage } from "./codecs/xen-message"
import {
  incomingXenMessageBufferTopic,
  incomingXenMessageTopic,
} from "./topics/xen-protocol"

const logger = createLogger("xen:node:incoming-xen-message-parser")

export const incomingXenMessageParser = (sig: EffectContext) => {
  sig.on(incomingXenMessageBufferTopic, async (buffer) => {
    let message
    try {
      message = parseMessage(buffer)
    } catch (error) {
      logger.logError(error, { ignoreInProduction: true })
      throw new BadRequestError(`Bad Request, failed to parse message`)
    }

    await sig.emitAndWait(incomingXenMessageTopic, message)
  })
}
