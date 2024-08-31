import { describe, test } from "vitest"

import { isFailure } from "@dassie/lib-type-utils"

import { allocateAmounts } from "../math/allocate-amounts"

describe("allocateAmounts", () => {
  test("should split 30 units evenly amongst three streams", ({ expect }) => {
    const streamReceiveList = [
      {
        streamId: 1,
        shares: 10n,
        receiveMaximum: 30n,
      },
      {
        streamId: 2,
        shares: 10n,
        receiveMaximum: 30n,
      },
      {
        streamId: 3,
        shares: 10n,
        receiveMaximum: 30n,
      },
    ]

    const allocationList = allocateAmounts(30n, 90n, streamReceiveList)

    expect(allocationList).toMatchInlineSnapshot(`
      [
        {
          "amount": 30n,
          "streamId": 1,
        },
        {
          "amount": 30n,
          "streamId": 2,
        },
        {
          "amount": 30n,
          "streamId": 3,
        },
      ]
    `)
  })

  test("should divide excess funds evenly amongst streams with capacity", ({
    expect,
  }) => {
    const streamReceiveList = [
      {
        streamId: 1,
        shares: 1000n,
        receiveMaximum: 10n,
      },
      {
        streamId: 2,
        shares: 10n,
        receiveMaximum: 1000n,
      },
      {
        streamId: 3,
        shares: 10n,
        receiveMaximum: 1000n,
      },
    ]

    const allocationList = allocateAmounts(30n, 510n, streamReceiveList)

    expect(allocationList).toMatchInlineSnapshot(`
      [
        {
          "amount": 10n,
          "streamId": 1,
        },
        {
          "amount": 250n,
          "streamId": 2,
        },
        {
          "amount": 250n,
          "streamId": 3,
        },
      ]
    `)
  })

  test("should allocate remainder even when it would normally round to zero", ({
    expect,
  }) => {
    const streamReceiveList = [
      { streamId: 1, shares: 10n, receiveMaximum: 1000n },
      { streamId: 2, shares: 10n, receiveMaximum: 1000n },
      { streamId: 3, shares: 10n, receiveMaximum: 1000n },
    ]

    const allocationList = allocateAmounts(30n, 100n, streamReceiveList)

    expect(allocationList).toMatchInlineSnapshot(`
      [
        {
          "amount": 34n,
          "streamId": 1,
        },
        {
          "amount": 33n,
          "streamId": 2,
        },
        {
          "amount": 33n,
          "streamId": 3,
        },
      ]
    `)
  })

  test("should reject packets when the amount is too large", ({ expect }) => {
    const streamReceiveList = [
      { streamId: 1, shares: 10n, receiveMaximum: 1000n },
      { streamId: 2, shares: 10n, receiveMaximum: 1000n },
      { streamId: 3, shares: 10n, receiveMaximum: 1000n },
    ]

    const allocationList = allocateAmounts(30n, 3001n, streamReceiveList)

    expect(isFailure(allocationList)).toBe(true)
    expect(isFailure(allocationList) && allocationList.name).toBe(
      "AmountExceedsMaximumReceiveAmountFailure",
    )
  })
})
