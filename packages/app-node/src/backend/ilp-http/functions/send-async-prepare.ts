import type { Reactor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../../config/database-config"
import { ilpHttp as logger } from "../../logger/instances"
import { ILP_OVER_HTTP_CONTENT_TYPE } from "../constants/content-type"

export interface IlpHttpAsyncRequestParameters {
  packet: Uint8Array
  url: string
  requestId: string
}

export interface IlpHttpAsyncResponseParameters {
  packet: Uint8Array
  callbackUrl: string
  requestId: string
}

export const SendAsyncPrepare = (reactor: Reactor) => {
  const databaseConfigStore = reactor.use(DatabaseConfigStore)

  async function sendAsyncPrepare({
    packet,
    url,
    requestId,
  }: IlpHttpAsyncRequestParameters) {
    logger.debug?.("send ilp-http prepare", { packet })
    await fetch(url, {
      method: "POST",
      body: packet,
      headers: {
        accept: ILP_OVER_HTTP_CONTENT_TYPE,
        "content-type": ILP_OVER_HTTP_CONTENT_TYPE,
        prefer: "respond-async",
        "request-id": requestId,
        "callback-url": `https://${
          databaseConfigStore.read().hostname
        }/ilp/callback}`,
        authorization: "Bearer test-USD",
      },
    })
  }

  return sendAsyncPrepare
}
