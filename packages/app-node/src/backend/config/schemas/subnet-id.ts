import { z } from "zod"

import { SubnetId } from "../../peer-protocol/types/subnet-id"

// TODO: Enforce valid characters
export const subnetIdSchema = z
  .string()
  .min(2)
  .max(128)
  .refine((_value): _value is SubnetId => true)
