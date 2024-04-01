import { DevelopmentLogViewer } from "../../development-log-viewer/development-log-viewer"

const Logs = () => {
  return (
    <div className="grid grid-rows-[auto_1fr] gap-4 h-full max-h-screen py-10">
      <header>
        <h1 className="font-bold leading-tight text-3xl px-4">Logs</h1>
      </header>
      <div className="px-4 min-h-0">
        <DevelopmentLogViewer />
      </div>
    </div>
  )
}

export default Logs
