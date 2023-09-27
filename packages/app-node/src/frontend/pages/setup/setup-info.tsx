import { Settings2 } from "lucide-react"

import { Alert, AlertDescription, AlertTitle } from "../../components/ui/alert"

export const SetupInfoPage = () => (
  <div className="w-full h-full flex items-center justify-center">
    <Alert className="max-w-5xl">
      <Settings2 className="w-4 h-4" />
      <AlertTitle>Setup</AlertTitle>
      <AlertDescription>
        This Dassie node is ready for setup. Please visit the setup URL provided
        in your server logs or the output of the{" "}
        <pre className="inline-block">dassie init</pre> command.
      </AlertDescription>
    </Alert>
  </div>
)
