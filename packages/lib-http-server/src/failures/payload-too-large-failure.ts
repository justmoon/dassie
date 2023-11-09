import { DefaultHttpFailure } from "./default-http-failure"

export class PayloadTooLargeFailure extends DefaultHttpFailure {
  readonly name = "PayloadTooLargeError"
  readonly statusCode = 413
}
