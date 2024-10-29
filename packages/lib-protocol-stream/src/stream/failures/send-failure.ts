import { Failure } from "@dassie/lib-type-utils"

export class SendFailure extends Failure {
  readonly name = "SendFailure"

  constructor(readonly reason: string) {
    super()
  }
}

export const SEND_TIMEOUT_FAILURE = new SendFailure("Sending timed out")

export const SEND_INCOMPLETE_FAILURE = new SendFailure(
  "Send loop exited without completing send",
)
