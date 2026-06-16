import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./index.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("missing #root element");
}

function mount() {
  createRoot(root!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

// E2E mock harness (Task 520): when `?mock=1` is present, lazily load + install
// the Core-free DataClient BEFORE mounting so the app picks it up. The dynamic
// import keeps the mock out of the normal production chunk.
if (new URLSearchParams(window.location.search).has("mock")) {
  void import("./lib/mock-setup")
    .then((m) => m.installMock())
    .finally(mount);
} else {
  mount();
}
