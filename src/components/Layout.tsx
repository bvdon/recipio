import { useEffect } from "react";
import { Link, Outlet, useNavigate } from "react-router";
import { ChefHat, Plus, Settings } from "lucide-react";

export default function Layout() {
  const navigate = useNavigate();

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
          <Link to="/recipes/new" className="button primary" title="New recipe (⌘/Ctrl+N)">
            <Plus size={16} aria-hidden />
            New recipe
          </Link>
        </div>
      </header>
      <main className="content">
        <Outlet />
      </main>
    </div>
  );
}
