import { z } from "zod"

import { RealmType } from "../../../../config/environment-config"
import { VALID_REALMS } from "../../../../constants/general"

export const xrplWalletSchema = z.object({
  realm: z.enum(VALID_REALMS),
  seed: z.string(),
  address: z.string(),
})

export type XrplWallet<TRealm extends RealmType = RealmType> = z.infer<
  typeof xrplWalletSchema
> & { realm: TRealm }
