import { z } from "zod"

export const subnetConfigSchema = z.array(
  z.object({
    id: z.string(),
    config: z.record(z.string(), z.unknown()),
    bootstrapNodes: z.array(
      z.object({
        nodeId: z.string(),
        url: z.string(),
        alias: z.string(),
        nodePublicKey: z.string(),
      })
    ),
    initialPeers: z
      .array(
        z.object({
          nodeId: z.string(),
          url: z.string(),
          nodePublicKey: z.string(),
        })
      )
      .optional(),
  })
)

export type SubnetConfig = z.infer<typeof subnetConfigSchema>
