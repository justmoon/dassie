import { ArrowRight, ProhibitInset } from "phosphor-react"

import Tabs from "../../components/tabs/tabs"
import { Button } from "../../components/ui/button"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "../../components/ui/card"

const onSelectNullSubnet = () => {
  throw new Error("not implemented")
}

export const CreateFirstAccount = () => {
  return (
    <div className="flex h-full items-center justify-center">
      <Card className="w-full mx-8">
        <CardHeader>
          <CardTitle>Select a network</CardTitle>
        </CardHeader>
        <CardContent>
          <Tabs.Root className="w-full" defaultValue="testnet">
            <Tabs.List>
              <Tabs.Trigger value="testnet">Test Networks</Tabs.Trigger>
              <Tabs.Trigger value="livenet">Live Networks</Tabs.Trigger>
            </Tabs.List>
            <Tabs.Content value="testnet">
              <div className="p-6 mt-4 max-w-sm bg-white rounded-lg border border-gray-200 shadow-md dark:bg-gray-800 dark:border-gray-700">
                <ProhibitInset
                  aria-hidden="true"
                  className="mb-2 w-10 h-10 text-gray-500 dark:text-gray-400"
                />
                <a href="#">
                  <h5 className="mb-2 text-2xl font-semibold tracking-tight text-gray-900 dark:text-white">
                    Null Subnet
                  </h5>
                </a>
                <p className="mb-3 font-normal text-gray-500 dark:text-gray-400">
                  This subnet allows sending Interledger packets without any
                  underlying settlement.
                </p>
                <Button onClick={onSelectNullSubnet}>
                  Select{" "}
                  <ArrowRight
                    className="ml-2 -mr-1 w-5 h-5"
                    aria-hidden="true"
                  />
                </Button>
              </div>
            </Tabs.Content>
            <Tabs.Content value="livenet">
              <div
                className="p-4 my-4 text-sm text-blue-700 bg-blue-100 rounded-lg dark:bg-blue-200 dark:text-blue-800"
                role="alert"
              >
                Connecting to the Dassie livenet is not yet supported.
              </div>
            </Tabs.Content>
          </Tabs.Root>
        </CardContent>
      </Card>
    </div>
  )
}
