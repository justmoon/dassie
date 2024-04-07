import { DefaultHttpFailure } from "./default-http-failure"

export class MethodNotAllowedFailure extends DefaultHttpFailure {
  readonly name = "MethodNotAllowedFailure"
  readonly statusCode = 405
}
