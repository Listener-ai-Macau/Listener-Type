import { Component, type ReactNode } from 'react';
import { isTauri } from '../lib/ipc';

interface Props {
  children: ReactNode;
}

interface State {
  error: string | null;
  source: 'frontend' | 'rust-panic';
}

/**
 * Catches unhandled React errors *and* Rust panics emitted via
 * `app.emit("panic:error", ...)`. Shows a recoverable error card
 * instead of letting the WebView go blank.
 */
export class ErrorBoundary extends Component<Props, State> {
  private unlisten: (() => void) | null = null;

  state: State = { error: null, source: 'frontend' };

  static getDerivedStateFromError(error: unknown): Partial<State> {
    const message = error instanceof Error ? error.message : String(error);
    return { error: message, source: 'frontend' };
  }

  componentDidMount() {
    if (!isTauri) return;
    import('@tauri-apps/api/event').then(({ listen }) =>
      listen<{ message: string }>('panic:error', event => {
        this.setState({ error: event.payload.message, source: 'rust-panic' });
      }),
    ).then(unlisten => {
      this.unlisten = unlisten;
    }).catch(() => {});
  }

  componentWillUnmount() {
    this.unlisten?.();
  }

  private handleRecover = () => {
    this.setState({ error: null, source: 'frontend' });
  };

  render() {
    if (this.state.error) {
      return (
        <div style={outer}>
          <div style={card}>
            <div style={iconRow}>
              <span style={iconCircle}>
                <svg width="16" height="16" viewBox="0 0 16 16">
                  <path
                    d="M8 1.5a6.5 6.5 0 100 13 6.5 6.5 0 000-13zM8 5v3.5M8 10.5v1"
                    stroke="var(--ol-err, #e53935)"
                    strokeWidth="1.4"
                    fill="none"
                    strokeLinecap="round"
                  />
                </svg>
              </span>
              <span style={{ fontWeight: 600, fontSize: 13 }}>
                {this.state.source === 'rust-panic' ? '内部错误' : '页面出错'}
              </span>
            </div>
            <p style={msgText}>
              {this.state.error.slice(0, 200)}
            </p>
            <button onClick={this.handleRecover} style={btn}>
              重试
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

const outer: React.CSSProperties = {
  minHeight: '100vh',
  display: 'grid',
  placeItems: 'center',
  background: 'transparent',
  fontFamily: 'var(--ol-font-sans)',
};

const card: React.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'center',
  gap: 12,
  padding: '20px 24px',
  borderRadius: 12,
  background: 'var(--ol-glass-bg-strong)',
  backdropFilter: 'blur(24px) saturate(180%)',
  WebkitBackdropFilter: 'blur(24px) saturate(180%)',
  border: '0.5px solid var(--ol-line-soft)',
  boxShadow: 'var(--ol-shadow-lg)',
  maxWidth: 340,
  textAlign: 'center',
};

const iconRow: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 8,
};

const iconCircle: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'center',
  width: 28,
  height: 28,
  borderRadius: 999,
  background: 'rgba(229, 57, 53, 0.08)',
};

const msgText: React.CSSProperties = {
  margin: 0,
  fontSize: 12,
  color: 'var(--ol-ink-3)',
  lineHeight: 1.5,
  wordBreak: 'break-word',
};

const btn: React.CSSProperties = {
  padding: '6px 20px',
  borderRadius: 999,
  border: '0.5px solid var(--ol-line-soft)',
  background: 'var(--ol-surface)',
  fontSize: 12,
  fontWeight: 600,
  color: 'var(--ol-ink)',
  cursor: 'pointer',
  transition: 'background 0.15s ease',
};
