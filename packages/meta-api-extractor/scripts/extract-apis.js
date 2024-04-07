#!/usr/bin/env node
import { Extractor } from "@microsoft/api-extractor"

import createExtractorConfig from "../src/base-config.js"

const extractApi = (module) => {
  const extractorConfig = createExtractorConfig(module)

  const extractorResult = Extractor.invoke(extractorConfig, {
    // Equivalent to the "--local" command-line parameter
    localBuild: true,

    // Equivalent to the "--verbose" command-line parameter
    showVerboseMessages: true,

    typescriptCompilerFolder: "node_modules/typescript",
  })

  if (extractorResult.succeeded) {
    console.log("success!")
  }
}

extractApi("lib-logger")
extractApi("lib-oer")
extractApi("lib-type-utils")
