import { parse as secureJsonParse } from "secure-json-parse"
import { JsonValue } from "type-fest"
import { concatUint8Arrays, uint8ArrayToString } from "uint8array-extras"
import { infer as InferZodType, ZodTypeAny } from "zod"

import { AnyOerType, Infer as InferOerType } from "@dassie/lib-oer"
import { isFailure } from "@dassie/lib-type-utils"

import { DEFAULT_MAX_BODY_SIZE } from "../constants"
import { BadRequestFailure } from "../failures/bad-request-failure"
import { PayloadTooLargeFailure } from "../failures/payload-too-large-failure"
import type { Middleware } from "../router"

export const parseBodyUint8Array = (async ({ request }) => {
  const body: Uint8Array[] = []
  let dataReceived = 0

  if (!request.body) {
    return { body: new Uint8Array() }
  }

  const reader = request.body.getReader()

  let result = await reader.read()
  while (!result.done) {
    const chunk = result.value
    dataReceived += chunk.length

    if (dataReceived > DEFAULT_MAX_BODY_SIZE) {
      await reader.cancel()

      return new PayloadTooLargeFailure(
        `Payload Too Large, expected body to be less than ${DEFAULT_MAX_BODY_SIZE} bytes`,
      )
    }
    body.push(result.value)
    result = await reader.read()
  }

  return { body: concatUint8Arrays(body) }
}) satisfies Middleware<{}, { body: Uint8Array } | PayloadTooLargeFailure>

export const parseBodyBuffer = (async (context) => {
  const uint8ArrayBody = await parseBodyUint8Array(context)
  if (isFailure(uint8ArrayBody)) return uint8ArrayBody

  return { body: Buffer.from(uint8ArrayBody.body) }
}) satisfies Middleware<{}, { body: Buffer } | PayloadTooLargeFailure>

export const parseBodyUtf8 = (async (context) => {
  const uint8ArrayBody = await parseBodyUint8Array(context)
  if (isFailure(uint8ArrayBody)) return uint8ArrayBody

  return { body: uint8ArrayToString(uint8ArrayBody.body) }
}) satisfies Middleware<{}, { body: string } | PayloadTooLargeFailure>

export const parseJson = (async (context) => {
  const utf8Body = await parseBodyUtf8(context)
  if (isFailure(utf8Body)) return utf8Body

  try {
    return { body: secureJsonParse(utf8Body.body) as JsonValue }
  } catch (error) {
    console.debug?.("request body is not valid JSON", { error })
    return new BadRequestFailure("Invalid HTTP request body")
  }
}) satisfies Middleware<{}, { body: JsonValue } | PayloadTooLargeFailure>

export const parseBodyZod =
  <TSchema extends ZodTypeAny>(
    schema: TSchema,
  ): Middleware<
    {},
    { body: InferZodType<TSchema> } | PayloadTooLargeFailure | BadRequestFailure
  > =>
  async (context) => {
    const jsonBody = await parseJson(context)
    if (isFailure(jsonBody)) return jsonBody

    const result = schema.safeParse(jsonBody.body)

    if (!result.success) {
      console.debug?.("request body does not pass schema", {
        error: result.error,
      })
      return new BadRequestFailure("Invalid HTTP request body")
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return { body: result.data as InferZodType<TSchema> }
  }

export const parseBodyOer =
  <TSchema extends AnyOerType>(
    schema: TSchema,
  ): Middleware<
    {},
    { body: InferOerType<TSchema> } | PayloadTooLargeFailure | BadRequestFailure
  > =>
  async (context) => {
    const uint8ArrayBody = await parseBodyUint8Array(context)
    if (isFailure(uint8ArrayBody)) return uint8ArrayBody

    const result = schema.parse(uint8ArrayBody.body)

    if (isFailure(result)) {
      console.info("request body does not pass schema", {
        error: result,
      })
      return new BadRequestFailure("Invalid HTTP request body")
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return { body: result.value as InferOerType<TSchema> }
  }
