import { RippledError } from "xrpl"

import { isObject } from "@dassie/lib-type-utils"

export const isRippledErrorWithId = (
  error: unknown,
  errorId: string,
): boolean =>
  error instanceof RippledError &&
  isObject(error.data) &&
  "error" in error.data &&
  error.data["error"] === errorId
