#!/usr/bin/env node
import { Extractor, ExtractorResult } from "@microsoft/api-extractor"

import createExtractorConfig from "../common/config/api-extractor"

const extractorConfig = createExtractorConfig("lib-logger")

const extractorResult: ExtractorResult = Extractor.invoke(extractorConfig, {
  // Equivalent to the "--local" command-line parameter
  localBuild: true,

  // Equivalent to the "--verbose" command-line parameter
  showVerboseMessages: true,
})

if (extractorResult.succeeded) {
  console.log("success!")
}
