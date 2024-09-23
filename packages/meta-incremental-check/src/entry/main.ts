import { command, flag, run } from "cmd-ts"

import { runChecks } from "../index.ts"

const cmd = command({
  name: "incremental-check",
  description: "Perform checks on the Dassie monorepo",
  version: "1.0.0",
  args: {
    all: flag({
      long: "all",
      short: "a",
      description: "Run all checks even if inputs have not changed",
    }),
  },
  handler: async ({ all }) => {
    await runChecks({ all })
  },
})

await run(cmd, process.argv.slice(2))
