import axios from "axios"

import { Reactor, createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { ilpHttp as logger } from "../logger/instances"
import { ILP_OVER_HTTP_CONTENT_TYPE } from "./constants/content-type"

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

export const SendIlpHttpPacketsActor = (reactor: Reactor) => {
  const databaseConfigStore = reactor.use(DatabaseConfigStore)
  return createActor(() => {
    return {
      sendAsyncPrepare: async ({
        packet,
        url,
        requestId,
      }: IlpHttpAsyncRequestParameters) => {
        logger.debug("send ilp-http prepare", { packet })
        await axios<Buffer>(url, {
          method: "POST",
          data: packet,
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
          responseType: "arraybuffer",
        })
      },
      sendAsyncResult: async ({
        packet,
        callbackUrl,
        requestId,
      }: IlpHttpAsyncResponseParameters) => {
        logger.debug("send ilp-http result", { packet })
        await axios<Buffer>(callbackUrl, {
          method: "POST",
          data: packet,
          headers: {
            accept: ILP_OVER_HTTP_CONTENT_TYPE,
            "content-type": ILP_OVER_HTTP_CONTENT_TYPE,
            "request-id": requestId,
          },
          responseType: "arraybuffer",
        })
      },
    }
  })
}
