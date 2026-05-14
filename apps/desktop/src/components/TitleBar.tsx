import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PanelLeft, Minus, Square, Copy, X } from "lucide-react";
import logo from "../assets/logo.png";

const appWindow = getCurrentWindow();

type TitleBarProps = {
  onToggleSidebar: () => void;
};

/**
 * Custom title bar. The native OS decorations are disabled
 * (`decorations: false` in tauri.conf.json), so this component owns the
 * drag region, the sidebar toggle, and the minimize/maximize/close controls.
 */
export default function TitleBar({ onToggleSidebar }: TitleBarProps) {
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    appWindow.isMaximized().then(setIsMaximized);
    appWindow
      .onResized(() => {
        appWindow.isMaximized().then(setIsMaximized);
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => unlisten?.();
  }, []);

  return (
    <div className="titlebar" data-tauri-drag-region>
      <button
        className="titlebar-button titlebar-toggle"
        onClick={onToggleSidebar}
        title="Mostrar/ocultar barra lateral"
        aria-label="Mostrar/ocultar barra lateral"
      >
        <PanelLeft size={16} />
      </button>

      <div className="titlebar-brand" data-tauri-drag-region>
        <img className="titlebar-logo" src={logo} alt="" draggable={false} />
        <span className="titlebar-title">Skopos</span>
      </div>

      <div className="titlebar-controls">
        <button
          className="titlebar-button"
          onClick={() => appWindow.minimize()}
          title="Minimizar"
          aria-label="Minimizar"
        >
          <Minus size={16} />
        </button>
        <button
          className="titlebar-button"
          onClick={() => appWindow.toggleMaximize()}
          title={isMaximized ? "Restaurar" : "Maximizar"}
          aria-label={isMaximized ? "Restaurar" : "Maximizar"}
        >
          {isMaximized ? <Copy size={14} /> : <Square size={14} />}
        </button>
        <button
          className="titlebar-button titlebar-close"
          onClick={() => appWindow.close()}
          title="Cerrar"
          aria-label="Cerrar"
        >
          <X size={16} />
        </button>
      </div>
    </div>
  );
}
