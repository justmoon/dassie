import { Failure } from "@dassie/lib-type-utils"

export class NoRemoteAddressFailure extends Failure {
  readonly name = "NoRemoteAddressFailure"
}

export const NO_REMOTE_ADDRESS_FAILURE = new NoRemoteAddressFailure()
