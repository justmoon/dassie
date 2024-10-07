import open, { apps } from "open"

import { type Key, emitKeypressEvents } from "node:readline"

import { createActor } from "@dassie/lib-reactive"

import { server as logger } from "../logger/instances"
import type { DevelopmentActorContext } from "../types/development-base"

const SHORTCUT_HINT = `
  (space) to open in browser
  (r) to restart
  (ctrl+c) to quit
`

export const ListenForKeyboardShortcutsActor = () =>
  createActor((sig: DevelopmentActorContext) => {
    emitKeypressEvents(process.stdin)
    process.stdin.resume()

    if (process.stdin.isTTY) process.stdin.setRawMode(true)

    function handleKeypress(_string: string, key: Key) {
      switch (key.name) {
        case "space": {
          open(
            "https://localhost",
            // Open in incognito mode if ctrl is pressed
            key.ctrl ? { app: { name: apps.browserPrivate } } : undefined,
          ).catch((error: unknown) => {
            logger.error("failed to open in browser", { error })
          })
          break
        }
        case "r": {
          sig.base.restart()

          break
        }
        case "c": {
          if (key.ctrl) {
            process.kill(process.pid, "SIGINT")
          }

          break
        }
        // No default
      }
    }

    process.stdin.on("keypress", handleKeypress)

    if (sig.base.isFirst) {
      // eslint-disable-next-line no-console
      console.log(SHORTCUT_HINT)
    }

    sig.onCleanup(() => {
      process.stdin.off("keypress", handleKeypress)

      process.stdin.pause()

      if (process.stdin.isTTY) process.stdin.setRawMode(false)
    })
  })
