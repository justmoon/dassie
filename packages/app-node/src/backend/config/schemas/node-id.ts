import { z } from "zod"

import type { NodeId } from "../../peer-protocol/types/node-id"

// TODO: Enforce valid characters
export const nodeIdSchema = z
  .string()
  .min(2)
  .max(45)
  .refine((_value): _value is NodeId => true)
