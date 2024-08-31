import { z } from "zod"

export const walletSchema = z.object({
  id: z.string(),
  publicName: z.string().optional(),
  assetCode: z.string(),
  assetScale: z.number().int().min(0).max(255),
  authServer: z.string(),
  resourceServer: z.string(),
})
