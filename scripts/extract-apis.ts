#!/usr/bin/env node
import { Extractor, ExtractorResult } from "@microsoft/api-extractor"

import createExtractorConfig from "../common/config/api-extractor"

const extractApi = (module: string) => {
  const extractorConfig = createExtractorConfig(module)

  const extractorResult: ExtractorResult = Extractor.invoke(extractorConfig, {
    // Equivalent to the "--local" command-line parameter
    localBuild: true,

    // Equivalent to the "--verbose" command-line parameter
    showVerboseMessages: true,
  })

  if (extractorResult.succeeded) {
    console.log("success!")
  }
}

extractApi("lib-logger")
extractApi("lib-type-utils")
