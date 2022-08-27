import type { IncomingMessage } from "node:http"

import { DEFAULT_MAX_BODY_SIZE } from "./constants"
import { PayloadTooLargeError } from "./errors"

export const parseBody = async (
  request: IncomingMessage
): Promise<Uint8Array> => {
  const body: Buffer[] = []
  let dataReceived = 0

  for await (const chunk of request as AsyncIterable<Buffer>) {
    dataReceived += chunk.length

    if (dataReceived > DEFAULT_MAX_BODY_SIZE) {
      throw new PayloadTooLargeError(
        `Payload Too Large, expected body to be less than ${DEFAULT_MAX_BODY_SIZE} bytes`
      )
    }
    body.push(chunk)
  }

  return new Uint8Array(Buffer.concat(body))
}
