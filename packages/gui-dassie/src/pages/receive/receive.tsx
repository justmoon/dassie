import {
  type ComponentType,
  type LazyExoticComponent,
  Suspense,
  lazy,
} from "react"
import { Link } from "wouter"

import type { SettlementSchemeId } from "@dassie/app-dassie/src/peer-protocol/types/settlement-scheme-id"

import { Button } from "../../components/ui/button"
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
} from "../../components/ui/card"
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../../components/ui/tabs"
import { SCHEME_NAME_MAP } from "../../constants/schemes"
import { rpc } from "../../utils/rpc"

const SCHEME_DEPOSIT_UI_MAP: Record<
  SettlementSchemeId,
  LazyExoticComponent<ComponentType> | null
> = {
  stub: lazy(() => import("./settlement-schemes/stub")),
  xrpl: null,
  "xrpl-testnet": null,
}

export function ReceivePage() {
  const basicState = rpc.general.getBasicState.useQuery()

  const activeSettlementSchemes = basicState.data?.activeSettlementSchemes ?? []

  return (
    <div className="flex h-full items-center justify-center">
      <Card>
        <CardHeader>
          <CardTitle className="flex-grow flex-shrink-0 basis-auto font-bold text-lg md:text-xl">
            Receive
          </CardTitle>
        </CardHeader>
        <CardContent>
          <Tabs defaultValue="interledger">
            <TabsList>
              <TabsTrigger value="interledger">Interledger</TabsTrigger>
              {activeSettlementSchemes.map((schemeId) =>
                SCHEME_NAME_MAP[schemeId] ?
                  <TabsTrigger key={schemeId} value={schemeId}>
                    {SCHEME_NAME_MAP[schemeId]}
                  </TabsTrigger>
                : null,
              )}
            </TabsList>
            <TabsContent value="interledger">
              <p>Send money to the following payment pointer:</p>
              <div className="text-lg font-bold border rounded-lg p-4 mt-4">
                {basicState.data ? `$${basicState.data.hostname}` : "???"}
              </div>
            </TabsContent>

            {activeSettlementSchemes.map((schemeId) => {
              const Component = SCHEME_DEPOSIT_UI_MAP[schemeId]

              if (!Component) {
                return null
              }

              return (
                <TabsContent key={schemeId} value={schemeId}>
                  <Suspense fallback={<div>Loading...</div>}>
                    <Component />
                  </Suspense>
                </TabsContent>
              )
            })}
          </Tabs>
        </CardContent>
        <CardFooter>
          <Link href="/">
            <Button variant="ghost">Back</Button>
          </Link>
        </CardFooter>
      </Card>
    </div>
  )
}
