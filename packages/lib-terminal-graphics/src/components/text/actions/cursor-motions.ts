import { TextState } from ".."
import {
  leftGrapheme,
  leftWord,
  rightGrapheme,
  rightWord,
} from "../../../helpers/string-offsets"

export const cursorLeft = (state: TextState): TextState => ({
  ...state,
  cursor: leftGrapheme(state.value, state.cursor),
})

export const cursorRight = (state: TextState): TextState => ({
  ...state,
  cursor: rightGrapheme(state.value, state.cursor),
})

export const cursorWordLeft = (state: TextState): TextState => ({
  ...state,
  cursor: leftWord(state.value, state.cursor),
})

export const cursorWordRight = (state: TextState): TextState => ({
  ...state,
  cursor: rightWord(state.value, state.cursor),
})

export const cursorHome = (state: TextState): TextState => ({
  ...state,
  cursor: 0,
})

export const cursorEnd = (state: TextState): TextState => ({
  ...state,
  cursor: state.value.length,
})
