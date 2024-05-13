export const leftGrapheme = (text: string, offset: number): number => {
  const segmenter = new Intl.Segmenter("en", { granularity: "grapheme" })
  const segments = segmenter.segment(text)

  let previousPosition = 0
  for (const segment of segments) {
    // If the current segment ends at or after the cursor position, go to the start of it
    if (segment.index + segment.segment.length >= offset) {
      return segment.index
    }

    previousPosition = segment.index
  }

  // If the cursor position is at the end of the string, return the start of the last grapheme
  if (offset >= text.length) {
    return previousPosition
  }

  // If no segments were found before the cursor position, return 0
  return 0
}

export const rightGrapheme = (text: string, offset: number): number => {
  const segmenter = new Intl.Segmenter("en", { granularity: "grapheme" })
  const segments = segmenter.segment(text)

  for (const segment of segments) {
    // If the current segment ends after the cursor position, return the end of it
    if (segment.index + segment.segment.length > offset) {
      return segment.index + segment.segment.length
    }
  }

  // If no segments were found after the cursor position, return the end of the string
  return text.length
}

const UTF16_SURROGATE_THRESHOLD = 0x1_00_00

export const leftCodepoint = (text: string, offset: number): number => {
  if (offset <= 0) {
    // If the cursor is at or before the start of the string, return 0
    return 0
  } else if (
    offset >= 2 &&
    text.length >= 2 &&
    text.codePointAt(offset - 2)! >= UTF16_SURROGATE_THRESHOLD
  ) {
    // If the codepoint before the cursor is a surrogate pair, return the cursor position - 2
    return offset - 2
  } else {
    // Otherwise, return the cursor position - 1
    return offset - 1
  }
}

export const rightCodepoint = (text: string, offset: number): number => {
  return Math.min(
    text.length,
    (text.codePointAt(offset) ?? 0) >= UTF16_SURROGATE_THRESHOLD ?
      offset + 2
    : offset + 1,
  )
}

export const leftWord = (text: string, offset: number): number => {
  const segmenter = new Intl.Segmenter("en", { granularity: "word" })
  const segments = segmenter.segment(text)

  let previousPosition = 0
  for (const segment of segments) {
    // If the current segment ends at or after the cursor position, go to the start of it
    if (segment.index + segment.segment.length >= offset) {
      // Unless it is a whitespace segment, in which case go to the start of the previous segment
      if (segment.segment.trim().length === 0) {
        return previousPosition
      }

      return segment.index
    }

    previousPosition = segment.index
  }

  // If the cursor position is at the end of the string, return the start of the last grapheme
  if (offset >= text.length) {
    return previousPosition
  }

  // If no segments were found before the cursor position, return 0
  return 0
}

export const rightWord = (text: string, offset: number): number => {
  const segmenter = new Intl.Segmenter("en", { granularity: "word" })
  const segments = segmenter.segment(text)

  for (const segment of segments) {
    // If the current segment ends after the cursor position, return the end of it
    if (
      segment.index + segment.segment.length > offset &&
      // Unless it is a whitespace segment, in which case go to the end of the next segment
      segment.segment.trim().length > 0
    ) {
      return segment.index + segment.segment.length
    }
  }

  // If no segments were found after the cursor position, return the end of the string
  return text.length
}

