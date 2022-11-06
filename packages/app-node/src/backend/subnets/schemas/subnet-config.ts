import { z } from "zod"

export const subnetConfigSchema = z.array(
  z.object({
    id: z.string(),
    config: z.record(z.string(), z.unknown()),
  })
)

export type SubnetConfig = z.infer<typeof subnetConfigSchema>
