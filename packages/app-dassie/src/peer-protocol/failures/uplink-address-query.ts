import { Failure } from "@dassie/lib-type-utils"

export default class UplinkAddressQueryFailure extends Failure {
  readonly name = "UplinkAddressQueryFailure"

  constructor(public readonly message: string) {
    super()
  }
}
