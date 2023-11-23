import { parse as secureJsonParse } from "secure-json-parse"
import { JsonValue } from "type-fest"
import { infer as InferZodType, ZodTypeAny } from "zod"

import type { IncomingMessage } from "node:http"

import { AnyOerType, Infer as InferOerType } from "@dassie/lib-oer"
import { isFailure } from "@dassie/lib-type-utils"

import { BadRequestFailure, PayloadTooLargeFailure } from "."
import { DEFAULT_MAX_BODY_SIZE } from "./constants"

export const parseBodyBuffer = async (
  request: IncomingMessage,
): Promise<Buffer | PayloadTooLargeFailure> => {
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

export const parseBodyUint8Array = async (
  request: IncomingMessage,
): Promise<Uint8Array | PayloadTooLargeFailure> => {
  const buffer = await parseBodyBuffer(request)

  return isFailure(buffer) ? buffer : new Uint8Array(buffer)
}

export const parseBodyUtf8 = async (
  request: IncomingMessage,
): Promise<string | PayloadTooLargeFailure> => {
  const buffer = await parseBodyBuffer(request)

  return isFailure(buffer) ? buffer : buffer.toString("utf8")
}

export const parseJson = async (
  request: IncomingMessage,
  // eslint-disable-next-line @typescript-eslint/no-redundant-type-constituents
): Promise<JsonValue | PayloadTooLargeFailure | BadRequestFailure> => {
  const body = await parseBodyUtf8(request)

  try {
    return isFailure(body) ? body : (secureJsonParse(body) as JsonValue)
  } catch (error) {
    console.debug("request body is not valid JSON", { error })
    return new BadRequestFailure("Invalid HTTP request body")
  }
}

export const parseBodyZod =
  <TSchema extends ZodTypeAny>(schema: TSchema) =>
  async (
    request: IncomingMessage,
  ): Promise<
    InferZodType<TSchema> | PayloadTooLargeFailure | BadRequestFailure
  > => {
    const body = await parseJson(request)
    if (isFailure(body)) return body

    const result = schema.safeParse(body)

    if (!result.success) {
      console.debug("request body does not pass schema", {
        error: result.error,
      })
      return new BadRequestFailure("Invalid HTTP request body")
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return result.data as InferZodType<TSchema>
  }

export const parseBodyOer =
  <TSchema extends AnyOerType>(schema: TSchema) =>
  async (
    request: IncomingMessage,
  ): Promise<
    InferOerType<TSchema> | PayloadTooLargeFailure | BadRequestFailure
  > => {
    const body = await parseBodyUint8Array(request)
    if (isFailure(body)) return body

    const result = schema.parse(body)

    if (isFailure(result)) {
      console.debug("request body does not pass schema", {
        error: result,
      })
      return new BadRequestFailure("Invalid HTTP request body")
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return result.value as InferOerType<TSchema>
  }
