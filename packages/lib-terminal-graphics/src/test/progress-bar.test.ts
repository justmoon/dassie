import { describe } from "vitest"

import { generateDeterminateProgressBar } from "../helpers/progress-bar"

describe("progress bar", (test) => {
  test("should render a single bar out of 4 when progress is 25%", ({
    expect,
  }) => {
    const result = generateDeterminateProgressBar(0, 0.25, 4)
    expect(result).toMatchInlineSnapshot(`"█╍╍ "`)
  })

  test("should render a half bar out of 4 when progress is 12.5%", ({
    expect,
  }) => {
    const result = generateDeterminateProgressBar(0, 0.125, 4)
    expect(result).toMatchInlineSnapshot(`"▋╍╍ "`)
  })

  test("should render a quarter bar out of 4 when progress is 6.25%", ({
    expect,
  }) => {
    const result = generateDeterminateProgressBar(0, 0.0625, 4)
    expect(result).toMatchInlineSnapshot(`"▍╍╍ "`)
  })

  test("should render an eighth bar out of 4 when progress is 3.125%", ({
    expect,
  }) => {
    const result = generateDeterminateProgressBar(0, 0.031_25, 4)
    expect(result).toMatchInlineSnapshot(`"▎╍╍ "`)
  })

  test("should render seven eighths out of four when progress is 21.8749%", ({
    expect,
  }) => {
    const result = generateDeterminateProgressBar(0, 0.218_749, 4)
    expect(result).toMatchInlineSnapshot(`"▉╍╍ "`)
  })

  test("should render one and a half bar out of 4 when progress is 37.5%", ({
    expect,
  }) => {
    const result = generateDeterminateProgressBar(0, 0.375, 4)
    expect(result).toMatchInlineSnapshot(`"█▋╍ "`)
  })

  test("should render four full bars out of four when progress is 100%", ({
    expect,
  }) => {
    const result = generateDeterminateProgressBar(0, 1, 4)
    expect(result).toMatchInlineSnapshot(`"████"`)
  })
})
