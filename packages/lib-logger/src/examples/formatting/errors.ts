import { showComparison } from "./util/compare"

const CustomError = class CustomError extends Error {
  constructor(message: string) {
    super(message)
    this.name = "CustomError"
  }
}

const errors = {
  basic: new Error("Bad thing happened"),
  stackless: (() => {
    const error = new Error("Bad thing happened")
    delete error.stack
    return error
  })(),
  custom: new CustomError("Bad thing happened"),
}

showComparison(errors, "Errors")
