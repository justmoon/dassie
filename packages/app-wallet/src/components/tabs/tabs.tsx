import * as RadixTabs from "@radix-ui/react-tabs"

import classed from "../../utils/classed"

const TabsRoot = classed(RadixTabs.Root, "")
const TabsList = classed(
  RadixTabs.List,
  "flex flex-wrap text-sm font-medium text-center text-gray-500 border-b border-gray-200 dark:border-gray-700 dark:text-gray-400"
)
const TabsTrigger = classed(
  RadixTabs.Trigger,
  "inline-block p-4 rounded-t-lg hover:text-gray-600 hover:bg-gray-50 dark:hover:bg-gray-800 dark:hover:text-gray-300",
  "radix-state-active:bg-gray-100 radix-state-active:text-blue-600 radix-state-active:dark:bg-gray-800 radix-state-active:dark:text-gray-400"
)
const TabsContent = classed(RadixTabs.Content, "")

const Tabs = {
  Root: TabsRoot,
  List: TabsList,
  Trigger: TabsTrigger,
  Content: TabsContent,
}

export default Tabs
