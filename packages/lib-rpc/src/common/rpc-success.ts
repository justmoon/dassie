export class RpcSuccess<T = unknown> {
  name = "RpcSuccess"

  constructor(public readonly data: T) {}
}
