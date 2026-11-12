import React from 'react';
import {
  ActivityIndicator as NativeActivityIndicator,
  NativeModules,
  Pressable as NativePressable,
  Text as NativeText,
  View as NativeView,
} from 'react-native';
import { EventHandler, HostHttpHandler } from 'askit/host-events';
import { Engine, type ComponentMap } from 'keel/host';
import { ScrollView, Text, TextInput, View } from 'keel/host/preset';

import type { TabConfig } from './types';

type GuestButtonProps = {
  title: string;
  onPress?: () => void;
  color?: string;
  disabled?: boolean;
};

const GuestButton: React.FC<GuestButtonProps> = ({
  title,
  onPress,
  color = '#2563eb',
  disabled = false,
}) => (
  <NativePressable
    accessibilityRole='button'
    disabled={disabled}
    onPress={onPress}
    style={({ pressed }) => ({
      alignItems: 'center',
      backgroundColor: disabled ? '#31527f' : color,
      borderRadius: 10,
      opacity: pressed ? 0.8 : 1,
      padding: 14,
    })}
  >
    <NativeText style={{ color: '#ffffff', fontSize: 16, fontWeight: '600' }}>
      {title}
    </NativeText>
  </NativePressable>
);

const hostComponents: ComponentMap = {
  Button: GuestButton as unknown as React.ComponentType<
    Record<string, unknown>
  >,
  ScrollView: ScrollView as React.ComponentType<Record<string, unknown>>,
  Text: Text as React.ComponentType<Record<string, unknown>>,
  TextInput: TextInput as React.ComponentType<Record<string, unknown>>,
  View: View as React.ComponentType<Record<string, unknown>>,
};

type ActorCallResult = {
  success: boolean;
  payloadBase64?: string;
  error?: string;
};

type Props = {
  tab: TabConfig;
  containerWidth?: number;
  containerHeight?: number;
};

const logToNative = (message: string) => {
  console.log(message);
  try {
    NativeModules.NativeLogger?.log?.(message);
  } catch {
    // Logging must never affect guest execution.
  }
};

export const StandaloneGuestApp: React.FC<Props> = ({
  tab,
  containerWidth,
  containerHeight,
}) => {
  const [content, setContent] = React.useState<React.ReactElement | null>(null);
  const [error, setError] = React.useState('');
  const guestScript = tab.unifiedScript || tab.guestScript;
  const hasActorCallPermission = (tab.permissions ?? []).includes('actor.call');

  React.useEffect(() => {
    // A reload must start from a clean state, otherwise a previous load
    // failure keeps rendering even after the new bundle loads successfully.
    setError('');
    setContent(null);

    if (!guestScript) {
      setError('The application does not contain a Keel guest bundle.');
      return;
    }

    let disposed = false;
    const engine = new Engine({ debug: false });
    engine.register(hostComponents);
    engine.createReceiver();

    const update = () => {
      if (disposed) return;
      const next = engine.getReceiver()?.render();
      setContent(next as React.ReactElement | null);
    };
    const unsubscribeUpdate = engine.on('update', update);
    const unsubscribeEvents = EventHandler.setup(engine, {
      tabId: tab.id,
      onLog: (direction, event) => {
        logToNative(`[StandaloneGuestApp] ${direction}: ${event}`);
      },
      handlers: {
        ACTR_CALL: {
          responseEvent: 'ACTR_CALL_RESULT',
          handle: async (payload: {
            requestId?: string;
            routeKey?: unknown;
            payloadBase64?: unknown;
            timeoutMs?: unknown;
          }) => {
            const requestId = payload?.requestId;
            if (!hasActorCallPermission) {
              return {
                requestId,
                success: false,
                error:
                  'The Actor package does not declare the actor.call capability.',
              };
            }
            if (
              typeof payload?.routeKey !== 'string' ||
              typeof payload?.payloadBase64 !== 'string'
            ) {
              return {
                requestId,
                success: false,
                error: 'ACTR_CALL requires routeKey and payloadBase64 strings.',
              };
            }

            const gatewayUrl =
              globalThis.__ACTRIUM_ACTR_GATEWAY_URL__ ??
              'http://127.0.0.1:8391/v1/call';
            logToNative(
              `[StandaloneGuestApp] ACTR_CALL ${payload.routeKey}`,
            );
            const result = await HostHttpHandler.handleRequest<ActorCallResult>({
              requestId,
              url: gatewayUrl,
              method: 'POST',
              body: {
                routeKey: payload.routeKey,
                payloadBase64: payload.payloadBase64,
                timeoutMs:
                  typeof payload.timeoutMs === 'number'
                    ? payload.timeoutMs
                    : 30_000,
              },
            });
            const response = {
              requestId,
              success: result.success && result.data?.success === true,
              payloadBase64: result.data?.payloadBase64,
              error: result.data?.error ?? result.message,
            };
            logToNative(
              `[StandaloneGuestApp] ACTR_CALL completed: ${response.success ? 'success' : 'failure'}`,
            );
            return response;
          },
        },
      },
    });

    void engine
      .loadBundle(guestScript, { containerHeight, containerWidth })
      .then(() => {
        const stats = engine.getReceiver()?.getStats();
        logToNative(
          `[StandaloneGuestApp] guest loaded: roots=${stats?.rootChildrenCount ?? 0}, nodes=${stats?.nodeCount ?? 0}`,
        );
        update();
        setTimeout(update, 0);
      })
      .catch((cause) => {
        if (!disposed) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      });

    return () => {
      disposed = true;
      unsubscribeEvents();
      unsubscribeUpdate();
      engine.destroy();
    };
  }, [
    // Do not recreate the engine on window resize. The first layout size is
    // enough for the guest bundle, and RN handles later size changes.
    // reloadKey still forces a clean reload when the package changes.
    guestScript,
    hasActorCallPermission,
    tab.id,
    tab.reloadKey,
  ]);

  if (error) {
    return (
      <NativeView
        style={{ flex: 1, padding: 24, backgroundColor: '#2b0b0b' }}
      >
        <NativeText style={{ color: '#fda4af' }}>{error}</NativeText>
      </NativeView>
    );
  }

  return (
    <NativeView collapsable={false} style={{ flex: 1 }}>
      {content ?? (
        <NativeView
          style={{
            flex: 1,
            alignItems: 'center',
            justifyContent: 'center',
            backgroundColor: '#101827',
          }}
        >
          <NativeActivityIndicator color='#60a5fa' />
        </NativeView>
      )}
    </NativeView>
  );
};
