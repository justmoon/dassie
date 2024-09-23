import { startVitest } from "vitest/node"

export async function runVitest() {
  await startVitest("test", [], { run: true })
}
