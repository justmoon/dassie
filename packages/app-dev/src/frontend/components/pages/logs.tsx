import LogViewer from "../log-viewer/log-viewer"

const Logs = () => {
  return (
    <div className="grid grid-rows-[auto_1fr] gap-4 h-full max-h-screen py-10">
      <header>
        <h1 className="font-bold leading-tight text-3xl px-4">Logs</h1>
      </header>
      <div className="px-4 min-h-0">
        <LogViewer />
      </div>
    </div>
  )
}

export default Logs
