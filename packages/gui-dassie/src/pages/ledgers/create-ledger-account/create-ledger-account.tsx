import { ArrowRight, CircleSlash, TestTube2Icon } from "lucide-react"
import { useCallback } from "react"
import { useLocation } from "wouter"

import type { SettlementSchemeId } from "@dassie/app-dassie/src/peer-protocol/types/settlement-scheme-id"

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "../../../components/ui/alert"
import { Button } from "../../../components/ui/button"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "../../../components/ui/card"
import { rpc } from "../../../utils/rpc"

export const CreateLedgerAccount = () => {
  const [, navigate] = useLocation()
  const addSettlementScheme = rpc.ledgers.addSettlementScheme.useMutation({
    onSuccess: () => {
      navigate("/ledgers")
    },
  })

  const onSelectStubSettlement = useCallback(() => {
    addSettlementScheme.mutate({
      id: "stub" as SettlementSchemeId,
      config: {},
    })
  }, [addSettlementScheme])

  return (
    <div className="flex h-full items-center justify-center">
      <Card className="w-full max-w-lg mx-8 h-full max-h-lg">
        <CardHeader>
          <CardTitle>Choose a Ledger Type</CardTitle>
        </CardHeader>
        <CardContent>
          <Alert>
            <TestTube2Icon className="h-4 w-4" />
            <AlertTitle>Testnet</AlertTitle>
            <AlertDescription>
              You are connected to the Dassie test network. The settlement
              methods below do <strong>not</strong> use real money.
            </AlertDescription>
          </Alert>
          <div className="p-6 mt-4 bg-white rounded-lg border border-gray-200 shadow-md dark:bg-gray-800 dark:border-gray-700">
            <CircleSlash
              aria-hidden="true"
              className="mb-2 w-10 h-10 text-gray-500 dark:text-gray-400"
            />
            <a href="#">
              <h5 className="mb-2 text-2xl font-semibold tracking-tight text-gray-900 dark:text-white">
                Stub Settlement
              </h5>
            </a>
            <p className="mb-3 font-normal text-gray-500 dark:text-gray-400">
              This settlement scheme simulates settlement delays but does not
              actually connect to any external ledger.
            </p>
            <Button
              onClick={onSelectStubSettlement}
              disabled={addSettlementScheme.isPending}
            >
              Select{" "}
              <ArrowRight className="ml-2 -mr-1 w-5 h-5" aria-hidden="true" />
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
