import process from "node:process"

import { build } from "../src"

await build(process.argv[2])
