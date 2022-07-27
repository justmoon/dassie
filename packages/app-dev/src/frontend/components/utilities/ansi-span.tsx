import Anser from "anser"
import { useMemo } from "react"

import blink from "./ansi-span.module.css"

export interface AnsiSpanProperties {
  content: string
}

/**
 * Dracula theme.
 *
 * @see https://draculatheme.com/
 */
const ANSI_COLORS = {
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

const ANSI_DECORATIONS = {
  bold: "font-bold",
  italic: "italic",
  underline: "underline",
  blink,
  dim: "opacity-50",
  reverse: undefined, // Reversing the colors is already being applied by Anser
  hidden: "opacity-0",
  strikethrough: "line-through",
} as const

export const AnsiSpan = ({ content }: AnsiSpanProperties) => {
  const decoded = useMemo(() => {
    return Anser.ansiToJson(content, {
      json: true,
      use_classes: true,
      remove_empty: true,
    })
  }, [content])

  return (
    <>
      {decoded.map((span, index) => (
        <span
          key={index}
          style={{
            color: span.fg
              ? ANSI_COLORS[span.fg as keyof typeof ANSI_COLORS]
              : undefined,
            backgroundColor: span.bg
              ? ANSI_COLORS[span.bg as keyof typeof ANSI_COLORS]
              : undefined,
          }}
          className={span.decorations
            .map((decoration) => ANSI_DECORATIONS[decoration])
            .join(" ")}
        >
          {span.content}
        </span>
      ))}
    </>
  )
}
