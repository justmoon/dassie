import { OerType } from "./base-type"

const serializer = () => {
  // no-op
}
serializer.size = 0

export class OerEmpty extends OerType<void> {
  clone() {
    return new OerEmpty()
  }

  parseWithContext() {
    return [undefined, 0] as const
  }
  serializeWithContext() {
    return serializer
  }
}

export const empty = () => {
  return new OerEmpty()
}
