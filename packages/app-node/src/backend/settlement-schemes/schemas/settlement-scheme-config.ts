import { ReadonlyDeep } from "type-fest"
import { z } from "zod"

import { nodeIdSchema } from "../../config/schemas/node-id"
import { settlementSchemeIdSchema } from "../../config/schemas/settlement-scheme-id"

export const settlementSchemeConfigSchema = z.array(
  z.object({
    id: settlementSchemeIdSchema,
    config: z.record(z.string(), z.unknown()),
    bootstrapNodes: z.array(
      z.object({
        nodeId: nodeIdSchema,
        url: z.string(),
        alias: z.string(),
        nodePublicKey: z.string(),
      })
    ),
    initialPeers: z
      .array(
        z.object({
          nodeId: nodeIdSchema,
          url: z.string(),
          nodePublicKey: z.string(),
        })
      )
      .optional(),
  })
)

export type SettlementSchemeConfig = ReadonlyDeep<
  z.infer<typeof settlementSchemeConfigSchema>
>
