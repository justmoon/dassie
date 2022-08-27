export class BadRequestError extends Error {
  readonly statusCode = 400
  readonly isOperational = true

  constructor(message: string) {
    super(message)
    this.name = "BadRequestError"
  }
}

export class NotAcceptableError extends Error {
  readonly statusCode = 406
  readonly isOperational = true

  constructor(message: string) {
    super(message)
    this.name = "NotAcceptableError"
  }
}

export class PayloadTooLargeError extends Error {
  readonly statusCode = 413
  readonly isOperational = true

  constructor(message: string) {
    super(message)
    this.name = "PayloadTooLargeError"
  }
}

export class UnsupportedMediaTypeError extends Error {
  readonly statusCode = 415
  readonly isOperational = true

  constructor(message: string) {
    super(message)
    this.name = "UnsupportedMediaTypeError"
  }
}
