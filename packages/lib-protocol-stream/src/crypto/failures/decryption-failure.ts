import { Failure } from "@dassie/lib-type-utils"

export class DecryptionFailure extends Failure {
  readonly name = "DecryptionFailure"
}

export const DECRYPTION_FAILURE = new DecryptionFailure()
