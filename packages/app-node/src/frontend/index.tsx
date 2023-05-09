/* @refresh reload */
import { createRoot } from "react-dom/client"

import { assertDefined } from "@dassie/lib-type-utils"

import "./index.css"
import Root from "./root"

const rootElement = document.querySelector("#root")
assertDefined(rootElement)
const root = createRoot(rootElement)
root.render(<Root />)
