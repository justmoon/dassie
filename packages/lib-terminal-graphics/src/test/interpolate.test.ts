import { describe } from "vitest"

import { interpolate, interpolateBetweenStops } from "../utils/interpolate"

describe("interpolate", (test) => {
  test("should interpolate between two values", ({ expect }) => {
    const result = interpolate([0], [4], 0.25)
    expect(result).toEqual([1])
  })

  test("should interpolate between two stops", ({ expect }) => {
    const result = interpolateBetweenStops([[0], [4]], 0.25)
    expect(result).toEqual([1])
  })

  test("should interpolate between three stops", ({ expect }) => {
    const result = interpolateBetweenStops([[0], [4], [6]], 0.25)
    expect(result).toEqual([2])
  })

  test("should allow interpolation when targeting the first stop", ({
    expect,
  }) => {
    const result = interpolateBetweenStops([[0], [4], [6]], 0)
    expect(result).toEqual([0])
  })

  test("should allow interpolation when targeting the second stop", ({
    expect,
  }) => {
    const result = interpolateBetweenStops([[0], [4], [6]], 0.5)
    expect(result).toEqual([4])
  })

  test("should allow interpolation when targeting the third stop", ({
    expect,
  }) => {
    const result = interpolateBetweenStops([[0], [4], [6]], 1)
    expect(result).toEqual([6])
  })
})
