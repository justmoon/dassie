import { octetString, sequence } from "@dassie/lib-oer"

export const settlementProofSchema = sequence({
  transactionHash: octetString(32),
})
