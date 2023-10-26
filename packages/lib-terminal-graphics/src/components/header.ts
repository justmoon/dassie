import chalk from "chalk"

import { StaticTerminalComponent } from "../types/terminal-component"

export interface HeaderOptions {
  title: string
  paddingTop?: number
  paddingBottom?: number
}

export const header = ({
  title,
  paddingTop = 1,
  paddingBottom = 0,
}: HeaderOptions) =>
  ({
    type: "static",
    render: () => [
      "\n".repeat(paddingTop),
      chalk.blue.inverse.bold(` ${title} `),
      "\n".repeat(1 + paddingBottom),
    ],
  }) satisfies StaticTerminalComponent
