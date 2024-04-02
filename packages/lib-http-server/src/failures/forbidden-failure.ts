import { DefaultHttpFailure } from "./default-http-failure"

export class ForbiddenFailure extends DefaultHttpFailure {
  readonly name = "ForbiddenFailure"
  readonly statusCode = 403
}
