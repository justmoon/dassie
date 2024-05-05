// eslint-disable-next-line unicorn/import-style
import {
  type BackgroundColorName,
  type ForegroundColorName,
  backgroundColorNames,
  foregroundColorNames,
} from "chalk"

export function foregroundToBackgroundColor(
  foregroundColor: ForegroundColorName,
): BackgroundColorName {
  const index = foregroundColorNames.indexOf(foregroundColor)

  if (index === -1) {
    return "bgGray"
  }

  return backgroundColorNames[index]!
}
