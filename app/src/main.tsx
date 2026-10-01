import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
// git.cafe's typefaces, bundled: the CSP allows only same-origin fonts, and
// the app cannot count on a network to fetch them from.
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./ui/app.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
