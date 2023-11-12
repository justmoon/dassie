import { assertDefined, isFailure } from "@dassie/lib-type-utils"

import { OerType } from "./base-type"
import {
  parseBase128,
  predictBase128Length,
  serializeBase128,
} from "./utils/base-128"
import { ParseFailure, SerializeFailure } from "./utils/failures"
import {
  parseLengthPrefix,
  predictLengthPrefixLength,
  serializeLengthPrefix,
} from "./utils/length-prefix"
import type { ParseContext, SerializeContext } from "./utils/parse"

export class OerObjectIdentifier extends OerType<string> {
  clone() {
    return new OerObjectIdentifier()
  }

  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array } = context

    const result = parseLengthPrefix(context, offset)
    if (isFailure(result)) return result

    const [length, lengthOfLength] = result

    if (length === 0) {
      return new ParseFailure(
        "object identifier of length zero is invalid",
        uint8Array,
        offset,
      )
    }

    const subidentifiers: bigint[] = []

    let nextOffset = offset + lengthOfLength //?
    while (nextOffset < offset + lengthOfLength + length) {
      const subidentifier = parseBase128(
        context,
        nextOffset,
        offset + lengthOfLength + length,
      )
      if (isFailure(subidentifier)) return subidentifier

      subidentifiers.push(subidentifier[0])
      nextOffset += subidentifier[1]
    }

    assertDefined(subidentifiers[0])

    const firstTwo =
      subidentifiers[0] >= 80
        ? ([2, subidentifiers[0] - 80n] as const)
        : ([subidentifiers[0] / 40n, subidentifiers[0] % 40n] as const)

    return [
      [...firstTwo, ...subidentifiers.slice(1)].join("."),
      nextOffset - offset,
    ] as const
  }

  serializeWithContext(input: string) {
    const objectIdentifierComponents = input.split(".").map(BigInt)

    if (objectIdentifierComponents.some((x) => x < 0n)) {
      return new SerializeFailure(
        "object identifier components must be non-negative",
      )
    }

    if (objectIdentifierComponents.length < 2) {
      return new SerializeFailure(
        "object identifier must have at least two components",
      )
    }

    if (
      objectIdentifierComponents[0]! > 2 ||
      objectIdentifierComponents[0]! < 0
    ) {
      return new SerializeFailure(
        "object identifier first component must be in the range of 0..2",
      )
    }

    if (
      objectIdentifierComponents[0]! < 2 &&
      objectIdentifierComponents[1]! >= 40
    ) {
      return new SerializeFailure(
        "object identifier second component must be in the range of 0..39 when first component is 0 or 1",
      )
    }

    const firstSubidentifier =
      objectIdentifierComponents[0]! * 40n + objectIdentifierComponents[1]!

    const subidentifiers = [
      firstSubidentifier,
      ...objectIdentifierComponents.slice(2),
    ]

    let length = 0

    for (const subidentifier of subidentifiers) {
      const subidentifierLength = predictBase128Length(subidentifier)
      if (isFailure(subidentifierLength)) return subidentifierLength

      length += subidentifierLength
    }

    const lengthOfLengthPrefix = predictLengthPrefixLength(length)
    if (isFailure(lengthOfLengthPrefix)) return lengthOfLengthPrefix

    const serializer = ({ uint8Array }: SerializeContext, offset: number) => {
      const lengthPrefixSerializeResult = serializeLengthPrefix(
        length,
        uint8Array,
        offset,
      )

      if (isFailure(lengthPrefixSerializeResult))
        return lengthPrefixSerializeResult

      let currentOffset = (offset = lengthOfLengthPrefix)
      for (const subidentifier of subidentifiers) {
        const subidentifierSerializeResult = serializeBase128(
          subidentifier,
          uint8Array,
          currentOffset,
        )

        if (isFailure(subidentifierSerializeResult))
          return subidentifierSerializeResult

        currentOffset += subidentifierSerializeResult
      }
      return
    }
    serializer.size = lengthOfLengthPrefix + length
    return serializer
  }
}

export const objectIdentifier = () => {
  return new OerObjectIdentifier()
}
