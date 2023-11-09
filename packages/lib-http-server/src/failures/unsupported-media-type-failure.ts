import { DefaultHttpFailure } from "./default-http-failure"

export class UnsupportedMediaTypeFailure extends DefaultHttpFailure {
  readonly name = "UnsupportedMediaTypeError"
  readonly statusCode = 415
}
