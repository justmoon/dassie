import { DefaultHttpFailure } from "./default-http-failure"

export class NotFoundFailure extends DefaultHttpFailure {
  readonly name = "NotFoundFailure"
  readonly statusCode = 404
}
