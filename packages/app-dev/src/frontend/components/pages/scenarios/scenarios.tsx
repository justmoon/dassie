import { useLocation } from "wouter"

import { combine } from "@dassie/app-node/src/frontend/utils/class-helper"
import { useRemoteSignal } from "@dassie/lib-reactive-rpc/client"

import { builtinScenarios } from "../../../../backend/constants/scenarios"
import { rpc } from "../../../utils/rpc"

export function Scenarios() {
  const currentScenario = useRemoteSignal(rpc.ui.subscribeToScenario)
  const setScenario = rpc.ui.setScenario.useMutation()
  const [, setLocation] = useLocation()

  if (!currentScenario) return <div>Loading...</div>

  return (
    <div className="grid grid-rows-[auto_1fr] gap-4 h-full max-h-screen py-10">
      <header>
        <h1 className="font-bold leading-tight text-3xl px-4">Scenarios</h1>
      </header>
      <div className="px-4 flex flex-col space-y-4">
        {Object.values(builtinScenarios).map((scenario) => (
          <div
            key={scenario.id}
            className={combine(
              "rounded-xl border p-4 cursor-pointer",
              scenario.id === currentScenario.id ? "bg-accent" : "",
            )}
            onClick={() => {
              setScenario.mutate(scenario)
              setLocation("/")
            }}
          >
            <h1 className="text-xl">{scenario.name}</h1>
          </div>
        ))}
      </div>
    </div>
  )
}
