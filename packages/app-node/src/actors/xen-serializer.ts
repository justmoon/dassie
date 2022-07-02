import type { EventBroker } from "@xen-ilp/lib-events"
import { createLogger } from "@xen-ilp/lib-logger"

import { XenMessage, parseMessage } from "../codecs/xen-message"
import { BadRequestError } from "../services/http"
import {
  incomingXenMessageBufferTopic,
  incomingXenMessageTopic,
} from "../topics/xen-protocol"

const logger = createLogger("xen:node:xen-serializer")

interface XenSerializerContext {
  eventBroker: EventBroker
}

export default class XenSerializer {
  constructor(readonly context: XenSerializerContext) {
    context.eventBroker.addListener(
      incomingXenMessageBufferTopic,
      this.handleIncomingXenMessageBuffer
    )
  }

  handleIncomingXenMessageBuffer = async (buffer: Buffer) => {
    let message: XenMessage
    try {
      message = parseMessage(buffer)
    } catch (error) {
      logger.logError(error, { ignoreInProduction: true })
      throw new BadRequestError(`Bad Request, failed to parse message`)
    }

    await this.context.eventBroker.emitAndWait(incomingXenMessageTopic, message)
  }
}
