import type { BaseRequestContext } from "./context"

export interface HttpResult {
  asResponse: (context: BaseRequestContext) => Response
}
