import chalk from "chalk"
import { describe } from "vitest"

import { countLines } from "../functions/count-lines"
import {
  generateDeterminateProgressBar,
  generateIndeterminateProgressBar,
} from "../helpers/progress-bar"

describe("countLines", (test) => {
  test("should return 1 for an empty string", ({ expect }) => {
    const test = ""
    const result = countLines(test, 8)
    expect(result).toBe(1)
  })

  test("should return 2 for a newline character", ({ expect }) => {
    const test = "\n"
    const result = countLines(test, 8)
    expect(result).toBe(2)
  })

  test("should return 2 for a string with a newline character", ({
    expect,
  }) => {
    const test = "a\nb"
    const result = countLines(test, 8)
    expect(result).toBe(2)
  })

  test("should return 3 for a string with two newline characters", ({
    expect,
  }) => {
    const test = "a\nb\nc"
    const result = countLines(test, 8)
    expect(result).toBe(3)
  })

  test("should return 2 for a string with a newline character at the end", ({
    expect,
  }) => {
    const test = "a\n"
    const result = countLines(test, 8)
    expect(result).toBe(2)
  })

  test("should return 1 for a string that barely fits the column width", ({
    expect,
  }) => {
    const test = "a\n"
    const result = countLines(test, 8)
    expect(result).toBe(2)
  })

  test("should return 2 for a string that barely doesn't fit the column width", ({
    expect,
  }) => {
    const test = "a".repeat(9)
    const result = countLines(test, 8)
    expect(result).toBe(2)
  })

  test("should return 2 for a string that is 1.5 times column width long", ({
    expect,
  }) => {
    const test = "a".repeat(12)
    const result = countLines(test, 8)
    expect(result).toBe(2)
  })

  test("should return 2 for a string that is 2 times column width long", ({
    expect,
  }) => {
    const test = "a".repeat(16)
    const result = countLines(test, 8)
    expect(result).toBe(2)
  })

  test("should return 3 for a string that is just over 2 times column width long", ({
    expect,
  }) => {
    const test = "a".repeat(17)
    const result = countLines(test, 8)
    expect(result).toBe(3)
  })

  test("should return 3 for a string that is 2.5 times column width long", ({
    expect,
  }) => {
    const test = "a".repeat(20)
    const result = countLines(test, 8)
    expect(result).toBe(3)
  })

  test("should ignore ansi colors", ({ expect }) => {
    const test = chalk.blue("a").repeat(8)
    const result = countLines(test, 8)
    expect(result).toBe(1)
  })

  test("should ignore rgb colors", ({ expect }) => {
    const test = chalk.rgb(100, 110, 120)("a").repeat(8)
    const result = countLines(test, 8)
    expect(result).toBe(1)
  })

  test("should correctly measure determinate progress bar", ({ expect }) => {
    const test = generateDeterminateProgressBar(0.75, 80)
    const result = countLines(test, 80)
    expect(result).toBe(1)
  })

  test("should correctly measure indeterminate progress bar", ({ expect }) => {
    const test = generateIndeterminateProgressBar(0, 80)
    const result = countLines(test, 80)
    expect(result).toBe(1)
  })
})
