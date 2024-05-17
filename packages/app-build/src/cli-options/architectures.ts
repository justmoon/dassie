import { Type } from "cmd-ts"
import { z } from "zod"

import {
  Architecture,
  SUPPORTED_ARCHITECTURES,
} from "../constants/architectures"

const ArchitectureSchema = z.array(z.enum(SUPPORTED_ARCHITECTURES))

export const ArchitecturesParameter: Type<string, readonly Architecture[]> = {
  from: (value) => {
    const result = ArchitectureSchema.parse(value.split(","))

    return Promise.resolve(result)
  },
}
