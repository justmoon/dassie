import type { IncomingMessage } from "node:http"

import { isFailure } from "@dassie/lib-type-utils"

import { PayloadTooLargeFailure } from "."
import { DEFAULT_MAX_BODY_SIZE } from "./constants"

export const parseBodyBuffer = async (
  request: IncomingMessage,
): Promise<PayloadTooLargeFailure | Buffer> => {
  const body: Buffer[] = []
  let dataReceived = 0

  for await (const chunk of request as AsyncIterable<Buffer>) {
    dataReceived += chunk.length

    if (dataReceived > DEFAULT_MAX_BODY_SIZE) {
      return new PayloadTooLargeFailure(
        `Payload Too Large, expected body to be less than ${DEFAULT_MAX_BODY_SIZE} bytes`,
      )
    }
    body.push(chunk)
  }

  return Buffer.concat(body)
}

export const parseBody = async (
  request: IncomingMessage,
): Promise<PayloadTooLargeFailure | Uint8Array> => {
  const buffer = await parseBodyBuffer(request)

  return isFailure(buffer) ? buffer : new Uint8Array(buffer)
}

export const parseBodyUtf8 = async (
  request: IncomingMessage,
): Promise<PayloadTooLargeFailure | string> => {
  const buffer = await parseBodyBuffer(request)

  return isFailure(buffer) ? buffer : buffer.toString("utf8")
}
