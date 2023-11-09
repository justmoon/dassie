import { DefaultHttpFailure } from "./default-http-failure"

export class NotAcceptableFailure extends DefaultHttpFailure {
  readonly name = "NotAcceptableError"
  readonly statusCode = 406
}
