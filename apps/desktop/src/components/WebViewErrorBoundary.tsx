import { SupportedLocale } from '@clipper-helper/i18n';
import React from 'react';
import { commands } from '../ipc/bindings';
import { ErrorBanner } from './ErrorBanner';

interface Props {
  children: React.ReactNode;
  locale: SupportedLocale;
  onOpenDiagnostics: () => void;
}

interface State {
  hasError: boolean;
  errorMessage: string;
}

export class WebViewErrorBoundary extends React.Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, errorMessage: '' };
  }

  static getDerivedStateFromError(error: Error): State {
    return {
      hasError: true,
      errorMessage: error.message || 'Unknown UI rendering error',
    };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    const stackText = [error.stack, info.componentStack].filter(Boolean).join('\n');
    void commands
      .recordWebviewError('error_boundary', error.message || 'Render error', stackText || null)
      .catch(() => undefined);
  }

  render(): React.ReactNode {
    if (this.state.hasError) {
      return (
        <div className="p-6 max-w-2xl mx-auto">
          <ErrorBanner
            locale={this.props.locale}
            error={{
              code: 'E_UNKNOWN',
              message: this.state.errorMessage,
              redactedDetail: this.state.errorMessage,
              actions: [
                {
                  label: 'Muat Ulang Tampilan',
                  variant: 'primary',
                  onClick: () => this.setState({ hasError: false, errorMessage: '' }),
                },
              ],
            }}
            onOpenDiagnostics={() => {
              this.setState({ hasError: false, errorMessage: '' });
              this.props.onOpenDiagnostics();
            }}
          />
        </div>
      );
    }
    return this.props.children;
  }
}

export function installGlobalWebViewErrorHandlers(): () => void {
  const onWindowError = (event: ErrorEvent) => {
    const msg = event.message || 'Unhandled window error';
    const stack =
      event.error instanceof Error
        ? (event.error.stack ?? null)
        : `${event.filename ?? ''}:${event.lineno ?? 0}:${event.colno ?? 0}`;
    void commands.recordWebviewError('window.onerror', msg, stack).catch(() => undefined);
  };

  const onUnhandledRejection = (event: PromiseRejectionEvent) => {
    const reason = event.reason;
    const msg =
      reason instanceof Error ? reason.message : typeof reason === 'string' ? reason : 'Rejection';
    const stack = reason instanceof Error ? (reason.stack ?? null) : null;
    void commands.recordWebviewError('unhandledrejection', msg, stack).catch(() => undefined);
  };

  window.addEventListener('error', onWindowError);
  window.addEventListener('unhandledrejection', onUnhandledRejection);

  return () => {
    window.removeEventListener('error', onWindowError);
    window.removeEventListener('unhandledrejection', onUnhandledRejection);
  };
}
