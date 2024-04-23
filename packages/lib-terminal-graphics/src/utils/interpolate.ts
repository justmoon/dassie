export function interpolate<TShape extends number[]>(
  from: TShape,
  to: TShape,
  progress: number,
): TShape {
  if (from.length !== to.length) {
    throw new Error("Shapes must have the same length")
  }

  return from.map((fromValue, index) => {
    const toValue = to[index]!
    return fromValue + (toValue - fromValue) * progress
  }) as TShape
}

export function interpolateBetweenStops<TShape extends number[]>(
  stops: TShape[],
  progress: number,
): TShape {
  progress = Math.max(0, Math.min(1, progress))

  const stopIndex = Math.floor(progress * (stops.length - 1))
  const from = stops[stopIndex]!
  const to = stops[stopIndex + 1]
  if (to === undefined) {
    return from
  }

  return interpolate(
    from,
    to,
    (progress - stopIndex / (stops.length - 1)) * (stops.length - 1),
  )
}
