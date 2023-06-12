import { ReadonlyDeep } from "type-fest"
import { z } from "zod"

import { nodeIdSchema } from "../../config/schemas/node-id"
import { subnetIdSchema } from "../../config/schemas/subnet-id"

export const subnetConfigSchema = z.array(
  z.object({
    id: subnetIdSchema,
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

export type SubnetConfig = ReadonlyDeep<z.infer<typeof subnetConfigSchema>>
