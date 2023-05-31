import blink from "./ansi-theme.module.css"

/**
 * Dracula theme.
 *
 * @see https://draculatheme.com/
 */
export const ANSI_COLORS = {
  "ansi-black": "#262626",
  "ansi-red": "#e356a7",
  "ansi-green": "#42e66c",
  "ansi-yellow": "#e4f34a",
  "ansi-blue": "#9b6bdf",
  "ansi-magenta": "#e64747",
  "ansi-cyan": "#75d7ec",
  "ansi-white": "#efa554",
  "ansi-bright-black": "#7a7a7a",
  "ansi-bright-red": "#ff79c6",
  "ansi-bright-green": "#50fa7b",
  "ansi-bright-yellow": "#f1fa8c",
  "ansi-bright-blue": "#bd93f9",
  "ansi-bright-magenta": "#ff5555",
  "ansi-bright-cyan": "#8be9fd",
  "ansi-bright-white": "#ffb86c",
} as const

export const ANSI_DECORATIONS = {
  bold: "font-bold",
  italic: "italic",
  underline: "underline",
  blink,
  dim: "opacity-50",
  reverse: undefined, // Reversing the colors is already being applied by Anser
  hidden: "opacity-0",
  strikethrough: "line-through",
} as const
