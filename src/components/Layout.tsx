import { useEffect, useState } from "react";
import { Link, Outlet, useNavigate } from "react-router";
import { ChefHat, Globe, Plus, Settings, Users } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { api, events } from "../api";
import ImportRecipeDialog from "./ImportRecipeDialog";

export default function Layout() {
  const navigate = useNavigate();
  const [importing, setImporting] = useState(false);

  // Cmd/Ctrl+N: new recipe from anywhere.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "n") {
        e.preventDefault();
        navigate("/recipes/new");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

  // A .recipio file opened from Finder, Explorer or a Messages/Mail attachment goes
  // straight to the import screen (queued in Rust if it arrived before the UI was ready).
  useEffect(() => {
    const openFile = (file: string | undefined) => {
      if (file) navigate(`/share/import?file=${encodeURIComponent(file)}`);
    };
    void api.takeOpenedFiles().then((files) => openFile(files[files.length - 1]));
    const unlisten = listen<string[]>(events.openFiles, () => {
      void api.takeOpenedFiles().then((files) => openFile(files[files.length - 1]));
    });
    return () => void unlisten.then((stop) => stop());
  }, [navigate]);

  return (
    <div className="app">
      <header className="topbar">
        <Link to="/" className="brand">
          <ChefHat size={22} aria-hidden />
          Recipio
        </Link>
        <div className="actions">
          <Link to="/settings" className="button ghost icon" title="Settings" aria-label="Settings">
            <Settings size={18} aria-hidden />
          </Link>
          <Link to="/friends" className="button" title="Share recipes with friends">
            <Users size={16} aria-hidden />
            Friends
          </Link>
          <button type="button" className="button" onClick={() => setImporting(true)}>
            <Globe size={16} aria-hidden />
            Import from web
          </button>
          <Link to="/recipes/new" className="button primary" title="New recipe (⌘/Ctrl+N)">
            <Plus size={16} aria-hidden />
            New recipe
          </Link>
        </div>
      </header>
      <main className="content">
        <Outlet />
      </main>
      {importing && <ImportRecipeDialog onClose={() => setImporting(false)} />}
    </div>
  );
}
