import chalk from "chalk"

import { StaticTerminalComponent } from "../types/terminal-component"

export interface HeaderOptions {
  title: string
}

export const header = ({ title }: HeaderOptions) =>
  ({
    type: "static",
    render: () => ["\n", chalk.blue.inverse.bold(` ${title} `), "\n"],
  } satisfies StaticTerminalComponent)
