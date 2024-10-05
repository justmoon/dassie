import { useMutation } from "@tanstack/react-query"
import { ExternalLinkIcon } from "lucide-react"
import { Link } from "wouter"

import { Button } from "../../components/ui/button"
import { logout } from "../../utils/authentication"
import { extend } from "../../utils/class-helper"
import { rpc } from "../../utils/rpc"
import { ApiKeysSettings } from "./api-keys/api-keys"

const SettingsSection = extend(
  "SettingsSection",
  "div",
  "grid grid-rows-[auto_auto] space-4 md:grid-rows-1 md:grid-cols-[1fr_2fr]",
)

const SettingsSectionHeader = extend(
  "SettingsSectionHeader",
  "div",
  "p-4 flex flex-col space-y-1.5",
)

const SettingsSectionTitle = extend(
  "SettingsSectionTitle",
  "h2",
  "text-2xl font-semibold leading-none tracking-tight",
)

const SettingsSectionDescription = extend(
  "SettingsSectionDescription",
  "p",
  "text-sm text-muted-foreground",
)

const SettingsSectionContent = extend(
  "SettingsSectionContent",
  "div",
  "p-4 pt-0 md:pt-4 flex flex-col space-y-2",
)

export const Settings = () => {
  const basicState = rpc.general.getBasicState.useQuery()
  const logoutMutation = useMutation({
    mutationFn: logout,
    onSuccess: () => {
      window.location.reload()
    },
  })

  return (
    <div>
      <SettingsSection>
        <SettingsSectionHeader>
          <SettingsSectionTitle>Wallet</SettingsSectionTitle>
          <SettingsSectionDescription>
            Manage your account
          </SettingsSectionDescription>
        </SettingsSectionHeader>
        <SettingsSectionContent>
          <div>
            <h3 className="text-lg font-semibold">Node ID</h3>
            <div>{basicState.data?.nodeId ?? "unknown"}</div>
          </div>
          <div>
            <h3 className="text-lg font-semibold">Payment Pointer</h3>
            <div>${basicState.data?.hostname ?? "???"}</div>
          </div>
          <div className="pt-4">
            <Button
              variant="destructive"
              onClick={() => {
                logoutMutation.mutate()
              }}
            >
              Logout
            </Button>
          </div>
        </SettingsSectionContent>
      </SettingsSection>
      <SettingsSection>
        <SettingsSectionHeader>
          <SettingsSectionTitle>Node Settings</SettingsSectionTitle>
          <SettingsSectionDescription>
            Edit your node settings
          </SettingsSectionDescription>
        </SettingsSectionHeader>
        <SettingsSectionContent>
          <Link href="/network">
            <Button>
              Edit Node Settings <ExternalLinkIcon className="size-4 ml-2" />
            </Button>
          </Link>
        </SettingsSectionContent>
      </SettingsSection>
      <SettingsSection>
        <SettingsSectionHeader>
          <SettingsSectionTitle>API Keys</SettingsSectionTitle>
          <SettingsSectionDescription>
            Manage BTP access
          </SettingsSectionDescription>
        </SettingsSectionHeader>
        <SettingsSectionContent>
          <ApiKeysSettings />
        </SettingsSectionContent>
      </SettingsSection>
    </div>
  )
}
