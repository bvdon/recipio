import { useEffect, useState } from "react";
import { Link, Outlet, useNavigate } from "react-router";
import { ChefHat, FileDown, Globe, Plus, Settings, Share2 } from "lucide-react";
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
          <Link to="/share" className="button" title="Save chosen recipes in a file for a friend">
            <Share2 size={16} aria-hidden />
            Share recipes
          </Link>
          <Link
            to="/share/import"
            className="button"
            title="Add recipes from a .recipio file a friend shared"
          >
            <FileDown size={16} aria-hidden />
            Import shared recipes
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
