// eslint-disable-next-line unicorn/import-style
import { type ForegroundColorName } from "chalk"

export type UnicodeWithFallback =
  | readonly [unicode: string, ascii: string]
  | string

export type StepStyle = "info" | "warning" | "error" | "success"

export interface PromptTheme {
  stepStyles: {
    [Style in StepStyle]: {
      icon: UnicodeWithFallback
      color: ForegroundColorName
    }
  }
  radio: {
    checked: UnicodeWithFallback
    unchecked: UnicodeWithFallback
  }
  checkbox: {
    checked: UnicodeWithFallback
    unchecked: UnicodeWithFallback
  }
  spinner: UnicodeWithFallback[]
}

export const DEFAULT_THEME = {
  stepStyles: {
    info: {
      icon: "i",
      color: "cyan",
    },
    warning: {
      icon: "",
      color: "yellow",
    },
    error: {
      icon: "!",
      color: "red",
    },
    success: {
      icon: "✓",
      color: "green",
    },
  },
  radio: {
    checked: ["●", "(*)"],
    unchecked: ["◯", "( )"],
  },
  checkbox: {
    checked: ["☑", "[x]"],
    unchecked: ["☐", "[ ]"],
  },
  spinner: ["⣏", "⡟", "⠿", "⢻", "⣹", "⣼", "⣶", "⣧"],
} as const satisfies PromptTheme
