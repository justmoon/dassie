import cx from "classnames"

import { environmentSettingsStore } from "../../../remote-signals/environment-settings"
import { useSig } from "../../../utils/remote-reactive"
import { trpc } from "../../../utils/trpc"

const PeeringModeToggle = () => {
  const sig = useSig()
  const setPeeringMode = trpc.ui.setPeeringMode.useMutation()
  const environmentSettings = sig.get(environmentSettingsStore)

  return (
    <>
      <h2 className="font-bold text-xl">Topology</h2>
      <div className="inline-flex rounded-md shadow-sm py-4" role="group">
        <button
          type="button"
          className={cx(
            "px-4 py-2 text-sm font-medium border rounded-l-lg focus:z-10 focus:ring-2 border-gray-600 text-white hover:text-white focus:ring-blue-500 focus:text-white",
            environmentSettings?.peeringMode === "autopeer"
              ? "bg-slate-600"
              : "bg-gray-700 hover:bg-gray-600"
          )}
          onClick={() => {
            if (environmentSettings?.peeringMode !== "autopeer")
              setPeeringMode.mutate("autopeer")
          }}
        >
          Autopeer
        </button>
        <button
          type="button"
          className={cx(
            "px-4 py-2 text-sm font-medium border rounded-r-lg focus:z-10 focus:ring-2 bg-gray-700 border-gray-600 text-white hover:text-white hover:bg-gray-600 focus:ring-blue-500 focus:text-white",
            environmentSettings?.peeringMode === "fixed"
              ? "bg-slate-600"
              : "bg-gray-700 hover:bg-gray-600"
          )}
          onClick={() => {
            if (environmentSettings?.peeringMode !== "fixed")
              setPeeringMode.mutate("fixed")
          }}
        >
          Fixed
        </button>
      </div>
    </>
  )
}

export default PeeringModeToggle
