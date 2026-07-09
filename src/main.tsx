import { App } from "@/App";
import { installGlobalErrorForwarding } from "@/components/ErrorBoundary";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@/styles/globals.css";

installGlobalErrorForwarding();

const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("#root not found");

createRoot(rootEl).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
