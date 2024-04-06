import { z } from "zod"

import { VALID_ROUTE_TYPES } from "./route-type"

export const requestEnvelopeSchema = z.object({
  protocol: z.literal("dassie-rpc-01"),
  type: z.literal("request"),
  routeType: z.enum(VALID_ROUTE_TYPES),
  id: z.string(),
  path: z.array(z.string()),
  input: z.unknown(),
})
export type RequestEnvelope = z.infer<typeof requestEnvelopeSchema>

export const responseEnvelopeSchema = z.object({
  protocol: z.literal("dassie-rpc-01"),
  type: z.literal("response"),
  id: z.string(),
  result: z.union([
    z.object({
      type: z.literal("success"),
      data: z.unknown(),
    }),
    z.object({
      type: z.literal("error"),
      message: z.string(),
    }),
  ]),
})
export type ResponseEnvelope = z.infer<typeof responseEnvelopeSchema>

export const eventEnvelopeSchema = z.object({
  protocol: z.literal("dassie-rpc-01"),
  type: z.literal("event"),
  id: z.string(),
  data: z.array(z.unknown()),
})
export type EventEnvelope = z.infer<typeof eventEnvelopeSchema>

export const cancelEnvelopeSchema = z.object({
  protocol: z.literal("dassie-rpc-01"),
  type: z.literal("cancel"),
  id: z.string(),
})
export type CancelEnvelope = z.infer<typeof cancelEnvelopeSchema>

export const serverEnvelopeSchema = z.union([
  requestEnvelopeSchema,
  cancelEnvelopeSchema,
])
export type ServerEnvelope = z.infer<typeof serverEnvelopeSchema>

export const clientEnvelopeSchema = z.union([
  responseEnvelopeSchema,
  eventEnvelopeSchema,
])
export type ClientEnvelope = z.infer<typeof clientEnvelopeSchema>
