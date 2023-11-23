import { parse as secureJsonParse } from "secure-json-parse"
import { JsonValue } from "type-fest"
import { infer as InferZodType, ZodTypeAny } from "zod"

import type { IncomingMessage } from "node:http"

import { AnyOerType, Infer as InferOerType } from "@dassie/lib-oer"
import { bufferToUint8Array, isFailure } from "@dassie/lib-type-utils"

import { BadRequestFailure, PayloadTooLargeFailure } from "."
import { DEFAULT_MAX_BODY_SIZE } from "./constants"

export const parseBodyBuffer = async (
  request: IncomingMessage,
): Promise<{ body: Buffer } | PayloadTooLargeFailure> => {
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

  return { body: Buffer.concat(body) }
}

export const parseBodyUint8Array = async (
  request: IncomingMessage,
): Promise<{ body: Uint8Array } | PayloadTooLargeFailure> => {
  const buffer = await parseBodyBuffer(request)
  if (isFailure(buffer)) return buffer

  return { body: bufferToUint8Array(buffer.body) }
}

export const parseBodyUtf8 = async (
  request: IncomingMessage,
): Promise<{ body: string } | PayloadTooLargeFailure> => {
  const bufferBody = await parseBodyBuffer(request)
  if (isFailure(bufferBody)) return bufferBody

  return { body: bufferBody.body.toString("utf8") }
}

export const parseJson = async (
  request: IncomingMessage,
  // eslint-disable-next-line @typescript-eslint/no-redundant-type-constituents
): Promise<
  { body: JsonValue } | PayloadTooLargeFailure | BadRequestFailure
> => {
  const utf8Body = await parseBodyUtf8(request)
  if (isFailure(utf8Body)) return utf8Body

  try {
    return { body: secureJsonParse(utf8Body.body) as JsonValue }
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
    { body: InferZodType<TSchema> } | PayloadTooLargeFailure | BadRequestFailure
  > => {
    const jsonBody = await parseJson(request)
    if (isFailure(jsonBody)) return jsonBody

    const result = schema.safeParse(jsonBody.body)

    if (!result.success) {
      console.debug("request body does not pass schema", {
        error: result.error,
      })
      return new BadRequestFailure("Invalid HTTP request body")
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return { body: result.data as InferZodType<TSchema> }
  }

export const parseBodyOer =
  <TSchema extends AnyOerType>(schema: TSchema) =>
  async (
    request: IncomingMessage,
  ): Promise<
    { body: InferOerType<TSchema> } | PayloadTooLargeFailure | BadRequestFailure
  > => {
    const uint8ArrayBody = await parseBodyUint8Array(request)
    if (isFailure(uint8ArrayBody)) return uint8ArrayBody

    const result = schema.parse(uint8ArrayBody.body)

    if (isFailure(result)) {
      console.debug("request body does not pass schema", {
        error: result,
      })
      return new BadRequestFailure("Invalid HTTP request body")
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return { body: result.value as InferOerType<TSchema> }
  }
