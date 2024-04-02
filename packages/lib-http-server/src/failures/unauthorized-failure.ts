import { DefaultHttpFailure } from "./default-http-failure"

export class UnauthorizedFailure extends DefaultHttpFailure {
  readonly name = "UnauthorizedFailure"
  readonly statusCode = 401
}
