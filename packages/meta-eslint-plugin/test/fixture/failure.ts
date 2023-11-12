const FAILURE_UNIQUE_KEY = "dassie.failure"

export abstract class Failure {
  [FAILURE_UNIQUE_KEY] = true as const
  abstract readonly name: string
}

export declare const isFailure: <T>(value: T) => value is Extract<T, Failure>

export declare const findFailure: <T extends readonly unknown[]>(
  values: T,
) => { [K in keyof T]: Extract<T[K], Failure> }
