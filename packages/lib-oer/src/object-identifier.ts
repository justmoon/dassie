import { assertDefined } from "@xen-ilp/lib-type-utils"

import { OerType } from "./base-type"
import {
  parseBase128,
  predictBase128Length,
  serializeBase128,
} from "./utils/base-128"
import { ParseError, SerializeError } from "./utils/errors"
import {
  parseLengthPrefix,
  predictLengthPrefixLength,
  serializeLengthPrefix,
} from "./utils/length-prefix"
import type { ParseContext, SerializeContext } from "./utils/parse"

export class OerObjectIdentifier extends OerType<string> {
  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array } = context
    const result = parseLengthPrefix(context, offset)

    if (result instanceof ParseError) {
      return result
    }

    const [length, lengthOfLength] = result

    if (length === 0) {
      return new ParseError(
        "object identifier of length zero is invalid",
        uint8Array,
        offset
      )
    }

    const subidentifiers: bigint[] = []

    let nextOffset = offset + lengthOfLength //?
    while (nextOffset < offset + lengthOfLength + length) {
      const subidentifier = parseBase128(
        context,
        nextOffset,
        offset + lengthOfLength + length
      )
      if (subidentifier instanceof ParseError) {
        return subidentifier
      }
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
      return new SerializeError(
        "object identifier components must be non-negative"
      )
    }

    if (objectIdentifierComponents.length < 2) {
      return new SerializeError(
        "object identifier must have at least two components"
      )
    }

    if (
      objectIdentifierComponents[0]! > 2 ||
      objectIdentifierComponents[0]! < 0
    ) {
      return new SerializeError(
        "object identifier first component must be in the range of 0..2"
      )
    }

    if (
      objectIdentifierComponents[0]! < 2 &&
      objectIdentifierComponents[1]! >= 40
    ) {
      return new SerializeError(
        "object identifier second component must be in the range of 0..39 when first component is 0 or 1"
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
      if (subidentifierLength instanceof SerializeError) {
        return subidentifierLength
      }

      length += subidentifierLength
    }

    const lengthOfLengthPrefix = predictLengthPrefixLength(length)

    if (lengthOfLengthPrefix instanceof SerializeError) {
      return lengthOfLengthPrefix
    }

    return [
      ({ uint8Array }: SerializeContext, offset: number) => {
        const lengthPrefixSerializeResult = serializeLengthPrefix(
          length,
          uint8Array,
          offset
        )

        if (lengthPrefixSerializeResult instanceof SerializeError) {
          return lengthPrefixSerializeResult
        }

        let currentOffset = (offset = lengthOfLengthPrefix)
        for (const subidentifier of subidentifiers) {
          const subidentifierSerializeResult = serializeBase128(
            subidentifier,
            uint8Array,
            currentOffset
          )

          if (subidentifierSerializeResult instanceof SerializeError) {
            return subidentifierSerializeResult
          }

          currentOffset += subidentifierSerializeResult
        }
        return
      },
      lengthOfLengthPrefix + length,
    ] as const
  }
}

export const objectIdentifier = () => {
  return new OerObjectIdentifier()
}
