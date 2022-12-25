import { ia5String, sequence, uint8Number } from "@dassie/lib-oer"

export const ildcpResponseSchema = sequence({
  address: ia5String(),
  assetScale: uint8Number(),
  assetCode: ia5String(),
})
