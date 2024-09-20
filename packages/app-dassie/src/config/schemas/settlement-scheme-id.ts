import { z } from "zod"

import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

// TODO: Enforce valid characters
export const settlementSchemeIdSchema = z
  .string()
  .min(2)
  .max(128)
  .refine((_value): _value is SettlementSchemeId => true)
