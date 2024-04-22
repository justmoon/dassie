# @dassie/lib-logger

This library provides logging services for Dassie.

## Basic Usage

```ts
import { createLogger } from "@dassie/lib-logger"

const logger = createLogger("foo:example:http-server")

logger.debug?.("This will print depending on the debug scope")
logger.info("This will always print")
logger.warn("This will print with some emphasis")
logger.error("This will print with maximum emphasis")
logger.logError(new Error("This is an example error"))
```

## Prior Art

- [ts-log](https://tslog.js.org/#/)
- [diagnostics](https://github.com/3rd-Eden/diagnostics)
- [debug](https://github.com/debug-js/debug)
- [Ololog!](https://github.com/xpl/ololog)
