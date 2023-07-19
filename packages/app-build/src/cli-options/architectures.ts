import { Type } from "cmd-ts"
import { z } from "zod"

import { Architecture } from "../constants/architectures"

const ArchitectureSchema = z.array(z.enum(["x64", "arm64", "armv7l"]))

export const ArchitecturesParameter: Type<string, readonly Architecture[]> = {
  from: (value) => {
    const result = ArchitectureSchema.parse(value.split(","))

    return Promise.resolve(result)
  },
}
