import chalk from "chalk"

import { interpolateBetweenStops } from "../utils/interpolate"

type RgbColor = [r: number, g: number, b: number]

const DEFAULT_GRADIENT: RgbColor[] = [
  [255, 255, 128],
  [255, 128, 191],
  [149, 128, 255],
]
const DEFAULT_INDETERMINATE_GRADIENT: RgbColor[] = [
  [255, 255, 128],
  [255, 128, 191],
  [255, 255, 128],
]

const LEFT_BLOCKS = ["▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"]

function getColorizer(gradient: RgbColor[], relativePosition: number) {
  let color = interpolateBetweenStops<RgbColor>(gradient, relativePosition)
  color = color.map((value) => Math.round(value)) as RgbColor

  return chalk.rgb(...color)
}

function getLeftBlock(fraction: number) {
  if (fraction < 0) {
    return " "
  }

  if (fraction >= 1) {
    return LEFT_BLOCKS[7]!
  }

  const index = Math.floor(fraction * LEFT_BLOCKS.length)

  return LEFT_BLOCKS[index]!
}

export function generateIndeterminateProgressBar(tick: number, width: number) {
  return generateProgressBar(
    1,
    width,
    DEFAULT_INDETERMINATE_GRADIENT,
    Number.MAX_SAFE_INTEGER - tick,
  )
}

export function generateDeterminateProgressBar(
  progress: number,
  width: number,
) {
  return generateProgressBar(progress, width)
}

export function generateProgressBar(
  progress: number,
  width: number,
  gradient: RgbColor[] = DEFAULT_GRADIENT,
  gradientOffset: number = 0,
) {
  const bar = Array.from({ length: width }).map((_, index) => {
    const colorize = getColorizer(
      gradient,
      ((gradientOffset + index) % width) / width,
    )
    return index < progress * width
      ? colorize(getLeftBlock(progress * width - index))
      : " "
  })

  return bar.join("")
}
