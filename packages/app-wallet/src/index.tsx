/* @refresh reload */
import { render } from "solid-js/web"
import "virtual:windi.css"

import { assertDefined } from "@xen-ilp/lib-type-utils"

import App from "./app"
import "./index.css"
import Uplink from "./modules/uplink"

const rootElement = document.querySelector("#root")
assertDefined(rootElement)
render(() => <App />, rootElement)

new Uplink()
