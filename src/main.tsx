import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createHashRouter, RouterProvider } from "react-router";
import { error as logError } from "@tauri-apps/plugin-log";
import Layout from "./components/Layout";
import Library from "./pages/Library";
import RecipeView from "./pages/RecipeView";
import RecipeEditor from "./pages/RecipeEditor";
import Organize from "./pages/Organize";
import Settings from "./pages/Settings";
import CookingMode from "./pages/CookingMode";
import "./styles.css";

window.addEventListener("error", (e) => void logError(`ui: ${e.message}`));
window.addEventListener(
  "unhandledrejection",
  (e) => void logError(`ui: unhandled rejection: ${String(e.reason)}`),
);

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});

// Hash routing: the app is served from a custom protocol, so there is no server to
// rewrite deep links to index.html.
const router = createHashRouter([
  {
    element: <Layout />,
    children: [
      { index: true, element: <Library /> },
      { path: "recipes/new", element: <RecipeEditor /> },
      { path: "recipes/:id", element: <RecipeView /> },
      { path: "recipes/:id/edit", element: <RecipeEditor /> },
      { path: "organize", element: <Organize /> },
      { path: "settings", element: <Settings /> },
    ],
  },
  // Full screen, without the app's top bar.
  { path: "recipes/:id/cook", element: <CookingMode /> },
]);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </React.StrictMode>,
);
