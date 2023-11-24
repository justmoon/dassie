import { Failure } from "@dassie/lib-type-utils"

import { IlpFailure } from "../types/ilp-failure"

export abstract class BaseIlpFailure extends Failure implements IlpFailure {
  abstract readonly errorCode: string

  constructor(readonly message: string) {
    super()
  }
}
