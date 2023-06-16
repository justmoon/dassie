import { TextState } from ".."
import {
  leftCodepoint,
  leftWord,
  rightGrapheme,
  rightWord,
} from "../../../helpers/string-offsets"

export const insertString =
  (text: string) =>
  (state: TextState): TextState => ({
    ...state,
    value:
      state.value.slice(0, state.cursor) +
      text +
      state.value.slice(state.cursor),
    cursor: state.cursor + text.length,
  })

export const deleteLeft = (state: TextState): TextState => {
  // When deleting left, we delete by codepoint, not by grapheme. This mirrors
  // what most text editors do.
  const left = leftCodepoint(state.value, state.cursor)
  return {
    ...state,
    value: state.value.slice(0, left) + state.value.slice(state.cursor),
    cursor: left,
  }
}

export const deleteRight = (state: TextState): TextState => {
  const right = rightGrapheme(state.value, state.cursor)
  return {
    ...state,
    value: state.value.slice(0, state.cursor) + state.value.slice(right),
  }
}

export const deleteWordLeft = (state: TextState): TextState => {
  const left = leftWord(state.value, state.cursor)
  return {
    ...state,
    value: state.value.slice(0, left) + state.value.slice(state.cursor),
    cursor: left,
  }
}

export const deleteWordRight = (state: TextState): TextState => {
  const right = rightWord(state.value, state.cursor)
  return {
    ...state,
    value: state.value.slice(0, state.cursor) + state.value.slice(right),
  }
}

export const deleteLineLeft = (state: TextState): TextState => ({
  ...state,
  value: state.value.slice(state.cursor),
  cursor: 0,
})

export const deleteLineRight = (state: TextState): TextState => ({
  ...state,
  value: state.value.slice(0, state.cursor),
})
