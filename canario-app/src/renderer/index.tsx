// Solid entry point
import { render } from "solid-js/web";
import "./index.css";
import App from "./App";

const root = document.getElementById("root");

if (!root) {
  document.body.innerHTML = '<p style="color:red;padding:2em">Error: #root not found</p>';
} else {
  try {
    render(() => <App />, root);
  } catch (err) {
    root.innerHTML = `<p style="color:red;padding:2em">Render error: ${err}</p>`;
    console.error("Canario render error:", err);
  }
}
