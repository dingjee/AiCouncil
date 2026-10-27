import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

/**
 * Top-level error boundary.
 *
 * Without it a render error leaves a **blank page** with no clue what
 * happened. With it the user sees the message, and the diagnostic detail is
 * exposed on `window.__AICOUNCIL_ERROR__` so the native smoke harness can
 * report it instead of only seeing an empty document.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    const detail = {
      message: error?.message ?? String(error),
      stack: error?.stack ?? null,
      componentStack: info?.componentStack ?? null,
      at: new Date().toISOString(),
    };
    (globalThis as { __AICOUNCIL_ERROR__?: unknown }).__AICOUNCIL_ERROR__ = detail;
    // eslint-disable-next-line no-console
    console.error("[aicouncil] render error", error);
  }

  render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="empty" role="alert">
        <div className="empty__title">AiCouncil could not render this view</div>
        <p className="small">{error.message}</p>
        <p className="hint">
          The details are in the developer console; the application log keeps
          the same message.
        </p>
      </div>
    );
  }
}
