import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import WebTransfer from "./WebTransfer";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {document.querySelector('meta[name="windows-mcp-transfer"]') ? (
      <WebTransfer />
    ) : (
      <App />
    )}
  </React.StrictMode>,
);
