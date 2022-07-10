import ansi, { parse } from "ansicolor"
import LinkifyIt from "linkify-it"
import type { Component } from "solid-js"
import { For } from "solid-js"

import { selectBySeed } from "@xen-ilp/lib-logger"

import type { IndexedLogLine } from "../../../backend/features/logs"
import { COLORS } from "../../constants/palette"
import { startupTime } from "../../signals/startup-time"

const linkify = new LinkifyIt()

/**
 * Dracula theme.
 *
 * @see https://draculatheme.com/
 */
ansi.rgb = {
  black: [38, 38, 38],
  red: [227, 86, 167],
  green: [66, 230, 108],
  yellow: [228, 243, 74],
  blue: [155, 107, 223],
  magenta: [230, 71, 71],
  cyan: [117, 215, 236],
  lightGray: [239, 165, 84],
  darkGray: [122, 122, 122],
  lightRed: [255, 121, 198],
  lightGreen: [80, 250, 123],
  lightYellow: [241, 250, 140],
  lightBlue: [189, 147, 249],
  lightMagenta: [255, 85, 85],
  lightCyan: [139, 233, 253],
  white: [255, 184, 108],
}

export const linkifyText = (text: string) => {
  const matches = linkify.match(text) ?? []

  const elements = []
  let lastIndex = 0

  for (const match of matches) {
    if (match.index > lastIndex) {
      elements.push(text.slice(lastIndex, match.index))
    }

    const link = (
      <a
        href={match.url}
        target="blank"
        class="underline-gray-400 hover:underline"
      >
        {match.text}
      </a>
    )
    elements.push(link)

    lastIndex = match.lastIndex
  }

  if (text.length > lastIndex) {
    elements.push(text.slice(lastIndex))
  }

  return elements
}

export const formatLogMessage = (message: string) =>
  parse(message).spans.map(({ css, text }) => (
    <span
      style={
        // ansicolors' result for text that has no specified color but is
        // dimmed assumes that the text is black. This corrects that.
        css === "color:rgba(0,0,0,0.5);" ? "color:rgba(255,255,255,0.5)" : css
      }
    >
      {linkifyText(text)}
    </span>
  ))

const LogLine: Component<IndexedLogLine> = (log) => {
  return (
    <div class="flex text-xs py-0.5" style={{ order: -log.index }}>
      <div class="font-mono flex-shrink-0 text-right px-2 text-gray-400 w-20">
        {((Number(new Date(log.date)) - (startupTime() ?? 0)) / 1000).toFixed(
          3
        )}
      </div>
      <div
        class="font-bold flex-shrink-0 text-center w-8"
        style={{ color: selectBySeed(COLORS, log.node) }}
      >
        {log.node}
      </div>
      <pre class="font-mono px-2 break-all">
        <span style={{ color: selectBySeed(COLORS, log.component) }}>
          {log.component}
        </span>{" "}
        <span>{formatLogMessage(log.error ? log.error : log.message)}</span>
        <For each={Object.entries(log.data ?? {})}>
          {([key, value]) => (
            <span class="bg-dark-100 rounded-1 text-xs ml-1 py-0.5 px-1">
              <span class="font-sans text-gray-400">{key}=</span>
              <span>{String(value)}</span>
            </span>
          )}
        </For>
      </pre>
    </div>
  )
}

export default LogLine
