import { getCurrentWindow } from "@tauri-apps/api/window";
import type { CaseInfo } from "../api";

const win = getCurrentWindow();

// Custom titlebar (SPEC.md §9, UX redesign): tauri.conf.json sets
// decorations: false, so there's no OS-native window chrome at all —
// these three buttons are the *only* way to minimize/maximize/close the
// window. `data-tauri-drag-region` is kept as the standard declarative
// hint, but it only matches the *exact* element under the pointer, not
// its ancestors — a plain child (e.g. a text node inside .titlebar-brand)
// falls through it with no drag starting, which is what made most of the
// bar feel undraggable. The onMouseDown handler below is the reliable
// fix: React bubbles the event up from wherever it actually started, so
// startDragging() fires for a press anywhere in this div, not just the
// handful of elements explicitly tagged.
export default function TitleBar({ current }: { current: CaseInfo | null }) {
  return (
    <div className="titlebar">
      <div
        className="titlebar-drag"
        data-tauri-drag-region
        onMouseDown={(e) => {
          if (e.button === 0) win.startDragging();
        }}
      >
        <div className="titlebar-brand">
          <div className="titlebar-mark">E</div>
          <span className="titlebar-name">Eumeaus</span>
        </div>
        <div style={{ flex: 1 }} data-tauri-drag-region />
        {current && (
          <div className="titlebar-case">
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="#6fb98a" strokeWidth="1.4">
              <rect x="3" y="7" width="10" height="7" rx="1.5"></rect>
              <path d="M5.5 7V5a2.5 2.5 0 015 0v2"></path>
            </svg>
            <span className="titlebar-case-name">{current.name}</span>
            <span className="titlebar-case-id">{current.id.slice(0, 8)}</span>
          </div>
        )}
        <div style={{ flex: 1 }} data-tauri-drag-region />
      </div>
      <div className="titlebar-controls">
        <button
          className="titlebar-btn"
          title="Minimize"
          onClick={() => win.minimize()}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" stroke="#c9c9d6" strokeWidth="1">
            <path d="M0 5h10"></path>
          </svg>
        </button>
        <button
          className="titlebar-btn"
          title="Maximize"
          onClick={() => win.toggleMaximize()}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="#c9c9d6" strokeWidth="1">
            <rect x="0.5" y="0.5" width="9" height="9"></rect>
          </svg>
        </button>
        <button
          className="titlebar-btn danger"
          title="Close"
          onClick={() => win.close()}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" stroke="#c9c9d6" strokeWidth="1">
            <path d="M0 0l10 10M10 0L0 10"></path>
          </svg>
        </button>
      </div>
    </div>
  );
}
