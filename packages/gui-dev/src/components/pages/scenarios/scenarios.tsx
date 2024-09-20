import { useLocation } from "wouter"

import { combine } from "@dassie/gui-dassie/src/utils/class-helper"
import { useRemoteSignal } from "@dassie/lib-reactive-rpc/client"

import { rpc } from "../../../utils/rpc"

export function Scenarios() {
  const builtinScenarios = rpc.getBuiltinScenarios.useQuery()
  const currentScenario = useRemoteSignal(rpc.subscribeToScenario)
  const setScenario = rpc.setScenario.useMutation()
  const [, setLocation] = useLocation()

  if (!builtinScenarios.data || !currentScenario) return <div>Loading...</div>

  return (
    <div className="grid grid-rows-[auto_1fr] gap-4 h-full max-h-screen py-10">
      <header>
        <h1 className="font-bold leading-tight text-3xl px-4">Scenarios</h1>
      </header>
      <div className="px-4 flex flex-col space-y-4">
        {builtinScenarios.data.map((scenario) => (
          <div
            key={scenario.id}
            className={combine(
              "rounded-xl border p-4 cursor-pointer flex flex-col gap-3 hover:bg-accent",
              scenario.id === currentScenario ? "bg-accent" : "",
            )}
            onClick={() => {
              setScenario.mutate(scenario.id)
              setLocation("/")
            }}
          >
            <h1 className="text-xl">{scenario.name}</h1>
            <p>{scenario.description}</p>
          </div>
        ))}
      </div>
    </div>
  )
}
