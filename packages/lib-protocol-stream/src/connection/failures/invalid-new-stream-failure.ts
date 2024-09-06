import { Failure } from "@dassie/lib-type-utils"

import { ErrorCode } from "../../packets/schema"

export class InvalidNewStreamFailure extends Failure {
  readonly name = "InvalidNewStreamFailure"

  constructor(
    readonly errorCode: number,
    readonly reason: string,
  ) {
    super()
  }
}

export const CLIENT_STREAM_IDS_MUST_BE_ODD_FAILURE =
  new InvalidNewStreamFailure(
    ErrorCode.ProtocolViolation,
    "Invalid stream ID - client stream IDs must be odd",
  )

export const SERVER_STREAM_IDS_MUST_BE_EVEN_FAILURE =
  new InvalidNewStreamFailure(
    ErrorCode.ProtocolViolation,
    "Invalid stream ID - server stream IDs must be even",
  )

export const STREAM_ID_ZERO_FAILURE = new InvalidNewStreamFailure(
  ErrorCode.ProtocolViolation,
  "Invalid stream ID - stream ID must not be zero",
)

export const MAXIMUM_STREAM_ID_EXCEEDED_FAILURE = new InvalidNewStreamFailure(
  ErrorCode.StreamIdError,
  "Invalid stream ID - stream ID is above maximum",
)
