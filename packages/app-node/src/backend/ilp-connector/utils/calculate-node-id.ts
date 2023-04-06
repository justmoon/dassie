import { createHash } from "node:crypto"

import { objectIdentifier, octetString, sequence } from "@dassie/lib-oer"

import { dassieNodeFingerprintV1Id } from "../../constants/object-identifiers"

const nodeIdHashContentSchema = sequence({
  versionPrefix: objectIdentifier().constant(dassieNodeFingerprintV1Id),
  nodePublicKey: octetString(),
})

export const calculateNodeId = (nodePublicKey: Uint8Array): string => {
  const nodeIdHashContent = nodeIdHashContentSchema.serializeOrThrow({
    nodePublicKey,
  })

  const hash = createHash("sha256")
  hash.update(nodeIdHashContent)

  return hash.digest("base64url")
}
