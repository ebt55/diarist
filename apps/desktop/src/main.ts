import { mount } from "svelte";
import App from "./App.svelte";
import { live } from "./lib/live.svelte";
import { app } from "./lib/state.svelte";
import "./app.css";

// Start the first IPC round trips before mounting so data and DOM overlap.
(window as unknown as { __boot: number }).__boot = performance.now();
app.init();
live.init();

const target = document.getElementById("app");
if (!target) throw new Error("#app not found in index.html");

export default mount(App, { target });
