import { Simplify } from "type-fest"

import {
  type Infer,
  type ParseFailure,
  ia5String,
  sequence,
  uint8Number,
} from "@dassie/lib-oer"
import { isFailure } from "@dassie/lib-type-utils"

export const ildcpResponseSchema = sequence({
  address: ia5String(),
  assetScale: uint8Number(),
  assetCode: ia5String(),
})

export type IldcpResponse = Simplify<Infer<typeof ildcpResponseSchema>>

export function parseIldcpResponse(
  data: Uint8Array,
): IldcpResponse | ParseFailure {
  const ildcpResponse = ildcpResponseSchema.parse(data)

  if (isFailure(ildcpResponse)) {
    return ildcpResponse
  }

  return ildcpResponse.value
}

export function serializeIldcpResponse(ildcpResponse: IldcpResponse) {
  return ildcpResponseSchema.serializeOrThrow(ildcpResponse)
}
