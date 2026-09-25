import ReactDOM from "react-dom/client";
import Gate from "./Gate";
import "./gate.css";

// No StrictMode: its double-invoked effects would post the ready handshake
// twice, and the gate's Python side answers the first one.
ReactDOM.createRoot(document.getElementById("root")!).render(<Gate />);
