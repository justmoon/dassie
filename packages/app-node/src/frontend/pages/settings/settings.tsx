import { useMutation } from "@tanstack/react-query"

import { Button } from "../../components/ui/button"
import { logout } from "../../utils/authentication"
import { queryClientReactContext } from "../../utils/trpc"

export const Settings = () => {
  const logoutMutation = useMutation({
    mutationFn: logout,
    onSuccess: () => {
      window.location.reload()
    },
    context: queryClientReactContext,
  })
  return (
    <div>
      <Button onClick={() => logoutMutation.mutate()}>Logout</Button>
    </div>
  )
}
