import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import TitleBar from "./components/TitleBar";

export default function App() {
  const [status, setStatus] = useState<string>("loading…");
  const [sidebarOpen, setSidebarOpen] = useState(true);

  useEffect(() => {
    invoke<string>("app_status")
      .then(setStatus)
      .catch((err) => setStatus(`error: ${String(err)}`));
  }, []);

  return (
    <div className="app">
      <TitleBar onToggleSidebar={() => setSidebarOpen((open) => !open)} />

      <div className="app-body">
        {sidebarOpen && (
          <aside className="sidebar">
            <nav className="sidebar-nav">
              <a className="sidebar-item">Resumen</a>
              <a className="sidebar-item">Eventos</a>
              <a className="sidebar-item">Modelos</a>
              <a className="sidebar-item">Ajustes</a>
            </nav>
          </aside>
        )}

        <main className="content">
          <h1>Skopos</h1>
          <p className="tagline">Local-first AI usage observability.</p>
          <p className="status">
            backend: <code>{status}</code>
          </p>
        </main>
      </div>
    </div>
  );
}
