const LEFT_BLOCKS = [..."▏▎▍▌▋▊▉█"]
const TARGET_TICK_INTERVAL = 100

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

const PULSE = "╍╍╍ "

function getPulse(index: number) {
  return PULSE[index % PULSE.length]!
}

export function getTick(timestamp: number, refreshInterval: number) {
  // Find an interval which is a multiple of the refresh interval but as close
  // as possible to the target interval.
  const tickInterval = Math.max(
    1,
    Math.round(TARGET_TICK_INTERVAL / refreshInterval) * refreshInterval,
  )

  return Math.floor(timestamp / tickInterval)
}

export function generateIndeterminateProgressBar(tick: number, width: number) {
  return generateProgressBar(0, width, tick)
}

export function generateDeterminateProgressBar(
  tick: number,
  progress: number,
  width: number,
) {
  return generateProgressBar(progress, width, tick)
}

export function generateProgressBar(progress: number, width: number, tick = 0) {
  const bar = Array.from({ length: width }).map((_, index) => {
    return (
      index < progress * width ? getLeftBlock(progress * width - index)
      : index === 0 || index === width - 1 ? " "
      : getPulse(Math.abs(-tick + index))
    )
  })

  return bar.join("")
}
