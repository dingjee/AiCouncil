import { useCallback, useEffect, useRef } from "react";

import { useStore } from "../state/store";

const MIN_WIDTH = 220;
const MAX_WIDTH = 460;
const DEFAULT_WIDTH = 300;

/**
 * Draggable divider between the sidebar and the chat area.
 *
 * Drag to resize, double-click to reset. The width is persisted in the
 * backend settings (`sidebar_width`), so it survives a restart.
 */
export function SidebarResizer() {
  const { state, actions } = useStore();
  const draggingRef = useRef(false);
  const startXRef = useRef(0);
  const startWidthRef = useRef(DEFAULT_WIDTH);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const persist = useCallback(
    (width: number) => {
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
      saveTimerRef.current = setTimeout(() => {
        void actions.updateSettings({ sidebar_width: width });
      }, 400);
    },
    [actions],
  );

  const onPointerDown = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      draggingRef.current = true;
      startXRef.current = event.clientX;
      startWidthRef.current = state.settings.sidebar_width || DEFAULT_WIDTH;
      event.currentTarget.setPointerCapture(event.pointerId);
      event.preventDefault();
    },
    [state.settings.sidebar_width],
  );

  const onPointerMove = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!draggingRef.current) return;
      const delta = event.clientX - startXRef.current;
      const width = Math.min(
        MAX_WIDTH,
        Math.max(MIN_WIDTH, startWidthRef.current + delta),
      );
      actions.setSidebarWidth(width);
    },
    [actions],
  );

  const onPointerUp = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!draggingRef.current) return;
      draggingRef.current = false;
      event.currentTarget.releasePointerCapture?.(event.pointerId);
      persist(state.settings.sidebar_width || DEFAULT_WIDTH);
    },
    [persist, state.settings.sidebar_width],
  );

  const onDoubleClick = useCallback(() => {
    actions.setSidebarWidth(DEFAULT_WIDTH);
    void actions.updateSettings({ sidebar_width: DEFAULT_WIDTH });
  }, [actions]);

  useEffect(
    () => () => {
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    },
    [],
  );

  return (
    <div
      className="divider"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      aria-valuemin={MIN_WIDTH}
      aria-valuemax={MAX_WIDTH}
      aria-valuenow={state.settings.sidebar_width || DEFAULT_WIDTH}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onDoubleClick={onDoubleClick}
      onKeyDown={(event) => {
        const width = state.settings.sidebar_width || DEFAULT_WIDTH;
        if (event.key === "ArrowLeft") {
          event.preventDefault();
          const next = Math.max(MIN_WIDTH, width - 16);
          actions.setSidebarWidth(next);
          void actions.updateSettings({ sidebar_width: next });
        } else if (event.key === "ArrowRight") {
          event.preventDefault();
          const next = Math.min(MAX_WIDTH, width + 16);
          actions.setSidebarWidth(next);
          void actions.updateSettings({ sidebar_width: next });
        }
      }}
    />
  );
}
