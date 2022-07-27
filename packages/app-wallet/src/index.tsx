/* @refresh reload */
import { createRoot } from "react-dom/client"
import "virtual:windi.css"

import { assertDefined } from "@xen-ilp/lib-type-utils"

import App from "./app"
import "./index.css"
import Uplink from "./modules/uplink"

const rootElement = document.querySelector("#root")
assertDefined(rootElement)
const root = createRoot(rootElement)
root.render(<App />)

new Uplink()
