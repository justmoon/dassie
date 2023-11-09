import { DefaultHttpFailure } from "./default-http-failure"

export class BadRequestFailure extends DefaultHttpFailure {
  readonly name = "BadRequestFailure"
  readonly statusCode = 400
}