if (import.meta.vitest) {
  const { describe, it } = import.meta.vitest

  describe("leftGrapheme", () => {
    it("goes back one ascii character", ({ expect }) => {
      expect(leftGrapheme("abc", 3)).toBe(2)
    })

    it("goes back one surrogate pair", ({ expect }) => {
      expect(leftGrapheme("foo😍", 5)).toBe(3)
    })

    it("goes back one grapheme containing a zero-width join", ({ expect }) => {
      expect(leftGrapheme("foo👨🏻", 7)).toBe(3)
    })

    it("goes back one grapheme containing multiple zero-width joins", ({
      expect,
    }) => {
      expect(leftGrapheme("foo👨‍👩‍👧‍👦", 14)).toBe(3)
    })

    it("goes back to the start of a surrogate pair if it is in the middle of one", ({
      expect,
    }) => {
      expect(leftGrapheme("foo😍", 4)).toBe(3)
    })

    it("goes back to the start of grapheme if it is in the middle of one", ({
      expect,
    }) => {
      expect(leftGrapheme("foo👨‍👩‍👧‍👦", 13)).toBe(3)
    })
  })

  describe("rightGrapheme", () => {
    it("goes forward one ascii character", ({ expect }) => {
      expect(rightGrapheme("abcdef", 4)).toBe(5)
    })

    it("goes forward one surrogate pair", ({ expect }) => {
      expect(rightGrapheme("foo😍aaa", 3)).toBe(5)
    })

    it("goes forward one grapheme containing a zero-width join", ({
      expect,
    }) => {
      expect(rightGrapheme("foo👨🏻aaa", 3)).toBe(7)
    })

    it("goes forward one grapheme containing multiple zero-width joins", ({
      expect,
    }) => {
      expect(rightGrapheme("foo👨‍👩‍👧‍👦aaa", 3)).toBe(14)
    })

    it("goes forward to the end of a surrogate pair if it is in the middle of one", ({
      expect,
    }) => {
      expect(rightGrapheme("foo😍aaa", 4)).toBe(5)
    })

    it("goes forward to the end of grapheme if it is in the middle of one", ({
      expect,
    }) => {
      expect(rightGrapheme("foo👨‍👩‍👧‍👦aaa", 4)).toBe(14)
    })
  })

  describe("leftCodepoint", () => {
    it("goes back one ascii character", ({ expect }) => {
      expect(leftCodepoint("abc", 3)).toBe(2)
    })

    it("goes back one surrogate pair", ({ expect }) => {
      expect(leftCodepoint("foo😍", 5)).toBe(3)
    })

    it("goes back one codepoint in a grapheme containing a zero-width join", ({
      expect,
    }) => {
      expect(leftCodepoint("foo👨🏻", 7)).toBe(5)
    })

    it("goes back one codepoint in a grapheme containing multiple zero-width joins", ({
      expect,
    }) => {
      expect(leftCodepoint("foo👨‍👩‍👧‍👦", 14)).toBe(12)
    })

    it("goes back to the start of a surrogate pair if it is in the middle of one", ({
      expect,
    }) => {
      expect(leftCodepoint("foo😍", 4)).toBe(3)
    })

    it("goes back to the beginning of the codepoint in a grapheme using zero-width joins if it is in the middle of a surrogate pair", ({
      expect,
    }) => {
      expect(leftCodepoint("foo👨‍👩‍👧‍👦", 7)).toBe(6)
    })

    it("goes back two characters (one codepoint) in a grapheme using zero-width joins if it is at the end of surrogate pair", ({
      expect,
    }) => {
      expect(leftCodepoint("foo👨‍👩‍👧‍👦", 11)).toBe(9)
    })

    it("goes back one character in a grapheme using zero-width joins if it is at the end of zero-width join", ({
      expect,
    }) => {
      expect(leftCodepoint("foo👨‍👩‍👧‍👦", 6)).toBe(5)
    })
  })

  describe("rightCodepoint", () => {
    it("goes forward one ascii character", ({ expect }) => {
      expect(rightCodepoint("abcdef", 4)).toBe(5)
    })

    it("goes forward one surrogate pair", ({ expect }) => {
      expect(rightCodepoint("foo😍aaa", 3)).toBe(5)
    })

    it("goes forward one codepoint in a grapheme containing a zero-width join", ({
      expect,
    }) => {
      expect(rightCodepoint("foo👨🏻aaa", 3)).toBe(5)
    })

    it("goes forward one codepoint in a grapheme containing multiple zero-width joins", ({
      expect,
    }) => {
      expect(rightCodepoint("foo👨‍👩‍👧‍👦aaa", 3)).toBe(5)
    })

    it("goes forward to the end of a surrogate pair if it is in the middle of one", ({
      expect,
    }) => {
      expect(rightCodepoint("foo😍aaa", 4)).toBe(5)
    })

    it("goes forward to the end of a surrogate pair if it is in the middle of one inside of a grapheme with zero-width joins", ({
      expect,
    }) => {
      expect(rightCodepoint("foo👨‍👩‍👧‍👦aaa", 7)).toBe(8)
    })
  })

  describe("leftWord", () => {
    it("goes back to the beginning of the word if it is in the middle of one", ({
      expect,
    }) => {
      expect(leftWord("foo bar baz", 6)).toBe(4)
    })

    it("goes back to the beginning of the word if it is at the end of one", ({
      expect,
    }) => {
      expect(leftWord("foo bar baz", 7)).toBe(4)
    })

    it("goes back to the beginning of the word if it is at the start of the next one", ({
      expect,
    }) => {
      expect(leftWord("foo bar baz", 8)).toBe(4)
    })

    it("goes back to the beginning of the word if it is in whitespace after it", ({
      expect,
    }) => {
      expect(leftWord("foo    bar    baz", 12)).toBe(7)
    })

    it.skip("behaves in a line of code the same way VSCode does", ({
      expect,
    }) => {
      const code = 'const foo = bar("test", { foo: true })'

      const actual = Array.from({ length: code.length + 1 }, (_, index) =>
        leftWord(code, index),
      )

      expect(actual).toEqual([
        0, 0, 0, 0, 0, 0, 0, 6, 6, 6, 6, 10, 10, 12, 12, 12, 15, 15, 17, 17, 17,
        17, 21, 21, 21, 24, 24, 26, 26, 26, 29, 29, 31, 31, 31, 31, 31, 36, 36,
      ])
    })
  })

  describe("rightWord", () => {
    it("goes forward to the end of the word if it is in the middle of one", ({
      expect,
    }) => {
      expect(rightWord("foo bar baz", 5)).toBe(7)
    })

    it("goes forward to the end of the word if it is at the start of one", ({
      expect,
    }) => {
      expect(rightWord("foo bar baz", 4)).toBe(7)
    })

    it("goes forward to the end of the word if it is at the end of the previous one", ({
      expect,
    }) => {
      expect(rightWord("foo bar baz", 3)).toBe(7)
    })

    it("goes forward to the end of the word if it is in whitespace before it", ({
      expect,
    }) => {
      expect(rightWord("foo    bar    baz", 4)).toBe(10)
    })

    it.skip("behaves in a line of code the same way VSCode does", ({
      expect,
    }) => {
      const code = 'const foo = bar("test", { foo: true })'

      const actual = Array.from({ length: code.length + 1 }, (_, index) =>
        rightWord(code, index),
      )

      expect(actual).toEqual([
        5, 5, 5, 5, 5, 9, 9, 9, 9, 11, 11, 15, 15, 15, 15, 17, 17, 21, 21, 21,
        21, 23, 23, 25, 25, 29, 29, 29, 29, 30, 35, 35, 35, 35, 35, 38, 38, 38,
        38,
      ])
    })
  })
}
