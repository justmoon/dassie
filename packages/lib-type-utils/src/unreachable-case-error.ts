export class UnreachableCaseError extends Error {
  constructor(value: never) {
    super(`Unreachable case: ${String(value)}`)
  }
}
