import { createLogger } from "@xen-ilp/lib-logger"

import { XenMessage, parseMessage } from "../codecs/xen-message"
import { BadRequestError } from "../services/http"
import type MessageBroker from "../services/message-broker"
import {
  incomingXenMessageBufferTopic,
  incomingXenMessageTopic,
} from "../topics/xen-protocol"

const logger = createLogger("xen:node:xen-serializer")

interface XenSerializerContext {
  messageBroker: MessageBroker
}

export default class XenSerializer {
  constructor(readonly context: XenSerializerContext) {
    context.messageBroker.addListener(
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

    await this.context.messageBroker.emitAndWait(
      incomingXenMessageTopic,
      message
    )
  }
}
